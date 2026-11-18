//! A project's store as part of its record: its home sends it to the linked hosts that hold the
//! project, and their writes to it go to the home. The person's own store stays on each host.

use crate::server::parts::Parts;
use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;
use sikemux_process::sync::Locked;
use sikemux_view::memory::{Knowledge, MemoryRule, Store};

use super::{tell_clients, Held, MemoryView, Peer, MAX_STORES, PERSONAL};
use crate::server::Core;

/// The calls from the app that change what Sikemux keeps.
pub(crate) const APP_WRITES: [&str; 6] = [
    "memoryAdd",
    "memoryUpdate",
    "promoteRule",
    "promoteRules",
    "keepSeededRules",
    "seedRules",
];

/// What the store under `key` keeps, as its home sends it, without the notes for its own chats.
pub(crate) fn record(core: &Core, key: &str) -> Option<Value> {
    let mut held = core.memory().state.locked().held.get(key)?.clone();
    held.removed.clear();
    held.mirrored = false;
    serde_json::to_value(held).ok()
}

/// Whether the store under `key` is a copy of what another host, the project's home, sent.
pub(crate) fn is_copy(core: &Core, key: &str) -> bool {
    (core.memory().state.locked().held.get(key)).is_some_and(|held| held.mirrored)
}

/// This host keeps the store under `key` as the project's home from now on.
pub(crate) fn kept_here(core: &Core, key: &str) {
    let memory = core.memory();
    let mut state = memory.state.locked();
    if let Some(held) = state.held.get_mut(key).filter(|held| held.mirrored) {
        held.mirrored = false;
        memory.save(&mut state, key);
    }
}

/// The project's home sent what its store under `key` keeps. It replaces this host's copy, which
/// keeps only the notes for this host's chats; an entry of the person's own that shares an id with
/// one sent takes a new one. Answers what this host kept as the project's own until now, for the
/// home to merge in.
pub(crate) fn mirror(core: &Core, key: &str, record: Value) -> Result<Option<Value>, String> {
    let mut held: Held = serde_json::from_value(record).map_err(|error| error.to_string())?;
    if held.store != Store::Project {
        return Err("only a project's store crosses to its linked hosts".into());
    }
    held.key = key.to_owned();
    held.mirrored = true;
    let memory = core.memory();
    let mut state = memory.state.locked();
    let mut kept_as_home = None;
    match state.held.remove(key) {
        Some(mut local) => {
            held.removed = std::mem::take(&mut local.removed);
            if !local.mirrored && (local.len() > 0 || !local.set_aside.is_empty()) {
                kept_as_home = serde_json::to_value(local).ok();
            }
        }
        None if state
            .held
            .values()
            .filter(|held| state.in_use(held))
            .count()
            >= MAX_STORES =>
        {
            return Err(format!(
                "this host already keeps rules and knowledge for {MAX_STORES} projects"
            ))
        }
        None => {}
    }
    let sent: HashSet<u64> = (held.rules.iter().map(|rule| rule.id))
        .chain(held.knowledge.iter().map(|known| known.id))
        .chain(held.set_aside.iter().map(|apart| apart.rule.id))
        .collect();
    state.held.insert(key.to_owned(), held);
    let renumbered = renumber(&mut state, &sent);
    memory.save(&mut state, key);
    if renumbered {
        memory.save(&mut state, PERSONAL);
    }
    drop(state);
    tell_clients(core, true);
    Ok(kept_as_home)
}

