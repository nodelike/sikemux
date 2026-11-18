//! The rules and knowledge of a project shared with an org, kept in the org's vault so its
//! members' agents follow them too. Each core mirrors the vault's entry into a store of its own
//! and writes its person's changes back over the revision it last read, so a write that crossed
//! another member's is redone on top of theirs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::memory::{Knowledge, MemoryRule, Moved, Stale, Store};

use super::{holding, holding_or_personal, tell_clients, Held, State};
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::vault::org::{self as vault, Put, PROJECT_MEMORY, PROJECT_MEMORY_DROPS};
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

/// The most the vault's entry of a project's store may hold, under the server's limit.
pub(super) const MAX_BODY: usize = 60 * 1024;
const TRIES: usize = 3;

/// A project open here that this account's person shares with an org.
#[derive(Clone, PartialEq)]
pub(super) struct Shared {
    pub(super) org: String,
    /// The org's name, as its members see it.
    pub(super) name: String,
    /// What the account server knows the project by.
    project: String,
    /// This account's membership in the org.
    pub(super) member: String,
    /// Whether the org's plan is paid up.
    pub(super) active: bool,
    /// Whether this account's person adds to the store without an admin's approval.
    pub(super) writes: bool,
    /// The first name of each of the org's members, by membership, as the org's list has them.
    pub(super) names: BTreeMap<String, String>,
}

impl Shared {
    pub(super) fn key(&self) -> String {
        format!("org:{}:{}", self.org, self.project)
    }

    fn entry_id(&self) -> String {
        vault::project_entry_id(PROJECT_MEMORY, &self.project)
    }

    /// Where the vault keeps the entries `member` has waiting for the project.
    pub(super) fn waiting_id(&self, member: &str) -> String {
        vault::waiting_entry_id(&self.project, member)
    }

    pub(super) fn drops_id(&self) -> String {
        vault::project_entry_id(PROJECT_MEMORY_DROPS, &self.project)
    }
}

/// The project's own store and this account's membership an org's store was kept for.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct KeptFor {
    pub(super) local: String,
    member: String,
}

/// What the vault's entry of a project's store holds.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Body {
    next: u64,
    rules: Vec<MemoryRule>,
    knowledge: Vec<Knowledge>,
    /// The member who put each entry there.
    #[serde(default)]
    writers: BTreeMap<u64, String>,
}

impl Body {
    fn of(held: &Held) -> Self {
        Self {
            next: held.next,
            rules: held.rules.iter().map(unnamed).collect(),
            knowledge: held.knowledge.iter().map(as_shared).collect(),
            writers: held.writers.clone(),
        }
    }

    fn shared_part(mut self) -> Self {
        self.rules = self.rules.iter().map(unnamed).collect();
        self.knowledge = self.knowledge.iter().map(as_shared).collect();
        self
    }

    fn same_entries(&self, other: &Body) -> bool {
        self.rules == other.rules
            && self.knowledge == other.knowledge
            && self.writers == other.writers
    }

    fn entries(&self) -> BTreeMap<u64, Entry> {
        (self
            .rules
            .iter()
            .map(|rule| (rule.id, Entry::Rule(Box::new(rule.clone())))))
        .chain(
            (self.knowledge.iter())
                .map(|known| (known.id, Entry::Knowledge(Box::new(known.clone())))),
        )
        .collect()
    }
}

/// A rule as the org's store keeps it, naming nobody: a row names its writer from the org's
/// member list, so a member who leaves or deletes their account is named on none.
pub(super) fn unnamed(rule: &MemoryRule) -> MemoryRule {
    let mut rule = rule.clone();
    rule.learned.person = None;
    if let Some(moved) = &mut rule.moved {
        moved.by.person = None;
    }
    rule
}

/// Knowledge as the org's store keeps it, naming nobody, without what testing it found on one
/// host.
pub(super) fn as_shared(known: &Knowledge) -> Knowledge {
    let mut known = known.clone();
    known.learned.person = None;
    if let Some(moved) = &mut known.moved {
        moved.by.person = None;
    }
    known.checked = None;
    if matches!(known.stale, Some(Stale::Failed { .. })) {
        known.stale = None;
    }
    known
}

