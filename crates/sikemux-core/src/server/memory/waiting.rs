//! The org's waiting list: entries members moved to a shared project's org store while its owners
//! and admins approve its rules. A member's entry stays in their project's own store, marked
//! waiting, until an owner or admin keeps it in the org's store or drops it; each owner and admin
//! is asked by a call.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallAsker, CallKind, CallOption, CallOutcome, CallResult, PersonCall};
use sikemux_view::memory::{Knowledge, MemoryRule, Store};

use super::org::Shared;
use super::{holding, moving, tell_clients, Held, State, MAX_ENTRIES};
use crate::server::board::goal::learned;
use crate::server::parts::Parts;
use crate::server::person_calls::{self, Raise, ORG_WAITING};
use crate::server::vault::org::{self as vault, Put, PROJECT_MEMORY_DROPS, PROJECT_MEMORY_WAITING};
use crate::server::Core;
use crate::sessions::asked::may_hold_secret;

const TRIES: usize = 3;
const KEEP: &str = "keep";
const DROP: &str = "drop";
/// The most entries one project's store keeps waiting at once, so a member's list stays well
/// inside what the vault seals in one entry.
const MAX_WAITING: usize = 30;

/// One member's entries waiting on the org's list, by the store they are in and their id there.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mine {
    #[serde(default)]
    items: BTreeMap<String, Waiting>,
}