/// Merges in the store another host kept as the project's own before this host's copy reached
/// it. An entry is the same when its words are, or when it has the same id and one side was
/// corrected from the other; the one learned or corrected last wins, and one this store lacks gets
/// a new id. A rule set aside and the same rule listed again: the later one stands. Nothing else
/// removed here stays removed, so knowledge or a rule removed here comes back.
pub(crate) fn merge_in(core: &Core, key: &str, theirs: Value) -> Result<(), String> {
    let theirs: Held = serde_json::from_value(theirs).map_err(|error| error.to_string())?;
    let memory = core.memory();
    let mut state = memory.state.locked();
    if !state.held.contains_key(key) {
        return Err("this host keeps no rules or knowledge for that project".into());
    }
    for mut rule in theirs.rules {
        let Some(own) = state.held.get_mut(key) else {
            break;
        };
        let said = Said::rule(&rule);
        let apart = (own.set_aside.iter().enumerate())
            .find(|(_, apart)| said.same(&Said::rule(&apart.rule)))
            .map(|(at, apart)| (at, apart.at));
        match apart {
            Some((at, aside)) if rule.learned.at > aside => {
                rule.id = own.set_aside.remove(at).rule.id;
                own.rules.push(rule);
                continue;
            }
            Some(_) => continue,
            None => {}
        }
        match (own.rules.iter_mut()).find(|own| said.same(&Said::rule(own))) {
            Some(own) if rule.learned.at > own.learned.at => {
                rule.id = own.id;
                *own = rule;
            }
            Some(_) => {}
            None => {
                rule.id = state.next_id(key);
                if let Some(own) = state.held.get_mut(key) {
                    own.rules.push(rule);
                }
            }
        }
    }
    for mut known in theirs.knowledge {
        let Some(own) = state.held.get_mut(key) else {
            break;
        };
        let said = Said::known(&known);
        match (own.knowledge.iter_mut()).find(|own| said.same(&Said::known(own))) {
            Some(own) if known.learned.at > own.learned.at => {
                known.id = own.id;
                *own = known;
            }
            Some(_) => {}
            None => {
                known.id = state.next_id(key);
                if let Some(own) = state.held.get_mut(key) {
                    own.knowledge.push(known);
                }
            }
        }
    }
    for mut apart in theirs.set_aside {
        let Some(own) = state.held.get_mut(key) else {
            break;
        };
        let said = Said::rule(&apart.rule);
        if let Some(own) = (own.set_aside.iter_mut()).find(|own| said.same(&Said::rule(&own.rule)))
        {
            if apart.at > own.at {
                apart.rule.id = own.rule.id;
                *own = apart;
            }
            continue;
        }
        let listed = (own.rules.iter().enumerate())
            .find(|(_, rule)| said.same(&Said::rule(rule)))
            .map(|(at, rule)| (at, rule.learned.at));
        match listed {
            Some((at, learned)) if apart.at > learned => {
                apart.rule.id = own.rules.remove(at).id;
            }
            Some(_) => continue,
            None => apart.rule.id = state.next_id(key),
        }
        if let Some(own) = state.held.get_mut(key) {
            own.set_aside.push(apart);
        }
    }
    if let Some(own) = state.held.get_mut(key) {
        own.seeding = own.seeding.take().or(theirs.seeding);
    }
    memory.save(&mut state, key);
    drop(state);
    tell_clients(core, true);
    Ok(())
}

struct Said<'a> {
    id: u64,
    text: &'a str,
    at: u64,
    was: Option<&'a str>,
}

impl<'a> Said<'a> {
    fn rule(rule: &'a MemoryRule) -> Self {
        let (id, text, at, was) = (rule.id, &rule.text, rule.learned.at, &rule.was);
        Said {
            id,
            text,
            at,
            was: was.as_deref(),
        }
    }

    fn known(known: &'a Knowledge) -> Self {
        let (id, text, at, was) = (known.id, &known.text, known.learned.at, &known.was);
        Said {
            id,
            text,
            at,
            was: was.as_deref(),
        }
    }

    fn same(&self, other: &Said) -> bool {
        self.text == other.text
            || self.id == other.id
                && (self.at == other.at
                    || self.was == Some(other.text)
                    || other.was == Some(self.text))
    }
}