/// `knowledge` from the org's store, with what testing it found on this host as `had` holds it.
fn with_local_checks(knowledge: Vec<Knowledge>, had: &[Knowledge]) -> Vec<Knowledge> {
    (knowledge.into_iter())
        .map(|mut known| {
            let tested = had.iter().find(|was| {
                was.id == known.id && was.text == known.text && was.check == known.check
            });
            if let Some(tested) = tested {
                known.checked = tested.checked.clone();
                if known.stale.is_none() {
                    known.stale = (tested.stale.clone())
                        .filter(|stale| matches!(stale, Stale::Failed { .. }));
                }
            }
            known
        })
        .collect()
}

/// The vault's entry as this core last read it, which its own changes are made against.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Base {
    revision: i64,
    body: Body,
}

#[derive(Clone, PartialEq)]
enum Entry {
    Rule(Box<MemoryRule>),
    Knowledge(Box<Knowledge>),
}

impl Entry {
    fn same_words(&self, other: &Entry) -> bool {
        match (self, other) {
            (Entry::Rule(one), Entry::Rule(two)) => one.text.eq_ignore_ascii_case(&two.text),
            (Entry::Knowledge(one), Entry::Knowledge(two)) => {
                one.text.eq_ignore_ascii_case(&two.text)
            }
            _ => false,
        }
    }

    fn with_id(self, id: u64) -> Self {
        match self {
            Entry::Rule(mut rule) => {
                rule.id = id;
                Entry::Rule(rule)
            }
            Entry::Knowledge(mut known) => {
                known.id = id;
                Entry::Knowledge(known)
            }
        }
    }
}

/// Names `member` the writer of each entry of an org's store that names none yet, as one added
/// here, and forgets the writers of entries that are gone.
pub(super) fn own_unwritten(held: &mut Held, member: &str) {
    let ids: Vec<u64> = (held.rules.iter().map(|rule| rule.id))
        .chain(held.knowledge.iter().map(|known| known.id))
        .collect();
    held.writers.retain(|id, _| ids.contains(id));
    for id in ids {
        held.writers.entry(id).or_insert_with(|| member.to_owned());
    }
}

/// `mine`'s changes since `base`, made again on top of `remote`. An entry added here under an id
/// another member's core gave an entry of its own meanwhile takes a fresh id from `fresh`, and one
/// another member's core added in the same words meanwhile is not added again.
fn rebased(remote: &Body, base: &Body, mine: &Body, mut fresh: impl FnMut() -> u64) -> Body {
    let (before, now) = (base.entries(), mine.entries());
    let mut entries = remote.entries();
    let mut writers = remote.writers.clone();
    for id in before.keys().filter(|id| !now.contains_key(id)) {
        entries.remove(id);
        writers.remove(id);
    }
    for (id, entry) in now
        .iter()
        .filter(|(id, entry)| before.get(id) != Some(entry))
    {
        let added = !before.contains_key(id);
        if added && (entries.values()).any(|there| there != entry && there.same_words(entry)) {
            continue;
        }
        let taken = added && entries.get(id).is_some_and(|there| there != entry);
        let at = if taken { fresh() } else { *id };
        entries.insert(at, entry.clone().with_id(at));
        match mine.writers.get(id) {
            Some(writer) => writers.insert(at, writer.clone()),
            None => writers.remove(&at),
        };
    }
    let next = (entries.keys().max().map_or(1, |id| id + 1))
        .max(remote.next)
        .max(mine.next);
    let (mut rules, mut knowledge) = (Vec::new(), Vec::new());
    for entry in entries.into_values() {
        match entry {
            Entry::Rule(rule) => rules.push(*rule),
            Entry::Knowledge(known) => knowledge.push(*known),
        }
    }
    Body {
        next,
        rules,
        knowledge,
        writers,
    }
}

/// Keeps each shared project's store in step with the org's vault: when the vault changes, when
/// a store is saved here, and each minute.
pub(super) async fn serve(core: Arc<Core>) {
    loop {
        sync(&core).await;
        let changed = core.vault().orgs().changed.clone();
        let saved = core.memory().org_sync.clone();
        tokio::select! {
            () = changed.notified() => {}
            () = saved.notified() => {}
            () = tokio::time::sleep(Duration::from_secs(60)) => {}
        }
    }
}

async fn sync(core: &Arc<Core>) {
    let Some(Sharing {
        shared,
        shut,
        unheard,
        orgs,
    }) = sharing(core).await
    else {
        return;
    };
    {
        let memory = core.memory();
        let mut state = memory.state.locked();
        end(core, &mut state, &shared, &unheard);
        let reshut = state.shut != shut;
        super::waiting::follow_shares(core, &mut state, &orgs);
        state.shared = shared.clone();
        state.shut = shut;
        if reshut {
            drop(state);
            tell_clients(core, false);
        }
    }
    for (local, shared) in &shared {
        mirror(core, shared);
        if shared.writes {
            push(core, shared).await;
        } else if unwritten(core, shared) {
            let why = if shared.active {
                format!("{}'s owners and admins approve its rules now", shared.name)
            } else {
                format!(
                    "{}'s plan has ended, so its rules can't change",
                    shared.name
                )
            };
            give_back(core, shared, &why);
        }
        super::waiting::sync(core, local, shared).await;
    }
}

