//! The rules and knowledge of a project shared with an org, kept in the org's vault so its
//! members' agents follow them too. Each core mirrors the vault's entry into a store of its own
//! and writes its person's changes back over the revision it last read, so a write that crossed
//! another member's is redone on top of theirs.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::memory::{Knowledge, MemoryRule, Moved, Store};

use super::{holding, tell_clients, Held, State};
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::vault::org::{self as vault, Put, PROJECT_MEMORY, PROJECT_MEMORY_WAITING};
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

/// The most the vault's entry of a project's store may hold, under the server's limit.
const MAX_BODY: usize = 60 * 1024;
const TRIES: usize = 3;

/// A project open here that this account's person shares with an org.
#[derive(Clone)]
pub(super) struct Shared {
    pub(super) org: String,
    /// The org's name, as its members see it.
    pub(super) name: String,
    /// What the account server knows the project by.
    project: String,
    /// This account's membership in the org.
    pub(super) member: String,
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

    pub(super) fn waiting_id(&self) -> String {
        vault::project_entry_id(PROJECT_MEMORY_WAITING, &self.project)
    }
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
            rules: held.rules.clone(),
            knowledge: held.knowledge.clone(),
            writers: held.writers.clone(),
        }
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

/// `mine`'s changes since `base`, made again on top of `remote`. An entry added here under an id
/// another member's core gave an entry of its own meanwhile takes a fresh id from `fresh`.
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
        let taken = !before.contains_key(id) && entries.get(id).is_some_and(|there| there != entry);
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
    let Some(shared) = shared(core).await else {
        return;
    };
    {
        let memory = core.memory();
        let mut state = memory.state.locked();
        end(core, &mut state, &shared);
        state.shared = shared.clone();
    }
    for (local, shared) in &shared {
        mirror(core, shared);
        if shared.writes {
            push(core, shared).await;
        }
        super::waiting::sync(core, local, shared).await;
    }
}

