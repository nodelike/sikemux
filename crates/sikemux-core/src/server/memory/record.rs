//! A project's store as part of its record: its home sends it to the linked hosts that hold the
//! project, and their writes to it go to the home. The person's own store stays on each host.

use crate::server::parts::Parts;
use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;
use sikemux_process::sync::Locked;
use sikemux_view::memory::Store;

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
    let state = core.memory().state.locked();
    let mut held = state.held.get(key)?.clone();
    held.removed.clear();
    serde_json::to_value(held).ok()
}

/// The project's home sent what its store under `key` keeps. It replaces this host's copy, which
/// keeps only the notes for this host's chats; an entry of the person's own that shares an id with
/// one sent takes a new one.
pub(crate) fn mirror(core: &Core, key: &str, record: Value) -> Result<(), String> {
    let mut held: Held = serde_json::from_value(record).map_err(|error| error.to_string())?;
    if held.store != Store::Project {
        return Err("only a project's store crosses to its linked hosts".into());
    }
    held.key = key.to_owned();
    let memory = core.memory();
    let mut state = memory.state.locked();
    match state.held.remove(key) {
        Some(local) => held.removed = local.removed,
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
    Ok(())
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
        "memoryUpdate" => moving_to(params).is_none() && !goal && held_here(),
        "promoteRule" => held_here(),
        _ => false,
    }
}

/// The store a move from the app takes an entry to, when `params` of its memoryUpdate are one.
pub(crate) fn moving_to(params: &Value) -> Option<Store> {
    serde_json::from_value(params.get("store")?.clone()).ok()
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
    if !record.is_null() {
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