/// The projects open here that this account's person shares with one org, by their store's key.
struct Sharing {
    shared: HashMap<String, Shared>,
    /// Those whose org's vault this host cannot open, with the org's name.
    shut: HashMap<String, String>,
    /// The orgs whose vault this host holds no key to and has not heard from yet, whose projects
    /// are in neither map.
    unheard: HashSet<String>,
    /// The org each project open here is shared with, by its store's key, whether or not this
    /// host can open its vault.
    orgs: HashMap<String, String>,
}

/// None until this host has heard the registry and the account's orgs.
async fn sharing(core: &Arc<Core>) -> Option<Sharing> {
    let registry = crate::server::account::shares::read(core).await?;
    let account = core.account().shown()?;
    let orgs = sikemux_corekit::accounts::orgs(Some(&account));
    for list in &registry.orgs {
        let shared = !list.shares.is_empty() || !list.mine.is_empty();
        let active = orgs.iter().any(|org| org.id == list.org && org.active);
        if shared && active && vault::manages_org(core, &list.org) {
            if let Err(error) = vault::make(core, &list.org).await {
                eprintln!("sikemux core: an org's vault could not be made: {error}");
            }
        }
    }
    let (mut shared, mut shut, mut unheard) = (HashMap::new(), HashMap::new(), HashSet::new());
    let mut shared_orgs = HashMap::new();
    for identity in (core.workspaces().projects().into_iter()).filter_map(|known| known.identity) {
        let project = crate::server::shares::key(&identity);
        let mut mine = (registry.orgs.iter())
            .filter(|list| list.mine.iter().any(|share| share.project == project));
        let (Some(list), None) = (mine.next(), mine.next()) else {
            continue;
        };
        shared_orgs.insert(identity.clone(), list.org.clone());
        let (Some(me), Some(org)) = (
            list.members.iter().find(|member| member.you),
            orgs.iter().find(|org| org.id == list.org),
        ) else {
            continue;
        };
        match vault::holds_key(core, &list.org) {
            Some(true) => {}
            Some(false) => {
                shut.insert(identity, org.name.clone());
                continue;
            }
            None => {
                unheard.insert(list.org.clone());
                continue;
            }
        }
        let manages = vault::manages_org(core, &list.org);
        let approves = crate::server::account::org_policy(core, &list.org)
            .await
            .admins_approve_rules;
        let writes = org.active && (manages || !approves);
        shared.insert(
            identity,
            Shared {
                org: list.org.clone(),
                name: org.name.clone(),
                project,
                member: me.id.clone(),
                active: org.active,
                writes,
                names: (list.members.iter())
                    .filter_map(|member| {
                        let first = member.name.split_whitespace().next()?;
                        Some((member.id.clone(), first.to_owned()))
                    })
                    .collect(),
            },
        );
    }
    Some(Sharing {
        shared,
        shut,
        unheard,
        orgs: shared_orgs,
    })
}

/// Ends each org's store kept here that no project open here is shared with any more, leaving
/// those of `unheard` orgs alone: what this account's person put there comes back to the project's
/// own store, and the rest goes.
fn end(core: &Core, state: &mut State, now: &HashMap<String, Shared>, unheard: &HashSet<String>) {
    let open: HashSet<String> = now.values().map(Shared::key).collect();
    let waits = |key: &str| {
        (key.strip_prefix("org:")
            .and_then(|rest| rest.split(':').next()))
        .is_some_and(|org| unheard.contains(org))
    };
    let was = |key: &str| {
        (state.shared.iter())
            .find(|(_, was)| was.key() == key)
            .map(|(local, was)| KeptFor {
                local: local.clone(),
                member: was.member.clone(),
            })
    };
    let ended: Vec<(String, Option<KeptFor>)> = (state.held.values())
        .filter(|held| held.store == Store::Org && !open.contains(&held.key) && !waits(&held.key))
        .map(|held| {
            (
                held.key.clone(),
                held.kept_for.clone().or_else(|| was(&held.key)),
            )
        })
        .collect();
    let told = !ended.is_empty();
    for (key, kept_for) in ended {
        let Some(org) = state.held.remove(&key) else {
            continue;
        };
        if let Some(KeptFor { local, member }) = kept_for {
            let theirs = |id: u64| org.writers.get(&id) == Some(&member);
            let own = holding_or_personal(state, &local, Store::Project);
            let to = own.key.clone();
            own.next = own.next.max(org.next);
            for rule in org.rules.into_iter().filter(|rule| theirs(rule.id)) {
                if !(own.rules.iter()).any(|kept| kept.text.eq_ignore_ascii_case(&rule.text)) {
                    own.rules.push(rule);
                }
            }
            for known in org.knowledge.into_iter().filter(|known| theirs(known.id)) {
                if !(own.knowledge.iter()).any(|kept| kept.text.eq_ignore_ascii_case(&known.text)) {
                    own.knowledge.push(known);
                }
            }
            core.memory().save(state, &to);
        }
        core.memory().forget(state, &key);
    }
    if told || state.shared != *now {
        tell_clients(core, false);
    }
}