/// The projects open here that this account's person shares with one org, by their store's
/// key; none while this host has not heard the registry.
async fn shared(core: &Core) -> Option<HashMap<String, Shared>> {
    let registry = crate::server::account::shares::read(core).await?;
    let orgs = sikemux_corekit::accounts::orgs(core.account().shown().as_ref()).to_vec();
    let mut shared = HashMap::new();
    for identity in (core.workspaces().projects().into_iter()).filter_map(|known| known.identity) {
        let project = crate::server::shares::key(&identity);
        let mut mine = (registry.orgs.iter())
            .filter(|list| list.mine.iter().any(|share| share.project == project));
        let (Some(list), None) = (mine.next(), mine.next()) else {
            continue;
        };
        let (Some(me), Some(org)) = (
            list.members.iter().find(|member| member.you),
            orgs.iter().find(|org| org.id == list.org),
        ) else {
            continue;
        };
        let approves = crate::server::account::org_policy(core, &list.org)
            .await
            .admins_approve_rules;
        let writes = org.active && (vault::manages_org(core, &list.org) || !approves);
        shared.insert(
            identity,
            Shared {
                org: list.org.clone(),
                name: org.name.clone(),
                project,
                member: me.id.clone(),
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
    Some(shared)
}

/// Ends the org's store of each project open here that nobody shares with the org any more:
/// what this account's person put there comes back to the project's own store, and the rest goes.
fn end(core: &Core, state: &mut State, now: &HashMap<String, Shared>) {
    let ended: Vec<(String, String, Shared)> = (state.shared.iter())
        .filter(|(local, was)| (now.get(*local)).is_none_or(|is| is.key() != was.key()))
        .map(|(local, was)| (local.clone(), was.key(), was.clone()))
        .collect();
    for (local, key, was) in ended {
        let Some(org) = state.held.remove(&key) else {
            continue;
        };
        let theirs = |id: u64| org.writers.get(&id) == Some(&was.member);
        let rules: Vec<MemoryRule> = (org.rules.into_iter())
            .filter(|rule| theirs(rule.id))
            .collect();
        let knowledge: Vec<Knowledge> = (org.knowledge.into_iter())
            .filter(|known| theirs(known.id))
            .collect();
        if !rules.is_empty() || !knowledge.is_empty() {
            if let Ok(own) = holding(state, &local, Store::Project) {
                own.next = own.next.max(org.next);
                own.rules.extend(rules);
                own.knowledge.extend(knowledge);
            }
            core.memory().save(state, &local);
        }
        core.memory().forget(state, &key);
    }
    if !state.shared.is_empty() || !now.is_empty() {
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
        Some(read) => match serde_json::from_value(read.body) {
            Ok(body) => Base {
                revision: read.revision,
                body,
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
    let Ok(held) = holding(&mut state, &key, Store::Org) else {
        return;
    };
    if (held.base.as_ref()).is_some_and(|base| base.revision == remote.revision) {
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
    held.knowledge = body.knowledge;
    held.writers = body.writers;
    held.base = Some(remote);
    let renumbered = renumber_clashes(&mut state, shared);
    memory.save(&mut state, &key);
    for local in renumbered {
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
            renumbered.push(key.clone());
        }
    }
    renumbered.dedup();
    renumbered
}

/// The key of the project's own store on this host.
fn local_key(state: &State, shared: &Shared) -> Option<String> {
    (state.keys.values())
        .find(|key| (state.shared.get(*key)).is_some_and(|is| is.key() == shared.key()))
        .cloned()
}

/// Puts the org's store back as the vault last gave it, after the vault refused a change for
/// `why`. What was added or moved here goes back to where it came from, saying why.
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
    let added: Vec<MemoryRule> = (held.rules.drain(..))
        .filter(|rule| !kept(rule.id))
        .collect();
    let added_knowledge: Vec<Knowledge> = (held.knowledge.drain(..))
        .filter(|known| !kept(known.id))
        .collect();
    held.rules = base.body.rules;
    held.knowledge = base.body.knowledge;
    held.writers = base.body.writers;
    let local = local_key(&state, shared);
    let home = |state: &State, moved: &Option<Moved>| {
        (moved.as_ref())
            .map(|moved| moved.from.clone())
            .filter(|from| *from != key && state.held.contains_key(from))
            .or_else(|| local.clone())
    };
    let mut touched = vec![key.clone()];
    for mut rule in added {
        let Some(to) = home(&state, &rule.moved) else {
            continue;
        };
        rule.moved = None;
        if let Ok(held) = holding(&mut state, &to, Store::Project) {
            held.refused.insert(rule.id, why.to_owned());
            held.rules.push(rule);
            touched.push(to);
        }
    }
    for mut known in added_knowledge {
        let Some(to) = home(&state, &known.moved) else {
            continue;
        };
        known.moved = None;
        if let Ok(held) = holding(&mut state, &to, Store::Project) {
            held.refused.insert(known.id, why.to_owned());
            held.knowledge.push(known);
            touched.push(to);
        }
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
        let (revision, body) = {
            let mut state = core.memory().state.locked();
            let Some(held) = state.held.get_mut(&key) else {
                return;
            };
            let Some(base) = held.base.clone() else {
                return;
            };
            for id in (held.rules.iter().map(|rule| rule.id))
                .chain(held.knowledge.iter().map(|known| known.id))
            {
                held.writers
                    .entry(id)
                    .or_insert_with(|| shared.member.clone());
            }
            let ids: Vec<u64> = (held.rules.iter().map(|rule| rule.id))
                .chain(held.knowledge.iter().map(|known| known.id))
                .collect();
            held.writers.retain(|id, _| ids.contains(id));
            let body = Body::of(held);
            if body.same_entries(&base.body) {
                return;
            }
            (base.revision, body)
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
            Ok(Put::Written) => {
                let revision = (vault::read(core, &shared.org, &shared.entry_id()))
                    .flatten()
                    .map_or(revision, |read| read.revision);
                let memory = core.memory();
                let mut state = memory.state.locked();
                if let Some(held) = state.held.get_mut(&key) {
                    held.base = Some(Base { revision, body });
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

/// The store an entry for `project` goes to: a rule the person said goes to the org's, while the
/// project is shared with one that lets them add to it.
pub(super) fn persons(state: &State, project: &str, store: Store, persons_rule: bool) -> Store {
    let writes = (state.shared.get(&state.key_of(project))).is_some_and(|shared| shared.writes);
    match store {
        Store::Project if persons_rule && writes => Store::Org,
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
