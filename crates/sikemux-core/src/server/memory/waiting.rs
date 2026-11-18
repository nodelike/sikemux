//! The org's waiting list: entries members moved to a shared project's org store while its owners
//! and admins approve its rules. A member's entry stays in their project's own store, marked
//! waiting, until an owner or admin keeps it in the org's store or drops it; each owner and admin
//! is asked by a call.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallAsker, CallKind, CallOption, CallOutcome, CallResult, PersonCall};
use sikemux_view::memory::{Knowledge, MemoryRule, Store};

use super::org::Shared;
use super::{holding, moving, tell_clients, Held, State, MAX_ENTRIES};
use crate::server::parts::Parts;
use crate::server::person_calls::{self, Raise, ORG_WAITING};
use crate::server::vault::org::{self as vault, Put, PROJECT_MEMORY_WAITING};
use crate::server::Core;

const TRIES: usize = 3;
const KEEP: &str = "keep";
const DROP: &str = "drop";

/// What the vault's waiting list of a project holds, by member and the entry's id in their store.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct List {
    #[serde(default)]
    items: BTreeMap<String, Item>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    member: String,
    /// The member's first name.
    from: Option<String>,
    entry: Waiting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decided: Option<Decided>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Waiting {
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

    fn of(held: &Held, id: u64) -> Option<Self> {
        let rule = (held.rules.iter().find(|rule| rule.id == id)).map(|rule| Waiting::Rule {
            rule: Box::new(rule.clone()),
        });
        rule.or_else(|| {
            (held.knowledge.iter().find(|known| known.id == id)).map(|known| Waiting::Knowledge {
                knowledge: Box::new(known.clone()),
            })
        })
    }
}

/// An owner's or admin's answer to an entry on the list.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum Decided {
    Kept,
    Dropped { by: Option<String> },
}

fn item_key(member: &str, id: u64) -> String {
    format!("{member}:{id}")
}

fn call_key(shared: &Shared, item: &str, entry: &Waiting) -> String {
    use sha2::{Digest, Sha256};
    let words = Sha256::digest(entry.text().as_bytes());
    let words = hex::encode(words.into_iter().take(6).collect::<Vec<u8>>());
    format!("{ORG_WAITING}{}|{item}|{words}", shared.key())
}

/// The org's store and the waiting entry it was asked about, from a call's key.
fn parse(key: &str) -> Option<(&str, &str)> {
    let mut parts = key.strip_prefix(ORG_WAITING)?.split('|');
    Some((parts.next()?, parts.next()?))
}

/// Moving entry `id` of the project's own store to the org's, while its owners and admins approve
/// its rules, puts it on the org's waiting list; moving it back takes it off. None for any other
/// move.
pub(super) fn listed(
    state: &mut State,
    project: &str,
    from: &str,
    id: u64,
    to: Store,
) -> Option<Result<(), String>> {
    let local = state.key_of(project);
    let shared = state.shared.get(&local)?;
    if from != local || shared.writes {
        return None;
    }
    let held = state.held.get_mut(&local)?;
    match to {
        Store::Org if held.waiting.contains(&id) => {
            Some(Err(format!("#{id} waits for an owner or admin already")))
        }
        Store::Org => {
            held.waiting.insert(id);
            held.dropped.remove(&id);
            held.refused.remove(&id);
            held.not_now.remove(&id);
            Some(Ok(()))
        }
        Store::Project if held.waiting.remove(&id) => Some(Ok(())),
        _ => None,
    }
}

/// Brings the waiting list of the project whose own store is `local` in step: what this host's
/// person moved or took back, what an owner or admin decided of it, and, for an owner or admin,
/// a call for each entry still waiting.
pub(super) async fn sync(core: &Arc<Core>, local: &str, shared: &Shared) {
    let manages = vault::manages_org(core, &shared.org);
    for _ in 0..TRIES {
        let Some(read) = vault::read(core, &shared.org, &shared.waiting_id()) else {
            return;
        };
        let (revision, list) = match read {
            Some(read) => match serde_json::from_value::<List>(read.body) {
                Ok(list) => (read.revision, list),
                Err(error) => {
                    eprintln!("sikemux core: an org's waiting rules could not be read: {error}");
                    return;
                }
            },
            None => (0, List::default()),
        };
        let mut now = list.clone();
        settle_mine(core, local, shared, &mut now);
        if manages {
            decide(core, shared, &mut now);
            ask(core, local, shared, &now);
        }
        if now == list {
            return;
        }
        let Ok(value) = serde_json::to_value(&now) else {
            return;
        };
        let id = shared.waiting_id();
        match vault::put_project(
            core,
            &shared.org,
            PROJECT_MEMORY_WAITING,
            &id,
            revision,
            &value,
        )
        .await
        {
            Ok(Put::Written) => return,
            Ok(Put::Stale) => {}
            Ok(Put::Refused(why)) => {
                eprintln!(
                    "sikemux core: {}'s waiting rules refused a change: {why}",
                    shared.name
                );
                return;
            }
            Err(error) => {
                eprintln!(
                    "sikemux core: {}'s waiting rules were not written: {error}",
                    shared.name
                );
                return;
            }
        }
    }
}