/// Brings the vault's entry into the project's org store here, keeping the changes made here
/// since it was last read.
fn mirror(core: &Core, shared: &Shared) {
    let Some(read) = vault::read(core, &shared.org, &shared.entry_id()) else {
        return;
    };
    let remote = match read {
        Some(read) => match serde_json::from_value::<Body>(read.body) {
            Ok(body) => Base {
                revision: read.revision,
                body: body.shared_part(),
            },
            Err(error) => {
                eprintln!("sikemux core: an org's store of rules could not be read: {error}");
                return;
            }
        },
        None => Base::default(),
    };
    let memory = core.memory();
    let mut state = memory.state.locked();
    let key = shared.key();
    let kept_for = local_key(&state, shared).map(|local| KeptFor {
        local,
        member: shared.member.clone(),
    });
    let Ok(held) = holding(&mut state, &key, Store::Org) else {
        return;
    };
    let refollowed = kept_for.is_some() && held.kept_for != kept_for;
    if refollowed {
        held.kept_for = kept_for;
    }
    if (held.base.as_ref()).is_some_and(|base| base.revision == remote.revision) {
        if refollowed {
            memory.save(&mut state, &key);
        }
        return;
    }
    let base = held.base.take().unwrap_or_default();
    let mine = Body::of(held);
    held.next = held.next.max(remote.body.next);
    let body = rebased(&remote.body, &base.body, &mine, || state.next_id(&key));
    let Some(held) = state.held.get_mut(&key) else {
        return;
    };
    held.next = body.next.max(held.next);
    held.rules = body.rules;
    held.knowledge = with_local_checks(body.knowledge, &held.knowledge);
    held.writers = body.writers;
    held.base = Some(remote);
    let mut touched = renumber_clashes(&mut state, shared);
    touched.extend(drop_copies(&mut state, shared));
    touched.sort();
    touched.dedup();
    memory.save(&mut state, &key);
    for local in touched {
        memory.save(&mut state, &local);
    }
    drop(state);
    tell_clients(core, false);
}

/// Gives the entries of the project's own store and the person's that share an id with one of
/// the org's store ids of their own, so every id applying to the project names one entry.
fn renumber_clashes(state: &mut State, shared: &Shared) -> Vec<String> {
    let Some(org) = state.held.get(&shared.key()) else {
        return Vec::new();
    };
    let taken: Vec<u64> = (org.rules.iter().map(|rule| rule.id))
        .chain(org.knowledge.iter().map(|known| known.id))
        .collect();
    let local = local_key(state, shared);
    let mut renumbered = Vec::new();
    for key in local.into_iter().chain([super::PERSONAL.to_owned()]) {
        let clashing: Vec<u64> = (state.held.get(&key).into_iter())
            .flat_map(|held| {
                (held.rules.iter().map(|rule| rule.id))
                    .chain(held.knowledge.iter().map(|known| known.id))
            })
            .filter(|id| taken.contains(id))
            .collect();
        for id in clashing {
            let fresh = state.next_id(&key);
            let Some(held) = state.held.get_mut(&key) else {
                continue;
            };
            for rule in held.rules.iter_mut().filter(|rule| rule.id == id) {
                rule.id = fresh;
            }
            for known in held.knowledge.iter_mut().filter(|known| known.id == id) {
                known.id = fresh;
            }
            if held.not_now.remove(&id) {
                held.not_now.insert(fresh);
            }
            if held.waiting.remove(&id) {
                held.waiting.insert(fresh);
            }
            if let Some(by) = held.dropped.remove(&id) {
                held.dropped.insert(fresh, by);
            }
            if let Some(why) = held.refused.remove(&id) {
                held.refused.insert(fresh, why);
            }
            renumbered.push(key.clone());
        }
    }
    renumbered.dedup();
    renumbered
}