/// The waiting entries an owner or admin dropped, by member and the entry's key on their list.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Drops {
    #[serde(default)]
    items: BTreeMap<String, Dropped>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dropped {
    /// The hash of the words the owner or admin read.
    words: String,
    /// The membership of the owner or admin who dropped it.
    by: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum Waiting {
    Rule { rule: Box<MemoryRule> },
    Knowledge { knowledge: Box<Knowledge> },
}

impl Waiting {
    fn text(&self) -> &str {
        match self {
            Waiting::Rule { rule } => &rule.text,
            Waiting::Knowledge { knowledge } => &knowledge.text,
        }
    }

    /// Marked sensitive, or in words that look like a credential.
    fn sensitive(&self) -> bool {
        let marked = match self {
            Waiting::Rule { rule } => rule.sensitive,
            Waiting::Knowledge { knowledge } => knowledge.sensitive,
        };
        Self::is_sensitive(marked, self.text())
    }

    fn of(held: &Held, id: u64) -> Option<Self> {
        let rule = (held.rules.iter().find(|rule| rule.id == id)).map(|rule| Waiting::Rule {
            rule: Box::new(super::org::unnamed(rule)),
        });
        rule.or_else(|| {
            (held.knowledge.iter().find(|known| known.id == id)).map(|known| Waiting::Knowledge {
                knowledge: Box::new(super::org::as_shared(known)),
            })
        })
    }
}

/// What this host's owner or admin answered that the org's vault does not hold yet.
#[derive(Default)]
pub(super) struct Answers {
    /// Why the org's store would not take an entry kept here, by its words.
    not_kept: HashMap<String, String>,
}

/// Ends the waiting, dropped and refused marks of each project open here whose share changed
/// since they were kept, so a rule sent to one org's admins is never listed for another's.
/// `orgs` holds the org each project open here is shared with, by its own store's key. True when
/// a mark ended, so the Playbook no longer shows it.
pub(super) fn follow_shares(
    core: &Core,
    state: &mut State,
    orgs: &HashMap<String, String>,
) -> bool {
    let open: HashSet<&String> = state.keys.values().collect();
    let mut cleared = false;
    let changed: Vec<String> = (state.held.values_mut())
        .filter(|held| held.store == Store::Project && !held.mirrored && open.contains(&held.key))
        .filter_map(|held| {
            let now = orgs.get(&held.key);
            if held.shared_with.as_ref() == now {
                return None;
            }
            held.shared_with = now.cloned();
            cleared |=
                !(held.waiting.is_empty() && held.dropped.is_empty() && held.refused.is_empty());
            held.waiting.clear();
            held.dropped.clear();
            held.refused.clear();
            Some(held.key.clone())
        })
        .collect();
    for key in changed {
        core.memory().save(state, &key);
    }
    cleared
}

/// Where the store under `local` puts its entries on its member's list, without naming the
/// project's folder to the org.
fn store_tag(local: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(
        Sha256::digest(local.as_bytes())
            .into_iter()
            .take(8)
            .collect::<Vec<u8>>(),
    )
}

fn item_key(local: &str, id: u64) -> String {
    format!("{}:{id}", store_tag(local))
}

fn words(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn dropped_key(member: &str, item: &str) -> String {
    format!("{member}|{item}")
}

fn call_key(shared: &Shared, member: &str, item: &str, entry: &Waiting) -> String {
    format!(
        "{ORG_WAITING}{}|{member}|{item}|{}",
        shared.key(),
        words(entry.text())
    )
}

/// The org's store, the member, the entry on their list and its words' hash, from a call's key.
fn parse(key: &str) -> Option<(&str, &str, &str, &str)> {
    let (store, rest) = key.strip_prefix(ORG_WAITING)?.split_once('|')?;
    let (member, rest) = rest.split_once('|')?;
    let (item, words) = rest.rsplit_once('|')?;
    Some((store, member, item, words))
}

/// An entry of the org's vault, with its revision, or an empty one when there is none; None
/// while this host cannot read the vault or the entry makes no sense.
fn read<T: serde::de::DeserializeOwned + Default>(
    core: &Core,
    shared: &Shared,
    id: &str,
) -> Option<(i64, T)> {
    match vault::read(core, &shared.org, id)? {
        Some(read) => match serde_json::from_value(read.body) {
            Ok(body) => Some((read.revision, body)),
            Err(error) => {
                eprintln!("sikemux core: an org's waiting rules could not be read: {error}");
                None
            }
        },
        None => Some((0, T::default())),
    }
}

enum Wrote {
    /// Written, or not written for a while and tried again on the next round.
    Done,
    /// Another wrote it first; read it again and redo the change.
    Stale,
    /// The server will not take it, saying why in words for the person.
    Refused(String),
}

/// Writes `body` over the vault's entry `id` at `revision`.
async fn write<T: Serialize>(
    core: &Core,
    shared: &Shared,
    kind: &str,
    id: &str,
    revision: i64,
    body: &T,
) -> Wrote {
    let Ok(value) = serde_json::to_value(body) else {
        return Wrote::Done;
    };
    match vault::put_project(core, &shared.org, kind, id, revision, &value, &[]).await {
        Ok(Put::Written(_)) => Wrote::Done,
        Ok(Put::Stale) => Wrote::Stale,
        Ok(Put::Refused(why)) => {
            eprintln!(
                "sikemux core: {}'s waiting rules refused a change: {why}",
                shared.name
            );
            Wrote::Refused(why)
        }
        Err(error) => {
            eprintln!(
                "sikemux core: {}'s waiting rules were not written: {error}",
                shared.name
            );
            Wrote::Done
        }
    }
}

/// Moving entry `id` of the project's own store to the org's, while its owners and admins approve
/// its rules, puts it on the org's waiting list; moving it back takes it off, whether or not they
/// still approve first. None for any other move.
pub(super) fn listed(
    state: &mut State,
    project: &str,
    from: &str,
    id: u64,
    to: Store,
) -> Option<Result<(), String>> {
    let local = state.key_of(project);
    let writes = state.shared.get(&local)?.writes;
    if from != local {
        return None;
    }
    let held = state.held.get_mut(&local)?;
    match to {
        Store::Org if writes => None,
        Store::Org if held.waiting.contains(&id) => {
            Some(Err(format!("#{id} waits for an owner or admin already")))
        }
        Store::Org => Some(wait(state, &local, id)),
        Store::Project if held.waiting.remove(&id) => Some(Ok(())),
        _ => None,
    }
}

/// Why a sensitive entry never waits for an admin: it goes up only by an admin's own move.
pub(super) const SENSITIVE: &str = "it's sensitive, so ask an owner or admin to add it";
/// The same, for a row marked sensitive, which says so already.
pub(super) const SENSITIVE_ROW: &str = "only an owner or admin can add it";

impl Waiting {
    pub(super) fn is_sensitive(sensitive: bool, text: &str) -> bool {
        sensitive || may_hold_secret(text)
    }

    /// Whether the entry, in its words and mark as they are now, may go to an org's admins.
    fn may_leave(&self) -> Result<(), String> {
        match self.sensitive() {
            true => Err(SENSITIVE.to_owned()),
            false => Ok(()),
        }
    }
}

/// Takes off the waiting list each entry that may no longer go to the org's admins, such as one
/// reworded into a credential while it waited, and marks it refused with why. True when one came
/// off.
pub(super) fn recheck(held: &mut Held) -> bool {
    let unfit: Vec<(u64, String)> = (held.waiting.iter())
        .filter_map(|id| Some((*id, Waiting::of(held, *id)?.may_leave().err()?)))
        .collect();
    for (id, why) in &unfit {
        held.waiting.remove(id);
        held.refused.insert(*id, why.clone());
    }
    !unfit.is_empty()
}

/// Puts entry `id` of the project's own store under `local` on the org's waiting list, unless it
/// is sensitive, the org keeps its words already or the list has no room for it. A sensitive
/// entry is marked refused, so its row says why.
pub(super) fn wait(state: &mut State, local: &str, id: u64) -> Result<(), String> {
    let shared = (state.shared.get(local)).ok_or("the project is no longer shared")?;
    let (org_key, org_name) = (shared.key(), shared.name.clone());
    let in_org = org_words(state, &org_key, |_| true);
    let held = (state.held.get_mut(local)).ok_or("the store went while it was being written to")?;
    let entry = Waiting::of(held, id);
    if let Some(Err(why)) = entry.as_ref().map(Waiting::may_leave) {
        held.refused.insert(id, why.clone());
        return Err(why);
    }
    let text = entry.map(|entry| entry.text().to_lowercase());
    if text.is_some_and(|text| in_org.contains(&text)) {
        return Err(format!("{org_name} keeps that already"));
    }
    if held.waiting.len() >= MAX_WAITING || list_too_big(held, id) {
        return Err(format!(
            "Too much waits for an owner or admin of {org_name} already"
        ));
    }
    held.waiting.insert(id);
    held.dropped.remove(&id);
    held.refused.remove(&id);
    held.not_now.remove(&id);
    Ok(())
}

/// The words of the entries the org's store under `org_key` holds, lowercased, of writers
/// `by` picks.
fn org_words(
    state: &State,
    org_key: &str,
    by: impl Fn(Option<&String>) -> bool,
) -> HashSet<String> {
    let Some(org) = state.held.get(org_key) else {
        return HashSet::new();
    };
    let writer = |id: &u64| by(org.writers.get(id));
    let rules = (org.rules.iter())
        .filter(|rule| writer(&rule.id))
        .map(|rule| &rule.text);
    let knowledge = (org.knowledge.iter())
        .filter(|known| writer(&known.id))
        .map(|known| &known.text);
    rules
        .chain(knowledge)
        .map(|text| text.to_lowercase())
        .collect()
}

/// Whether the member's list, with entry `id` waiting too, is past what the vault takes in one
/// entry.
fn list_too_big(held: &Held, id: u64) -> bool {
    let items: BTreeMap<String, Waiting> = (held.waiting.iter().chain([&id]))
        .filter_map(|id| Some((item_key(&held.key, *id), Waiting::of(held, *id)?)))
        .collect();
    serde_json::to_vec(&Mine { items }).map_or(0, |bytes| bytes.len()) > super::org::MAX_BODY
}

/// Brings the waiting list of the project whose own store is `local` in step: on the project's
/// home, what this host's person moved or took back and what an owner or admin decided of it;
/// for an owner or admin, what they dropped and a call for each entry still waiting.
pub(super) async fn sync(core: &Arc<Core>, local: &str, shared: &Shared) {
    let home = (core.memory().state.locked().held.get(local)).is_some_and(|held| !held.mirrored);
    if home && shared.writes {
        take_up(core, local);
    }
    if home {
        settle_mine(core, local, shared).await;
    }
    if vault::manages_org(core, &shared.org) {
        judge(core, local, shared).await;
    } else {
        withdraw_calls(core, local, shared);
    }
}

/// Closes the calls this host's person had about the org's waiting entries, once they no longer
/// own or run the org.
fn withdraw_calls(core: &Core, local: &str, shared: &Shared) {
    let project = {
        let state = core.memory().state.locked();
        let Some(folder) = state.folder_of(local) else {
            return;
        };
        folder.clone()
    };
    let prefix = format!("{ORG_WAITING}{}|", shared.key());
    for (id, _) in person_calls::open_keyed(core, &project, &prefix) {
        person_calls::set_outcome(core, &id, ended(CallResult::Withdrawn));
    }
}

/// Moves the entries still waiting here into the org's store, once its members may add to it
/// without an owner or admin.
fn take_up(core: &Core, local: &str) {
    let (rechecked, waiting) = {
        let memory = core.memory();
        let mut state = memory.state.locked();
        let Some(held) = state.held.get_mut(local) else {
            return;
        };
        let rechecked = recheck(held);
        if rechecked {
            memory.save(&mut state, local);
        }
        let project = state.folder_of(local);
        let waiting = (state
            .held
            .get(local)
            .filter(|held| !held.waiting.is_empty()))
        .zip(project)
        .map(|(held, project)| (project.clone(), held.waiting.clone()));
        (rechecked, waiting)
    };
    let Some((project, ids)) = waiting else {
        if rechecked {
            tell_clients(core, false);
        }
        return;
    };
    for id in ids {
        let moved = moving::move_to(core, &project, id, Store::Org, learned(core, None));
        let memory = core.memory();
        let mut state = memory.state.locked();
        if let Some(held) = state.held.get_mut(local) {
            held.waiting.remove(&id);
            if let Err(why) = moved {
                held.refused.insert(id, why);
            }
            memory.save(&mut state, local);
        }
    }
    tell_clients(core, false);
}

async fn settle_mine(core: &Core, local: &str, shared: &Shared) {
    let id = shared.waiting_id(&shared.member);
    for _ in 0..TRIES {
        let Some((revision, list)) = read::<Mine>(core, shared, &id) else {
            return;
        };
        let drops = read::<Drops>(core, shared, &shared.drops_id())
            .unwrap_or_default()
            .1;
        let mut now = list.clone();
        settle(core, local, shared, &drops, &mut now);
        if now == list {
            return;
        }
        match write(core, shared, PROJECT_MEMORY_WAITING, &id, revision, &now).await {
            Wrote::Done => return,
            Wrote::Stale => {}
            Wrote::Refused(why) => return not_listed(core, local, &list, &why),
        }
    }
}

/// Marks the entries waiting here that the member's list in the vault does not hold as refused,
/// after the server would not take the list with them.
fn not_listed(core: &Core, local: &str, list: &Mine, why: &str) {
    let memory = core.memory();
    let mut state = memory.state.locked();
    let Some(held) = state.held.get_mut(local) else {
        return;
    };
    let unlisted: Vec<u64> = (held.waiting.iter().copied())
        .filter(|id| !list.items.contains_key(&item_key(local, *id)))
        .collect();
    if unlisted.is_empty() {
        return;
    }
    for id in unlisted {
        held.waiting.remove(&id);
        held.refused.insert(id, why.to_owned());
    }
    memory.save(&mut state, local);
    drop(state);
    tell_clients(core, false);
}

/// Puts this host's waiting entries on the member's list as they are now, and takes in what an
/// owner or admin decided of them: an entry the org's store holds in its words under this
/// member's name leaves the project's own store, and a dropped one stays there.
fn settle(core: &Core, local: &str, shared: &Shared, drops: &Drops, list: &mut Mine) {
    let memory = core.memory();
    let mut state = memory.state.locked();
    let mine = |writer: Option<&String>| writer == Some(&shared.member);
    let in_org = org_words(&state, &shared.key(), mine);
    let theirs = org_words(&state, &shared.key(), |writer| !mine(writer));
    let Some(held) = state.held.get_mut(local) else {
        return;
    };
    let tag = format!("{}:", store_tag(local));
    let text = |held: &Held, id: u64| Waiting::of(held, id).map(|e| e.text().to_lowercase());
    let mut changed = recheck(held);
    let ids: Vec<u64> = (held.waiting.iter().chain(held.dropped.keys()).copied()).collect();
    for id in ids {
        let Some(text) = text(held, id) else {
            continue;
        };
        if in_org.contains(&text) {
            held.rules.retain(|rule| rule.id != id);
            held.knowledge.retain(|known| known.id != id);
            held.waiting.remove(&id);
            held.dropped.remove(&id);
            changed = true;
        } else if theirs.contains(&text) && held.waiting.remove(&id) {
            held.refused
                .insert(id, format!("{} keeps that already", shared.name));
            changed = true;
        }
    }
    let listed: Vec<String> = (list.items.keys())
        .filter(|key| key.starts_with(&tag))
        .cloned()
        .collect();
    for key in listed {
        let id = key.rsplit(':').next().and_then(|id| id.parse().ok());
        let entry = id
            .filter(|id| held.waiting.contains(id))
            .and_then(|id| Some((id, Waiting::of(held, id)?)));
        let Some((id, entry)) = entry else {
            list.items.remove(&key);
            continue;
        };
        let dropped = drops.items.get(&dropped_key(&shared.member, &key));
        match dropped.filter(|dropped| dropped.words == words(entry.text())) {
            Some(dropped) => {
                held.waiting.remove(&id);
                let by = (shared.members.get(&dropped.by).cloned().flatten()).unwrap_or_default();
                held.dropped.insert(id, by);
                list.items.remove(&key);
                changed = true;
            }
            None => drop(list.items.insert(key, entry)),
        }
    }
    for id in held.waiting.clone() {
        let key = item_key(local, id);
        if list.items.contains_key(&key) {
            continue;
        }
        match Waiting::of(held, id) {
            Some(entry) => drop(list.items.insert(key, entry)),
            None => {
                held.waiting.remove(&id);
                changed = true;
            }
        }
    }
    if changed {
        memory.save(&mut state, local);
        drop(state);
        tell_clients(core, false);
    }
}

/// Each member's list, by membership, as the vault vouches it is theirs.
fn lists(core: &Core, shared: &Shared) -> BTreeMap<String, Mine> {
    (shared.members.keys())
        .filter_map(|member| {
            let (_, list) = read::<Mine>(core, shared, &shared.waiting_id(member))?;
            Some((member.clone(), list))
        })
        .collect()
}

/// Writes what this host's person dropped, forgets drops whose entry left its member's list, and
/// asks this host's person about each entry still waiting.
async fn judge(core: &Arc<Core>, local: &str, shared: &Shared) {
    let id = shared.drops_id();
    for _ in 0..TRIES {
        let Some((revision, drops)) = read::<Drops>(core, shared, &id) else {
            return;
        };
        let lists = lists(core, shared);
        let mut now = drops.clone();
        record(core, shared, &lists, &mut now);
        ask(core, local, shared, &lists, &now);
        if now == drops {
            return;
        }
        if !matches!(
            write(core, shared, PROJECT_MEMORY_DROPS, &id, revision, &now).await,
            Wrote::Stale
        ) {
            return;
        }
    }
}

fn record(core: &Core, shared: &Shared, lists: &BTreeMap<String, Mine>, drops: &mut Drops) {
    let listed = |at: &str, shown: &str| {
        let (member, item) = at.split_once('|')?;
        let entry = lists.get(member)?.items.get(item)?;
        (words(entry.text()) == shown).then_some(())
    };
    let written = drops.clone();
    let memory = core.memory();
    let mut state = memory.state.locked();
    let store = shared.key();
    let Some(held) = state.held.get_mut(&store) else {
        return;
    };
    let before = held.drops.len();
    held.drops.retain(|at, shown| {
        if listed(at, shown).is_none()
            || (written.items.get(at)).is_some_and(|dropped| dropped.words == *shown)
        {
            return false;
        }
        drops.items.insert(
            at.clone(),
            Dropped {
                words: shown.clone(),
                by: shared.member.clone(),
            },
        );
        true
    });
    if held.drops.len() != before {
        memory.save(&mut state, &store);
    }
    drop(state);
    (drops.items).retain(|at, dropped| listed(at, &dropped.words).is_some());
}

/// Puts a call to this host's person, an owner or admin, for each entry waiting on a teammate's
/// list that nobody dropped and the org's store does not hold yet, and closes the calls of those
/// another took care of or their member took back.
fn ask(core: &Core, local: &str, shared: &Shared, lists: &BTreeMap<String, Mine>, drops: &Drops) {
    let (project, name, in_org) = {
        let state = core.memory().state.locked();
        let Some(folder) = state.folder_of(local) else {
            return;
        };
        let in_org: HashSet<String> = (state.held.get(&shared.key()))
            .map(|org| {
                let rules = org.rules.iter().map(|rule| &rule.text);
                (rules.chain(org.knowledge.iter().map(|known| &known.text)))
                    .map(|text| text.to_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        (folder.clone(), moving::project_name(&state, local), in_org)
    };
    let not_kept = core.memory().answers.locked().not_kept.clone();
    let waiting: BTreeMap<String, (&String, &Waiting)> = (lists.iter())
        .filter(|(member, _)| **member != shared.member)
        .flat_map(|(member, list)| {
            (list.items.iter()).map(move |(item, entry)| (member, item, entry))
        })
        .filter(|(member, item, entry)| {
            let dropped = drops.items.get(&dropped_key(member, item));
            entry.may_leave().is_ok()
                && !dropped.is_some_and(|dropped| dropped.words == words(entry.text()))
                && !in_org.contains(&entry.text().to_lowercase())
        })
        .map(|(member, item, entry)| (call_key(shared, member, item, entry), (member, entry)))
        .collect();
    let prefix = format!("{ORG_WAITING}{}|", shared.key());
    let open = person_calls::open_keyed(core, &project, &prefix);
    for (id, key) in &open {
        if !waiting.contains_key(key) {
            person_calls::set_outcome(core, id, ended(CallResult::Withdrawn));
        }
    }
    for (key, (member, entry)) in waiting {
        if open.iter().any(|(_, open)| *open == key) {
            continue;
        }
        let why = not_kept
            .get(&entry.text().to_lowercase())
            .map(String::as_str);
        let raise = call_for(project.clone(), &name, shared, key, member, entry, why);
        if let Err(problem) = person_calls::raise(core, raise) {
            eprintln!("sikemux core: a waiting rule was not put to the person: {problem}");
        }
    }
}

fn call_for(
    project: String,
    name: &str,
    shared: &Shared,
    key: String,
    member: &str,
    entry: &Waiting,
    not_kept: Option<&str>,
) -> Raise {
    let from = (shared.members.get(member).and_then(Option::as_deref)).unwrap_or("A member");
    let (what, them) = match entry {
        Waiting::Rule { .. } => ("rule", "follow it"),
        Waiting::Knowledge { .. } => ("note", "know it"),
    };
    let option = |id: &str, label: String, detail: String| CallOption {
        id: id.to_owned(),
        label,
        detail: Some(detail),
        picture: None,
    };
    Raise {
        project,
        kind: CallKind::Taste,
        asker: CallAsker {
            agent_id: String::new(),
            name: "Sikemux".to_owned(),
            cli: "sikemux".to_owned(),
            chief: false,
        },
        question: format!("Keep {from}'s {what} for everyone in {}?", shared.name),
        context: Some(match not_kept {
            Some(why) => format!(
                "\"{}\"\n\nKept, every member's agents on {name} {them}. Kept last time, {} would not take it: {why}",
                entry.text(),
                shared.name
            ),
            None => format!(
                "\"{}\"\n\nKept, every member's agents on {name} {them}.",
                entry.text()
            ),
        }),
        options: vec![
            option(
                KEEP,
                "Keep it".to_owned(),
                format!("In {}'s rules", shared.name),
            ),
            option(DROP, "Drop it".to_owned(), format!("It stays {from}'s own")),
        ],
        pick: None,
        against: None,
        evidence: Vec::new(),
        action: None,
        brief: None,
        place: None,
        key: Some(key),
        yes: None,
    }
}

fn ended(result: CallResult) -> CallOutcome {
    CallOutcome {
        result,
        healthy: None,
        roll_back: false,
        why: None,
        at: epoch_ms(),
    }
}

/// An entry an owner or admin here kept from a member's waiting list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Keep {
    member: String,
    item: String,
    /// The hash of the words the owner or admin read.
    words: String,
    /// The call the owner or admin answered.
    call: String,
}

/// The Keeps [`check_keeps`] took back.
pub(super) struct Voided {
    pub(super) any: bool,
    /// The calls of those whose member took the entry back.
    pub(super) withdrawn: Vec<String>,
    /// Each waiting list the Keeps still standing were checked against, at the revision read.
    pub(super) read: Vec<(String, i64)>,
}

impl Voided {
    /// Marks the calls of the entries their member took back as withdrawn.
    pub(super) fn withdraw(&self, core: &Core) {
        for call in &self.withdrawn {
            person_calls::set_outcome(core, call, ended(CallResult::Withdrawn));
        }
    }
}

/// Takes out of the org's store each entry kept here that its member's list no longer holds in
/// the words the owner or admin read, so only those still waiting are written. None while this
/// host holds no key to the org's vault.
pub(super) fn check_keeps(
    core: &Core,
    held: &mut Held,
    shared: &Shared,
    written: &dyn Fn(u64) -> bool,
) -> Option<Voided> {
    if held.keeps.is_empty() {
        return Some(Voided {
            any: false,
            withdrawn: Vec::new(),
            read: Vec::new(),
        });
    }
    let (mut void, mut withdrawn, mut read_at) = (Vec::new(), Vec::new(), Vec::new());
    for keep in &held.keeps {
        let id = shared.waiting_id(&keep.member);
        let (revision, list) = read::<Mine>(core, shared, &id)?;
        match list.items.get(&keep.item).map(|entry| words(entry.text())) {
            Some(still) if still == keep.words => {
                read_at.push((id, revision));
                continue;
            }
            Some(_) => {}
            None => withdrawn.push(keep.call.clone()),
        }
        void.push(keep.clone());
    }
    held.keeps.retain(|keep| !void.contains(keep));
    let voided = |id: u64, text: &str| {
        !written(id)
            && void.iter().any(|keep| {
                keep.words == words(text) && held.writers.get(&id) == Some(&keep.member)
            })
    };
    let ids: Vec<u64> = (held.rules.iter().map(|rule| (rule.id, rule.text.as_str())))
        .chain(
            held.knowledge
                .iter()
                .map(|known| (known.id, known.text.as_str())),
        )
        .filter(|(id, text)| voided(*id, text))
        .map(|(id, _)| id)
        .collect();
    held.rules.retain(|rule| !ids.contains(&rule.id));
    held.knowledge.retain(|known| !ids.contains(&known.id));
    held.writers.retain(|id, _| !ids.contains(id));
    read_at.sort();
    read_at.dedup();
    Some(Voided {
        any: !void.is_empty(),
        withdrawn,
        read: read_at,
    })
}

/// The person kept or dropped an entry waiting on a teammate's list: a kept one goes into the
/// org's store under that member's name, and a dropped one onto the org's drops at the next sync.
pub(crate) fn answered(core: &Arc<Core>, call: &PersonCall, key: &str) {
    person_calls::set_outcome(core, &call.id, ended(CallResult::Heard));
    let Some((store, member, item, shown)) = parse(key) else {
        return;
    };
    let picked = call
        .answer
        .as_ref()
        .and_then(|answer| answer.option.as_deref());
    let memory = core.memory();
    let mut state = memory.state.locked();
    let Some(entry) = waiting_entry(core, &state, store, member, item) else {
        return;
    };
    if words(entry.text()) != shown {
        return;
    }
    match picked {
        Some(KEEP) => {
            let from = Keep {
                member: member.to_owned(),
                item: item.to_owned(),
                words: shown.to_owned(),
                call: call.id.clone(),
            };
            if let Err(problem) = keep(&mut state, store, from, entry) {
                eprintln!("sikemux core: a waiting rule was not kept: {problem}");
                return;
            }
            memory.save(&mut state, store);
            drop(state);
            tell_clients(core, false);
        }
        Some(DROP) => {
            let Some(held) = state.held.get_mut(store) else {
                return;
            };
            (held.drops).insert(format!("{member}|{item}"), shown.to_owned());
            memory.save(&mut state, store);
        }
        _ => return,
    }
    memory.org_sync.notify_one();
}

/// The org's store would not take an entry kept here for a teammate: it stays on their list, and
/// the next call about it says why.
pub(super) fn not_kept(core: &Core, text: &str, why: &str) {
    (core.memory().answers.locked().not_kept).insert(text.to_lowercase(), why.to_owned());
}

/// The entry still waiting under `item` on `member`'s list for the org's store `store`.
fn waiting_entry(
    core: &Core,
    state: &State,
    store: &str,
    member: &str,
    item: &str,
) -> Option<Waiting> {
    let shared = (state.shared.values())
        .find(|shared| shared.key() == store)
        .filter(|shared| vault::manages_org(core, &shared.org))?;
    let (_, mut list) = read::<Mine>(core, shared, &shared.waiting_id(member))?;
    list.items.remove(item)
}

fn keep(state: &mut State, store: &str, from: Keep, entry: Waiting) -> Result<(), String> {
    let member = from.member.as_str();
    let held = holding(state, store, Store::Org)?;
    let same = |text: &str| text.eq_ignore_ascii_case(entry.text());
    if held.rules.iter().any(|rule| same(&rule.text))
        || held.knowledge.iter().any(|known| same(&known.text))
    {
        return Ok(());
    }
    if held.len() >= MAX_ENTRIES {
        return Err(format!(
            "the org's store already keeps {MAX_ENTRIES} entries"
        ));
    }
    let id = state.next_id(store);
    let held = state.held.get_mut(store).ok_or("the org's store went")?;
    match entry {
        Waiting::Rule { mut rule } => {
            rule.id = id;
            rule.moved = None;
            held.rules.push(*rule);
        }
        Waiting::Knowledge { mut knowledge } => {
            knowledge.id = id;
            knowledge.moved = None;
            held.knowledge.push(*knowledge);
        }
    }
    held.writers.insert(id, member.to_owned());
    if let Some(full) = super::org::too_big(held) {
        held.rules.retain(|rule| rule.id != id);
        held.knowledge.retain(|known| known.id != id);
        held.writers.remove(&id);
        return Err(full);
    }
    held.keeps.push(from);
    Ok(())
}