/// Puts this host's waiting entries on the list as they are now, and takes in what an owner or
/// admin decided of them: a kept entry leaves the project's own store, a dropped one stays there.
fn settle_mine(core: &Core, local: &str, shared: &Shared, list: &mut List) {
    let memory = core.memory();
    let mut state = memory.state.locked();
    let Some(held) = state.held.get_mut(local) else {
        return;
    };
    let mut changed = false;
    let mine: Vec<(String, u64)> = (list.items.iter())
        .filter(|(_, item)| item.member == shared.member)
        .filter_map(|(key, _)| {
            let id = key.rsplit(':').next()?.parse().ok()?;
            Some((key.clone(), id))
        })
        .collect();
    for (key, id) in mine {
        let Some(item) = list.items.get_mut(&key) else {
            continue;
        };
        let waits = held.waiting.contains(&id);
        match item.decided.clone() {
            Some(Decided::Kept) => {
                if waits {
                    held.rules.retain(|rule| rule.id != id);
                    held.knowledge.retain(|known| known.id != id);
                    held.waiting.remove(&id);
                    changed = true;
                }
                list.items.remove(&key);
            }
            Some(Decided::Dropped { by }) => {
                if waits {
                    held.waiting.remove(&id);
                    held.dropped.insert(id, by.unwrap_or_default());
                    changed = true;
                }
                list.items.remove(&key);
            }
            None => match Waiting::of(held, id).filter(|_| waits) {
                Some(entry) => item.entry = entry,
                None => drop(list.items.remove(&key)),
            },
        }
    }
    let from = moving::first_name(core);
    for id in held.waiting.clone() {
        let key = item_key(&shared.member, id);
        if list.items.contains_key(&key) {
            continue;
        }
        match Waiting::of(held, id) {
            Some(entry) => {
                list.items.insert(
                    key,
                    Item {
                        member: shared.member.clone(),
                        from: from.clone(),
                        entry,
                        decided: None,
                    },
                );
            }
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

/// Writes onto the list what this host's person decided, and forgets what the list already says.
fn decide(core: &Core, shared: &Shared, list: &mut List) {
    let prefix = format!("{}|", shared.key());
    let mut decided = core.memory().decided.locked();
    decided.retain(|at, decision| {
        let Some(item) = at.strip_prefix(&prefix) else {
            return true;
        };
        match list.items.get_mut(item) {
            Some(item) if item.decided.is_none() => {
                item.decided = Some(decision.clone());
                true
            }
            _ => false,
        }
    });
}

/// Puts a call to this host's person, an owner or admin, for each entry waiting on the list, and
/// closes the calls of those another took care of or their member took back.
fn ask(core: &Core, local: &str, shared: &Shared, list: &List) {
    let (project, name) = {
        let state = core.memory().state.locked();
        let folder = (state.keys.iter()).find(|(_, key)| **key == local);
        match folder {
            Some((folder, _)) => (folder.clone(), moving::project_name(&state, local)),
            None => return,
        }
    };
    let waiting: BTreeMap<String, &Item> = (list.items.iter())
        .filter(|(_, item)| item.decided.is_none() && item.member != shared.member)
        .map(|(key, item)| (call_key(shared, key, &item.entry), item))
        .collect();
    let prefix = format!("{ORG_WAITING}{}|", shared.key());
    let open = person_calls::open_keyed(core, &project, &prefix);
    for (id, key) in &open {
        if !waiting.contains_key(key) {
            person_calls::set_outcome(core, id, ended(CallResult::Withdrawn));
        }
    }
    for (key, item) in waiting {
        if open.iter().any(|(_, open)| *open == key) {
            continue;
        }
        if let Err(problem) =
            person_calls::raise(core, call_for(project.clone(), &name, shared, key, item))
        {
            eprintln!("sikemux core: a waiting rule was not put to the person: {problem}");
        }
    }
}

fn call_for(project: String, name: &str, shared: &Shared, key: String, item: &Item) -> Raise {
    let from = item.from.as_deref().unwrap_or("A member");
    let (what, them) = match item.entry {
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
        context: Some(format!(
            "\"{}\"\n\nKept, every member's agents on {name} {them}.",
            item.entry.text()
        )),
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

/// The person kept or dropped an entry waiting on the org's list: a kept one goes into the org's
/// store under its member's name, and the list hears the answer at the next sync.
pub(crate) fn answered(core: &Arc<Core>, call: &PersonCall, key: &str) {
    person_calls::set_outcome(core, &call.id, ended(CallResult::Heard));
    let Some((store, item_at)) = parse(key) else {
        return;
    };
    let picked = call
        .answer
        .as_ref()
        .and_then(|answer| answer.option.as_deref());
    let decision = match picked {
        Some(KEEP) => Decided::Kept,
        Some(DROP) => Decided::Dropped {
            by: moving::first_name(core),
        },
        _ => return,
    };
    let memory = core.memory();
    if decision == Decided::Kept {
        let mut state = memory.state.locked();
        let Some(item) = waiting_item(core, &state, store, item_at) else {
            return;
        };
        if let Err(problem) = keep(&mut state, store, item) {
            eprintln!("sikemux core: a waiting rule was not kept: {problem}");
            return;
        }
        memory.save(&mut state, store);
        drop(state);
        tell_clients(core, false);
    }
    memory
        .decided
        .locked()
        .insert(format!("{store}|{item_at}"), decision);
    memory.org_sync.notify_one();
}

fn waiting_item(core: &Core, state: &State, store: &str, item: &str) -> Option<Item> {
    let shared = state.shared.values().find(|shared| shared.key() == store)?;
    let read = vault::read(core, &shared.org, &shared.waiting_id())??;
    let mut list: List = serde_json::from_value(read.body).ok()?;
    list.items
        .remove(item)
        .filter(|item| item.decided.is_none())
}

fn keep(state: &mut State, store: &str, item: Item) -> Result<(), String> {
    let held = holding(state, store, Store::Org)?;
    let same = |text: &str| text.eq_ignore_ascii_case(item.entry.text());
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
    match item.entry {
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
    held.writers.insert(id, item.member);
    if let Some(full) = super::org::too_big(held) {
        held.rules.retain(|rule| rule.id != id);
        held.knowledge.retain(|known| known.id != id);
        held.writers.remove(&id);
        return Err(full);
    }
    Ok(())
}