/// Drops the entries of the project's own store whose words the org's store holds as this
/// account's person put them there, so a store that ended and came back is not followed twice.
fn drop_copies(state: &mut State, shared: &Shared) -> Option<String> {
    let org = state.held.get(&shared.key())?;
    let mine = |id: u64| org.writers.get(&id) == Some(&shared.member);
    let rules: Vec<String> = (org.rules.iter())
        .filter(|rule| mine(rule.id))
        .map(|rule| rule.text.to_lowercase())
        .collect();
    let knowledge: Vec<String> = (org.knowledge.iter())
        .filter(|known| mine(known.id))
        .map(|known| known.text.to_lowercase())
        .collect();
    let local = local_key(state, shared)?;
    let own = state.held.get_mut(&local)?;
    let before = own.len();
    let waiting = own.waiting.clone();
    own.rules
        .retain(|rule| waiting.contains(&rule.id) || !rules.contains(&rule.text.to_lowercase()));
    own.knowledge.retain(|known| {
        waiting.contains(&known.id) || !knowledge.contains(&known.text.to_lowercase())
    });
    (own.len() != before).then_some(local)
}

/// The key of the project's own store on this host.
fn local_key(state: &State, shared: &Shared) -> Option<String> {
    (state.keys.values())
        .find(|key| (state.shared.get(*key)).is_some_and(|is| is.key() == shared.key()))
        .cloned()
}

/// Whether this host's copy of the org's store holds changes the vault does not have yet.
fn unwritten(core: &Core, shared: &Shared) -> bool {
    let state = core.memory().state.locked();
    (state.held.get(&shared.key())).is_some_and(|held| {
        (held.base.as_ref()).is_some_and(|base| {
            let mine = Body::of(held);
            mine.rules != base.body.rules || mine.knowledge != base.body.knowledge
        })
    })
}

/// Puts the org's store back as the vault last gave it, after the vault refused a change for
/// `why`. What was added or moved here goes back to where it came from, and what was changed or
/// retired here shows as it was, each saying why.
fn give_back(core: &Core, shared: &Shared, why: &str) {
    let memory = core.memory();
    let mut state = memory.state.locked();
    let key = shared.key();
    let Some(held) = state.held.get_mut(&key) else {
        return;
    };
    let Some(base) = held.base.clone() else {
        return;
    };
    let kept = |id: u64| {
        (base.body.rules.iter().any(|rule| rule.id == id))
            || (base.body.knowledge.iter().any(|known| known.id == id))
    };
    let undone: Vec<u64> = (base.body.rules.iter())
        .filter(|rule| !held.rules.iter().any(|mine| unnamed(mine) == **rule))
        .map(|rule| rule.id)
        .chain(
            (base.body.knowledge.iter())
                .filter(|known| !held.knowledge.iter().any(|mine| as_shared(mine) == **known))
                .map(|known| known.id),
        )
        .collect();
    let writers = std::mem::take(&mut held.writers);
    let teammates = |id: u64| writers.get(&id).is_some_and(|by| *by != shared.member);
    let (theirs, added): (Vec<MemoryRule>, Vec<MemoryRule>) = (held.rules.drain(..))
        .filter(|rule| !kept(rule.id))
        .partition(|rule| teammates(rule.id));
    let had = std::mem::take(&mut held.knowledge);
    let (theirs_known, added_knowledge): (Vec<Knowledge>, Vec<Knowledge>) = (had.iter())
        .filter(|known| !kept(known.id))
        .cloned()
        .partition(|known| teammates(known.id));
    held.rules = base.body.rules;
    held.knowledge = with_local_checks(base.body.knowledge, &had);
    held.writers = base.body.writers;
    held.keeps.clear();
    for id in undone {
        (held.refused).insert(id, format!("{} keeps it as it was: {why}", shared.name));
    }
    let local = local_key(&state, shared);
    let home = |state: &State, moved: &Option<Moved>| {
        (moved.as_ref())
            .map(|moved| moved.from.clone())
            .filter(|from| *from != key && state.held.contains_key(from))
            .or_else(|| local.clone())
            .unwrap_or_else(|| super::PERSONAL.to_owned())
    };
    let texts = (theirs.iter().map(|rule| &rule.text)).chain(theirs_known.iter().map(|k| &k.text));
    for text in texts {
        super::waiting::not_kept(core, text, why);
    }
    let mut touched = vec![key.clone()];
    for mut rule in added {
        let to = home(&state, &rule.moved);
        rule.moved = None;
        let held = holding_or_personal(&mut state, &to, Store::Project);
        held.refused.insert(rule.id, why.to_owned());
        held.rules.push(rule);
        touched.push(held.key.clone());
    }
    for mut known in added_knowledge {
        let to = home(&state, &known.moved);
        known.moved = None;
        let held = holding_or_personal(&mut state, &to, Store::Project);
        held.refused.insert(known.id, why.to_owned());
        held.knowledge.push(known);
        touched.push(held.key.clone());
    }
    touched.sort();
    touched.dedup();
    for key in &touched {
        memory.save(&mut state, key);
    }
    drop(state);
    tell_clients(core, false);
}