fn renumber(state: &mut super::State, taken: &HashSet<u64>) -> bool {
    let Some(personal) = state.held.get(PERSONAL) else {
        return false;
    };
    let clashing: Vec<u64> = (personal.rules.iter().map(|rule| rule.id))
        .chain(personal.knowledge.iter().map(|known| known.id))
        .chain(personal.set_aside.iter().map(|apart| apart.rule.id))
        .filter(|id| taken.contains(id))
        .collect();
    for old in &clashing {
        let new = state.next_id(PERSONAL);
        let Some(personal) = state.held.get_mut(PERSONAL) else {
            break;
        };
        let rules = (personal.rules.iter_mut().map(|rule| &mut rule.id)).chain(
            personal
                .set_aside
                .iter_mut()
                .map(|apart| &mut apart.rule.id),
        );
        for id in rules.chain(personal.knowledge.iter_mut().map(|known| &mut known.id)) {
            if *id == *old {
                *id = new;
            }
        }
    }
    !clashing.is_empty()
}

/// Whether a write to what Sikemux keeps is to the person's own store, which stays on this host.
pub(crate) fn personal(core: &Core, method: &str, params: &Value) -> bool {
    let flag = |name: &str| params.get(name).and_then(Value::as_bool) == Some(true);
    let goal = params
        .get("goal")
        .is_some_and(|goal| !goal.is_null() && goal != false);
    let held_here = || {
        let Some(id) = params.get("id").and_then(Value::as_u64) else {
            return false;
        };
        let state = core.memory().state.locked();
        let found = (state.held.get(PERSONAL)).is_some_and(|held| {
            held.holds(id) || held.set_aside.iter().any(|apart| apart.rule.id == id)
        });
        found
    };
    match method {
        "memory.add" => flag("yours") && !goal,
        "memoryAdd" => params.pointer("/entry/store").and_then(Value::as_str) == Some("personal"),
        "memory.update" => !goal && held_here(),
        "memoryUpdate" => store_asked(params).is_none() && !goal && held_here(),
        "promoteRule" => held_here(),
        _ => false,
    }
}

/// The store a move from the app takes an entry to, when `params` of its memoryUpdate are one.
fn store_asked(params: &Value) -> Option<Store> {
    serde_json::from_value(params.get("store")?.clone()).ok()
}

/// The store a move from the app takes an entry to across the person's store, kept here, and the
/// project's, kept at its home; none for a move to or from the org's store, or off its waiting
/// list, which the home does.
pub(crate) fn moving_to(core: &Core, folder: &str, params: &Value) -> Option<Store> {
    let to = store_asked(params)?;
    let id = params.get("id").and_then(Value::as_u64);
    let state = core.memory().state.locked();
    let in_org = (state.org_key(folder))
        .and_then(|key| state.held.get(&key))
        .is_some_and(|held| id.is_some_and(|id| held.holds(id)));
    let taking_back = to == Store::Project
        && (state.own(folder)).is_some_and(|held| id.is_some_and(|id| held.holds(id)));
    (to != Store::Org && !in_org && !taking_back).then_some(to)
}

/// The view of the project at `folder` after its home did an app's write and sent back what the
/// project's store keeps now, if it keeps one, as when the write went to a goal's card.
pub(crate) fn written_at_home(
    core: &Arc<Core>,
    folder: &str,
    key: &str,
    peer: &Peer,
    record: Value,
) -> Result<MemoryView, String> {
    if !record.is_null() && is_copy(core, key) {
        mirror(core, key, record)?;
    }
    Ok(super::view(core, folder, peer))
}

/// What the store of the project at `folder` keeps, as its home answers a write to it.
pub(crate) fn record_at(core: &Core, folder: &str) -> Option<Value> {
    let key = core.memory().state.locked().key_of(folder);
    record(core, &key)
}

/// Takes the keys of the project stores saved since this was last asked.
pub(super) fn saved_since(core: &Core) -> Vec<String> {
    std::mem::take(&mut core.memory().state.locked().saved)
        .into_iter()
        .collect()
}

/// The lowest id no entry kept on this host has.
pub(crate) fn free_here(core: &Core) -> u64 {
    let state = core.memory().state.locked();
    let free = (state.held.values().map(|held| held.next))
        .max()
        .unwrap_or(1);
    free
}

/// The linked hosts have given entries ids below `free`.
pub(crate) fn free_elsewhere(core: &Core, free: u64) {
    core.memory().state.locked().free_elsewhere = free;
}