/// Writes the changes made here to the org's store into its vault.
async fn push(core: &Arc<Core>, shared: &Shared) {
    let key = shared.key();
    for _ in 0..TRIES {
        let (revision, body, checked) = {
            let memory = core.memory();
            let mut state = memory.state.locked();
            let Some(held) = state.held.get_mut(&key) else {
                return;
            };
            let Some(base) = held.base.clone() else {
                return;
            };
            let written = |id: u64| {
                (base.body.rules.iter().any(|rule| rule.id == id))
                    || (base.body.knowledge.iter().any(|known| known.id == id))
            };
            let Some(voided) = super::waiting::check_keeps(core, held, shared, &written) else {
                return;
            };
            let body = Body::of(held);
            let checked = held.keeps.clone();
            if voided.any {
                memory.save(&mut state, &key);
                drop(state);
                voided.withdraw(core);
                tell_clients(core, false);
            }
            if body.same_entries(&base.body) {
                return;
            }
            (base.revision, body, checked)
        };
        let Ok(value) = serde_json::to_value(&body) else {
            return;
        };
        match vault::put_project(
            core,
            &shared.org,
            PROJECT_MEMORY,
            &shared.entry_id(),
            revision,
            &value,
        )
        .await
        {
            Ok(Put::Written(revision)) => {
                let memory = core.memory();
                let mut state = memory.state.locked();
                if let Some(held) = state.held.get_mut(&key) {
                    held.base = Some(Base { revision, body });
                    held.refused.clear();
                    held.keeps.retain(|keep| !checked.contains(keep));
                }
                memory.save(&mut state, &key);
                return;
            }
            Ok(Put::Stale) => mirror(core, shared),
            Ok(Put::Refused(why)) => {
                eprintln!(
                    "sikemux core: {}'s store of rules refused a change: {why}",
                    shared.name
                );
                give_back(core, shared, &why);
                return;
            }
            Err(error) => {
                eprintln!(
                    "sikemux core: {}'s store of rules was not written: {error}",
                    shared.name
                );
                return;
            }
        }
    }
}

/// The store an entry for `project` goes to: a rule the person typed into the Playbook goes to the
/// org's, while the project is shared with one that lets them add to it.
pub(super) fn persons(state: &State, project: &str, store: Store, typed_rule: bool) -> Store {
    let writes = (state.shared.get(&state.key_of(project))).is_some_and(|shared| shared.writes);
    match store {
        Store::Project if typed_rule && writes => Store::Org,
        store => store,
    }
}

/// Why the store under `key` cannot take what it holds now: the org's vault would refuse it.
pub(super) fn too_big(held: &Held) -> Option<String> {
    let size = serde_json::to_vec(&Body::of(held)).map_or(0, |bytes| bytes.len());
    (held.store == Store::Org && size > MAX_BODY)
        .then(|| "The org's store of rules for this project is full; retire some first".to_owned())
}

/// Why `by` may not change or move entry `id`, kept in the org's store of `project`.
pub(super) fn refused(state: &State, project: &str, key: &str, by_agent: bool) -> Option<String> {
    let shared = state.shared.get(&state.key_of(project))?;
    if shared.key() != key {
        return None;
    }
    if by_agent {
        return Some(format!(
            "That is {}'s rule; only a person changes it, from the Playbook",
            shared.name
        ));
    }
    (!shared.writes).then(|| {
        format!(
            "{}'s owners and admins approve its rules; ask one of them to change it",
            shared.name
        )
    })
}
