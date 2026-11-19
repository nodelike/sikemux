//! What the core remembers: the rules agents follow and knowledge of how things are, in two
//! stores. A project's is kept by its repository's identity, so it is the same wherever the
//! repository is open; the person's own follows them to every project. Kept in `memory/` in the
//! data folder, one file per store. Every entry says where it was learned. The core tests the
//! knowledge it can; knowledge that no longer holds is marked stale, stays
//! out of briefs and waits in the inbox until someone corrects or retires it.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::memory::{
    KeepSeededParams, MemoryEntryParams, MemoryParams, MemoryUpdateParams,
};
pub use sikemux_calls::memory::{MemoryAddParams, NewMemory};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_view::memory::{
    Knowledge, Learned, ListedRule, MemoryCheck, MemoryRule, MemoryView, Seeding, Stale, Store,
    StoreMemory,
};
use sikemux_view::playbook::Entry;

use super::access::Peer;
use super::board::goal::{self, learned};
use super::board::{self, project_at};
use super::changed::Topic;
use super::connection::blocking;
use super::kept::{self, Kept};
use super::wake::Wake;
use super::Core;
use crate::cli::protocol::HarnessRequest;
use crate::text::to_keep;

mod brief;
mod check;
pub(crate) mod moving;
mod now_check;
mod org;
mod promote;
mod record;
mod removed;
mod seed;
mod waiting;

pub(crate) use brief::{brief, for_chat};
pub(crate) use check::settle;
pub(crate) use now_check::{set_aside, why as set_aside_why, SetAside};
pub(crate) use promote::{promote, promote_all};
pub(crate) use record::{
    free_elsewhere, free_here, is_copy, kept_here, merge_in, mirror, moving_to,
    personal as writes_personal, record, record_at, written_at_home,
    APP_WRITES as APP_RECORD_WRITES,
};
pub(crate) use removed::{
    chat_ready, feed_moved, show_on as rules_missing, take_note as take_removed_note,
};
pub(crate) use seed::{again as seed_again, keep as keep_seeded};
pub(crate) use waiting::answered as waiting_answered;

const FOLDER: &str = "memory";
const MAX_STORES: usize = 256;
const PERSONAL: &str = "personal";
const MAX_ENTRIES: usize = 200;
const MAX_TEXT_CHARS: usize = 1000;
const MAX_ABOUT: usize = 12;
const MAX_ABOUT_CHARS: usize = 200;
const MAX_CHECK_CHARS: usize = 1000;
const NOT_SHARED: &str = "This project is not shared with an org";

/// What one store keeps. Kept even once empty, so `next` never goes back.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Held {
    /// The project's identity, or its folder when it is outside git; `personal` for the person's.
    key: String,
    store: Store,
    next: u64,
    rules: Vec<MemoryRule>,
    knowledge: Vec<Knowledge>,
    /// The search of a project's past sessions for rules, kept on the
    /// project's store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seeding: Option<Seeding>,
    /// Rules of the person's that left the list as a check or by an agent, kept to bring back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    set_aside: Vec<now_check::SetAside>,
    /// Rules kept from a chat's words that the person removed, until that chat's agent hears it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed: Vec<removed::Removed>,
    /// Entries whose offer to move to the other store the person turned down.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    not_now: BTreeSet<u64>,
    /// Other projects' rules in the same words, taken in when one moved into the person's store.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    absorbed: Vec<moving::Absorbed>,
    /// It is a copy of what another host, the project's home, sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    mirrored: bool,
    /// For an org's store, the member who put each entry there.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    writers: BTreeMap<u64, String>,
    /// For an org's store, its entry in the org's vault as last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<org::Base>,
    /// Entries of a project's own store the person moved to its org's, waiting for an owner or
    /// admin to keep them there.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    waiting: BTreeSet<u64>,
    /// Entries an owner or admin dropped from the org's waiting list, by that admin's first name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    dropped: BTreeMap<u64, String>,
    /// Entries the org's vault would not take, kept here instead, with the vault's reason.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    refused: BTreeMap<u64, String>,
    /// For an org's store, what an owner or admin here kept from a teammate's waiting list and
    /// the vault does not hold yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keeps: Vec<waiting::Keep>,
    /// For an org's store, what an owner or admin here dropped from a teammate's waiting list
    /// and the vault does not hold yet, by member and entry, with the hash of the words read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    drops: BTreeMap<String, String>,
    /// The org the project was shared with when `waiting`, `dropped` and `refused` were last
    /// kept; they end with that share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared_with: Option<String>,
    /// For an org's store, the project's own store and the membership it was kept for here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kept_for: Option<org::KeptFor>,
    /// The keys this store was kept under before its project's identity changed.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    was: BTreeSet<String>,
    /// Entries given a new number when the org's store came to hold their old one, old to new.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    renumbered: BTreeMap<u64, u64>,
}

impl Held {
    fn new(key: &str, store: Store) -> Self {
        Self {
            key: key.to_owned(),
            store,
            next: 1,
            rules: Vec::new(),
            knowledge: Vec::new(),
            seeding: None,
            set_aside: Vec::new(),
            removed: Vec::new(),
            not_now: BTreeSet::new(),
            absorbed: Vec::new(),
            mirrored: false,
            writers: BTreeMap::new(),
            base: None,
            waiting: BTreeSet::new(),
            dropped: BTreeMap::new(),
            refused: BTreeMap::new(),
            keeps: Vec::new(),
            drops: BTreeMap::new(),
            shared_with: None,
            kept_for: None,
            was: BTreeSet::new(),
            renumbered: BTreeMap::new(),
        }
    }

    fn len(&self) -> usize {
        self.rules.len() + self.knowledge.len()
    }

    fn words(&self, id: u64) -> Option<&str> {
        let rule = self
            .rules
            .iter()
            .find(|rule| rule.id == id)
            .map(|rule| &rule.text);
        rule.or_else(|| {
            (self.knowledge.iter().find(|known| known.id == id)).map(|known| &known.text)
        })
        .map(String::as_str)
    }

    fn holds(&self, id: u64) -> bool {
        self.rules.iter().any(|rule| rule.id == id)
            || self.knowledge.iter().any(|known| known.id == id)
    }

    fn take_in(&mut self, from: Held) {
        self.next = self.next.max(from.next);
        self.rules.extend(from.rules);
        self.knowledge.extend(from.knowledge);
        self.seeding = self.seeding.take().or(from.seeding);
        self.set_aside.extend(from.set_aside);
        self.removed.extend(from.removed);
        self.not_now.extend(from.not_now);
        self.absorbed.extend(from.absorbed);
        self.waiting.extend(from.waiting);
        self.dropped.extend(from.dropped);
        self.refused.extend(from.refused);
        self.was.extend(from.was);
        self.renumbered.extend(from.renumbered);
    }
}

#[derive(Default)]
struct State {
    held: BTreeMap<String, Held>,
    /// Each project folder's store key, as [`rekey`] last found them.
    keys: HashMap<String, String>,
    files: HashMap<String, Arc<Kept>>,
    /// Knowledge being tested now.
    checking: HashSet<u64>,
    /// Knowledge a brief carried on a test that was not fresh, to test soon.
    wanted: HashSet<u64>,
    /// The rule being written into AGENTS.md. It stays listed but is saved as
    /// gone, so a core that stops halfway never brings it back.
    promoting: Option<u64>,
    /// Each chief's turn, and the rules it named said again in that turn.
    said_again: HashMap<String, (Option<u64>, HashSet<u64>)>,
    /// The project stores saved since their home last sent them to the linked hosts.
    saved: BTreeSet<String>,
    /// The lowest id no linked host has given an entry yet, so one kept here never takes the id
    /// of a linked host's own note.
    free_elsewhere: u64,
    /// The projects open here shared with an org, by their own store's key.
    shared: HashMap<String, org::Shared>,
    /// The projects open here shared with an org whose vault this host cannot open yet, by their
    /// own store's key, with the org's name.
    shut: HashMap<String, String>,
}

impl State {
    /// The one folder on this host that stands for a project's key, the same on every read.
    fn folder_of(&self, key: &str) -> Option<&String> {
        (self.keys.iter())
            .filter(|(_, kept)| *kept == key)
            .map(|(folder, _)| folder)
            .min()
    }

    /// The key of the store of the project in folder `project`.
    fn key_of(&self, project: &str) -> String {
        (self.keys.get(project).cloned()).unwrap_or_else(|| project.to_owned())
    }

    fn key_for(&self, project: &str, store: Store) -> String {
        match store {
            Store::Project => self.key_of(project),
            Store::Personal => PERSONAL.to_owned(),
            Store::Org => self.org_key(project).unwrap_or_default(),
        }
    }

    /// The key of the store of the org `project` is shared with.
    fn org_key(&self, project: &str) -> Option<String> {
        self.shared.get(&self.key_of(project)).map(org::Shared::key)
    }

    /// The stores whose entries apply to `project`: its own, its org's, then the person's.
    fn applying<'a>(&'a self, project: &str) -> impl Iterator<Item = &'a Held> + 'a {
        [
            self.held.get(&self.key_of(project)),
            (self.org_key(project)).and_then(|key| self.held.get(&key)),
            self.held.get(PERSONAL),
        ]
        .into_iter()
        .flatten()
    }

    /// The project's own store.
    fn own(&self, project: &str) -> Option<&Held> {
        self.held.get(&self.key_of(project))
    }

    /// The number entry `id` has now, when `id` is one a chat line kept before the entry was
    /// renumbered.
    fn kept_id(&self, project: &str, id: u64) -> u64 {
        self.applying(project)
            .find_map(|held| held.renumbered.get(&id).copied())
            .unwrap_or(id)
    }

    /// The key of the store applying to `project` that keeps entry `id`.
    fn key_holding(&self, project: &str, id: u64) -> Result<String, String> {
        self.applying(project)
            .find(|held| held.holds(id))
            .map(|held| held.key.clone())
            .ok_or_else(|| format!("Nothing Sikemux keeps for this project is #{id}"))
    }

    /// A folder on this host to test a store's knowledge in: the person's home for their own
    /// store, else a folder of the project's; none for a store of no project open here.
    fn folder_of_store(&self, held: &Held) -> Option<PathBuf> {
        match held.store {
            Store::Personal => std::env::var_os("HOME").map(PathBuf::from),
            Store::Project => self.folder_of(&held.key).map(PathBuf::from),
            Store::Org => (self.keys.iter())
                .filter(|(_, key)| self.shared.get(*key).is_some_and(|is| is.key() == held.key))
                .map(|(folder, _)| PathBuf::from(folder))
                .min(),
        }
    }

    /// Whether the store is the person's own, or a project's or its org's open on this host.
    fn in_use(&self, held: &Held) -> bool {
        held.store == Store::Personal
            || self.keys.values().any(|key| *key == held.key)
            || self.shared.values().any(|shared| shared.key() == held.key)
    }

    /// An id no entry has ever had.
    fn next_id(&mut self, key: &str) -> u64 {
        let id = (self.held.values().map(|held| held.next))
            .chain([self.free_elsewhere])
            .max()
            .unwrap_or(1);
        if let Some(held) = self.held.get_mut(key) {
            held.next = id + 1;
        }
        id
    }
}

pub(crate) struct Memory {
    state: Mutex<State>,
    folder: Option<PathBuf>,
    /// Held while a rule is promoted, one at a time, so two never write
    /// AGENTS.md over each other.
    promotes: Mutex<()>,
    /// Tests knowledge once it is due.
    pub(crate) wake: Wake,
    /// What the person said in the turn of each agent on a linked host whose write this host,
    /// the project's home, is doing now.
    said_elsewhere: Mutex<HashMap<String, Vec<String>>>,
    /// Each agent told that a number it named was renumbered, with that number, so its next update
    /// by the number is taken as written.
    told_renumbered: Mutex<HashSet<(String, u64)>>,
    /// Told when an org's store is saved here, or shares changed, to sync with the org's vault.
    org_sync: Arc<tokio::sync::Notify>,
    /// What this host's owner or admin answered of entries waiting on an org's list.
    answers: Mutex<waiting::Answers>,
}

impl Memory {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let folder = data_dir.map(|dir| dir.join(FOLDER));
        if let Some(folder) = &folder {
            if let Err(error) = std::fs::create_dir_all(folder) {
                eprintln!("sikemux core: what Sikemux keeps cannot be saved: {error}");
            }
        }
        let held = kept::load_all_upgraded::<Held>(folder.as_deref(), keyed_by_store)
            .into_iter()
            .map(|mut held| {
                held.seeding = seed::settled(held.seeding.take());
                (held.key.clone(), held)
            })
            .collect();
        Self {
            state: Mutex::new(State {
                held,
                ..State::default()
            }),
            folder,
            promotes: Mutex::new(()),
            wake: Wake::default(),
            said_elsewhere: Mutex::default(),
            told_renumbered: Mutex::default(),
            org_sync: Arc::default(),
            answers: Mutex::default(),
        }
    }

    /// Hands the store's file to be written on a thread of its own, and
    /// answers it, to wait for with [`Kept::flush`].
    fn save(&self, state: &mut State, key: &str) -> Option<Arc<Kept>> {
        self.wake.now();
        let State {
            held,
            files,
            promoting,
            saved,
            shared,
            ..
        } = state;
        let held = held.get_mut(key)?;
        match held.store {
            Store::Project => drop(saved.insert(key.to_owned())),
            Store::Org => {
                if let Some(shared) = shared.values().find(|shared| shared.key() == key) {
                    org::own_unwritten(held, &shared.member);
                }
                self.org_sync.notify_one();
            }
            Store::Personal => {}
        }
        let held = &*held;
        let file = files
            .entry(key.to_owned())
            .or_insert_with(|| Arc::new(Kept::new(self.folder.as_deref(), &kept::file_for(key))));
        match promoting.filter(|id| held.holds(*id)) {
            Some(id) => {
                let mut saved = held.clone();
                saved.rules.retain(|rule| rule.id != id);
                file.save_soon(&saved);
            }
            None => {
                file.save_soon(held);
            }
        }
        Some(file.clone())
    }

    /// Deletes the file of a store that moved to another key.
    fn forget(&self, state: &mut State, key: &str) {
        if let Some(file) = state.files.remove(key) {
            file.flush();
        }
        let Some(folder) = &self.folder else {
            return;
        };
        let path = folder.join(kept::file_for(key));
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("sikemux core: {} was not removed: {error}", path.display());
            }
        }
    }
}

/// A store an earlier core kept by a folder's path, read as that project's.
fn keyed_by_store(mut value: Value) -> Value {
    if let Some(held) = value.as_object_mut() {
        if let Some(folder) = held.remove("folder") {
            held.insert("key".into(), folder);
            held.insert("store".into(), json!("project"));
        }
    }
    value
}

/// Keys each project's store by what the project is known by now, moving a store kept by its
/// folder, or by what it was known by before, to that key.
pub(crate) fn rekey(core: &Core, renamed: &[(String, String)]) {
    let keys: HashMap<String, String> = (core.workspaces().projects().into_iter())
        .map(|project| {
            let folder = project.path.to_string_lossy().into_owned();
            let key = project.identity.unwrap_or_else(|| folder.clone());
            (folder, key)
        })
        .collect();
    let memory = core.memory();
    let mut state = memory.state.locked();
    let renames: Vec<(String, String)> = (keys.iter())
        .filter(|(folder, key)| folder != key)
        .map(|(folder, key)| (folder.clone(), key.clone()))
        .chain(renamed.iter().cloned())
        .filter(|(old, new)| old != new)
        .collect();
    state.keys = keys;
    for (old, new) in &renames {
        if let Some(shared) = state.shared.remove(old) {
            state.shared.insert(new.clone(), shared);
        }
        if let Some(shut) = state.shut.remove(old) {
            state.shut.insert(new.clone(), shut);
        }
        let following: Vec<String> = (state.held.values_mut())
            .filter_map(|held| {
                let kept_for = (held.kept_for.as_mut()).filter(|kept| kept.local == *old)?;
                kept_for.local = new.clone();
                Some(held.key.clone())
            })
            .collect();
        for key in following {
            memory.save(&mut state, &key);
        }
    }
    let moves: Vec<(String, String)> = (renames.into_iter())
        .filter(|(old, _)| state.held.contains_key(old))
        .collect();
    if moves.is_empty() {
        return;
    }
    for (old, new) in moves {
        let Some(mut moving) = state.held.remove(&old) else {
            continue;
        };
        moving.was.insert(old.clone());
        match state.held.get_mut(&new) {
            Some(held) => held.take_in(moving),
            None => {
                moving.key = new.clone();
                state.held.insert(new.clone(), moving);
            }
        }
        let pointing: Vec<String> = (state.held.values_mut())
            .filter_map(|held| moving::rekeyed(held, &old, &new).then(|| held.key.clone()))
            .collect();
        for key in pointing.iter().chain([&new]) {
            memory.save(&mut state, key);
        }
        memory.forget(&mut state, &old);
    }
    drop(state);
    tell_clients(core, true);
}

impl crate::server::part::Part for Memory {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![
            crate::server::part::on_wake(core, |core| &core.memory().wake, woke),
            tokio::spawn(org::serve(core.clone())),
        ]
    }

    fn flush(&self) {
        let files: Vec<Arc<Kept>> = self.state.locked().files.values().cloned().collect();
        for file in files {
            file.flush();
        }
    }
}

/// Which projects are shared with which orgs may have changed.
pub(crate) fn shares_changed(core: &Core) {
    core.memory().org_sync.notify_one();
}

fn woke(core: &Arc<Core>) {
    crate::server::hosts::record::memory_changed(core, record::saved_since(core));
    settle(core);
}

/// Clients read what is kept again, and the inbox moves when knowledge went
/// stale or holds again.
fn tell_clients(core: &Core, stale_moved: bool) {
    core.hosts().copies.wake.now();
    core.changed(What::Read(Topic::Memory));
    core.changed(What::Read(Topic::Playbook));
    if stale_moved {
        core.changed(What::View);
    }
}

fn abouts(about: Vec<String>) -> Result<Vec<String>, String> {
    if about.len() > MAX_ABOUT {
        return Err(format!("about names at most {MAX_ABOUT} paths or services"));
    }
    let mut kept: Vec<String> = Vec::new();
    for item in about {
        let item = item.trim();
        if item.is_empty() || item.chars().count() > MAX_ABOUT_CHARS {
            return Err(format!(
                "each of about must be a path or a service's name, at most {MAX_ABOUT_CHARS} characters"
            ));
        }
        if !kept.iter().any(|known| known.eq_ignore_ascii_case(item)) {
            kept.push(item.to_owned());
        }
    }
    Ok(kept)
}

fn checked_check(check: MemoryCheck) -> Result<MemoryCheck, String> {
    let fits = |text: &str| !text.trim().is_empty() && text.chars().count() <= MAX_CHECK_CHARS;
    let fine = match &check {
        MemoryCheck::PathExists { path } => fits(path),
        MemoryCheck::HostAnswers { host, port } => fits(host) && *port > 0,
    };
    if fine {
        Ok(check)
    } else {
        Err(format!(
            "check needs a path, or a host and a port, of at most {MAX_CHECK_CHARS} characters"
        ))
    }
}

/// What the store under `key` keeps, kept from now on when it kept nothing yet.
fn holding<'a>(state: &'a mut State, key: &str, store: Store) -> Result<&'a mut Held, String> {
    let in_use = state
        .held
        .values()
        .filter(|held| state.in_use(held))
        .count();
    if !state.held.contains_key(key) && in_use >= MAX_STORES {
        return Err(format!(
            "Sikemux already keeps rules and knowledge for {MAX_STORES} projects, as many as it can"
        ));
    }
    let shared_with = (store == Store::Project)
        .then(|| state.shared.get(key).map(|shared| shared.org.clone()))
        .flatten();
    Ok((state.held.entry(key.to_owned())).or_insert_with(|| Held {
        shared_with,
        ..Held::new(key, store)
    }))
}

/// What the store under `key` keeps, or the person's own store once no other can be kept.
fn holding_or_personal<'a>(state: &'a mut State, key: &str, store: Store) -> &'a mut Held {
    let (key, store) = match key != PERSONAL && holding(state, key, store).is_ok() {
        true => (key, store),
        false => (PERSONAL, Store::Personal),
    };
    (state.held.entry(key.to_owned())).or_insert_with(|| Held::new(key, store))
}

/// Keeps a new entry in the store it names. Nothing goes to an org's store this way: an entry
/// reaches it only by the person's move.
fn add(core: &Core, project: &str, new: NewMemory, learned: Learned) -> Result<u64, String> {
    let learned = moving::named(core, learned);
    let (text, store) = match &new {
        NewMemory::Rule { text, store, .. } | NewMemory::Knowledge { text, store, .. } => {
            (to_keep(text, "text", MAX_TEXT_CHARS)?, *store)
        }
    };
    if store == Store::Org {
        return Err(
            "A new rule can't go straight to the org's rules: add it to the project, \
            then move it there"
                .into(),
        );
    }
    let mut state = core.memory().state.locked();
    let key = state.key_for(project, store);
    if key.is_empty() {
        return Err(NOT_SHARED.into());
    }
    let held = holding(&mut state, &key, store)?;
    if held.len() >= MAX_ENTRIES {
        return Err(format!(
            "That store already keeps {MAX_ENTRIES} rules and knowledge; retire some first"
        ));
    }
    let known = match &new {
        NewMemory::Rule { .. } => (held.rules.iter())
            .find(|rule| rule.text.eq_ignore_ascii_case(&text))
            .map(|rule| rule.id),
        NewMemory::Knowledge { .. } => (held.knowledge.iter())
            .find(|known| known.text.eq_ignore_ascii_case(&text))
            .map(|known| known.id),
    };
    if let Some(id) = known {
        return Err(format!("Sikemux already keeps that, as #{id}"));
    }
    let id = state.next_id(&key);
    let held = state
        .held
        .get_mut(&key)
        .ok_or("the store went while it was being written to")?;
    match new {
        NewMemory::Rule { sensitive, .. } => {
            held.rules.push(MemoryRule {
                id,
                text,
                sensitive,
                learned,
                was: None,
                moved: None,
            });
        }
        NewMemory::Knowledge {
            about,
            sensitive,
            check,
            ..
        } => held.knowledge.push(Knowledge {
            id,
            text,
            about: abouts(about)?,
            sensitive,
            check: check.map(checked_check).transpose()?,
            learned,
            was: None,
            checked: None,
            stale: None,
            moved: None,
        }),
    }
    if let Some(full) = org::too_big(held) {
        held.rules.retain(|rule| rule.id != id);
        held.knowledge.retain(|known| known.id != id);
        return Err(full);
    }
    core.memory().save(&mut state, &key);
    drop(state);
    tell_clients(core, false);
    Ok(id)
}

/// Keeps a rule or knowledge, or a goal's rule on its card; answers what
/// happened, in words.
fn keep(core: &Core, project: &str, new: NewMemory, learned: Learned) -> Result<String, String> {
    match new {
        NewMemory::Rule {
            text,
            goal: Some(card),
            sensitive,
            store,
        } => {
            if sensitive || store == Store::Personal {
                return Err("A goal's rule shows on its card and belongs to its project; keep sensitive or personal rules without goal".into());
            }
            let rule = goal::Change::of(goal::List::Rules, None, Some(&text), None, false)?;
            goal::change(core, project, Some(card), goal::List::Rules, rule, learned)
        }
        new => add(core, project, new, learned).map(|id| format!("Kept as #{id}.")),
    }
}

fn amend(
    core: &Core,
    project: &str,
    update: MemoryUpdateParams,
    by: Learned,
) -> Result<(), String> {
    if let Some(store) = update.store {
        return moving::move_to(core, project, update.id, store, by);
    }
    if update.not_now {
        return moving::not_now(core, project, update.id);
    }
    let Some(card) = update.goal else {
        return change(core, project, update, by);
    };
    if update.wrong.is_some()
        || update.about.is_some()
        || update.check.is_some()
        || update.drop_check
    {
        return Err(format!(
            "#{} is a goal's rule: correct it with text, or retire it",
            update.id
        ));
    }
    let rule = goal::Change::of(
        goal::List::Rules,
        Some(update.id),
        update.text.as_deref(),
        None,
        update.retire,
    )?;
    goal::change(core, project, Some(card), goal::List::Rules, rule, by).map(drop)
}

fn change(
    core: &Core,
    project: &str,
    update: MemoryUpdateParams,
    by: Learned,
) -> Result<(), String> {
    let saying = usize::from(update.text.is_some())
        + usize::from(update.wrong.is_some())
        + usize::from(update.retire);
    if saying > 1 {
        return Err("Say one of text, to correct it, wrong, when it no longer holds and you do not know what does, or retire".into());
    }
    if update.check.is_some() && update.drop_check {
        return Err("Say check or dropCheck, not both".into());
    }
    let text = update
        .text
        .as_deref()
        .map(|text| to_keep(text, "text", MAX_TEXT_CHARS))
        .transpose()?;
    let wrong = update
        .wrong
        .as_deref()
        .map(|why| to_keep(why, "wrong", MAX_TEXT_CHARS))
        .transpose()?;
    let about = update.about.map(abouts).transpose()?;
    let check = update.check.map(checked_check).transpose()?;
    let id = update.id;
    {
        let state = core.memory().state.locked();
        let refused = (state.key_holding(project, id).ok())
            .and_then(|key| org::refused(&state, project, &key, by.agent.is_some()));
        if let Some(refused) = refused {
            return Err(refused);
        }
    }
    let only_id = saying == 0 && about.is_none() && check.is_none() && !update.drop_check;
    if now_check::is_set_aside(core, project, id) {
        return match only_id {
            true => now_check::bring_back_asked(core, project, id, &by),
            false => Err(format!("#{id} is set aside: {BRING_BACK}")),
        };
    }
    let only_wrong = saying == 1 && about.is_none() && check.is_none() && !update.drop_check;
    if let Some(said) = wrong
        .as_deref()
        .filter(|_| only_wrong && by.agent.is_some())
    {
        let is_rule = {
            let state = core.memory().state.locked();
            let found =
                (state.applying(project)).any(|held| held.rules.iter().any(|rule| rule.id == id));
            found
        };
        if let Some(done) = is_rule
            .then(|| now_check::labelled(core, project, id, said, &by))
            .flatten()
        {
            return done;
        }
    }
    let acting = (by.agent.clone()).filter(|_| update.retire || text.is_some());
    let persons_turn =
        (acting.as_deref()).is_some_and(|agent| now_check::persons_turn(core, agent));
    let overruled = acting.is_some() && crate::server::chief::kept_by_overrule(core, project, id);
    let new_words_persons = match (acting.as_deref(), text.as_deref()) {
        (Some(agent), Some(text)) => said_by_person(core, agent, text),
        _ => false,
    };
    let mut state = core.memory().state.locked();
    if state.promoting == Some(id) {
        return Err(format!("#{id} is being written into AGENTS.md"));
    }
    let key = state.key_holding(project, id)?;
    let persons_rule = (state.held.get(&key))
        .and_then(|held| held.rules.iter().find(|rule| rule.id == id))
        .filter(|rule| rule.learned.agent.is_none());
    if let Some(rule) = persons_rule.filter(|_| acting.is_some()) {
        now_check::agent_may_change(rule, overruled, persons_turn)?;
    }
    let note_id =
        (acting.is_some() && persons_rule.is_some() && text.is_some() && !new_words_persons)
            .then(|| state.next_id(&key));
    let held = state
        .held
        .get_mut(&key)
        .ok_or("the store went while it was being changed")?;
    let store = held.store;
    let mut stale_moved = false;
    let (mut line, mut back, mut undone_already, mut removed_from) = (None, None, None, None);
    if let Some(rule) = held.rules.iter_mut().find(|rule| rule.id == id) {
        let rule_change =
            wrong.is_none() && about.is_none() && check.is_none() && !update.drop_check;
        if !rule_change || saying == 0 {
            return Err(format!(
                "#{id} is a rule: correct it with text, or retire it"
            ));
        }
        let agent = (by.agent.clone()).filter(|_| rule.learned.agent.is_none());
        match (text, agent) {
            (Some(text), Some(agent)) if note_id.is_none() => {
                let was = std::mem::replace(&mut rule.text, text);
                let mut learned = by;
                learned.with = (learned.agent.take()).map(|agent| sikemux_view::memory::SaidTo {
                    agent,
                    name: learned.name.take(),
                });
                rule.learned = moving::reworded(&rule.learned, learned);
                line = Some((
                    now_check::line(core, project, store, rule, &agent, None, Some(was.clone())),
                    agent,
                ));
                rule.was = Some(was);
            }
            (Some(text), Some(agent)) => {
                let at = (held.rules.iter().position(|rule| rule.id == id))
                    .ok_or("the rule went while it was being changed")?;
                let rule = held.rules.remove(at);
                line = Some((
                    now_check::line(core, project, store, &rule, &agent, None, None),
                    agent.clone(),
                ));
                held.set_aside.push(now_check::SetAside {
                    rule,
                    commit: None,
                    at: sikemux_process::clock::epoch_ms(),
                    by: agent,
                });
                held.rules.push(MemoryRule {
                    id: note_id.unwrap_or(id),
                    text,
                    sensitive: false,
                    learned: by,
                    was: None,
                    moved: None,
                });
            }
            (Some(text), None) => {
                let flat = crate::text::one_line(&text);
                let put_back = (rule.was.clone()).filter(|was| crate::text::one_line(was) == flat);
                let was = std::mem::replace(&mut rule.text, put_back.clone().unwrap_or(text));
                let changed_by = (rule.learned.with.as_ref()).map(|with| with.agent.clone());
                if put_back.is_none() && flat == crate::text::one_line(&was) {
                    undone_already = Some(flat);
                }
                back = put_back.and(changed_by);
                rule.learned = moving::reworded(&rule.learned, by);
                rule.was = Some(was);
            }
            (None, agent) => {
                let at = (held.rules.iter().position(|rule| rule.id == id))
                    .ok_or("the rule went while it was being changed")?;
                let rule = held.rules.remove(at);
                let kept_in_chat = (rule.learned.with.as_ref())
                    .filter(|_| by.agent.is_none() && rule.was.is_none())
                    .map(|with| with.agent.clone());
                if let Some(agent) = agent {
                    line = Some((
                        now_check::line(core, project, store, &rule, &agent, None, None),
                        agent.clone(),
                    ));
                    held.set_aside.push(now_check::SetAside {
                        rule,
                        commit: None,
                        at: sikemux_process::clock::epoch_ms(),
                        by: agent,
                    });
                } else if let Some(agent) = kept_in_chat {
                    held.removed
                        .push(removed::Removed::of(&agent, project, &rule));
                    removed_from = Some(agent);
                }
            }
        }
    } else if let Some(known) = held.knowledge.iter_mut().find(|known| known.id == id) {
        if saying == 0 && about.is_none() && check.is_none() && !update.drop_check {
            return Err("Say what changes: text, wrong, retire, about, check or dropCheck".into());
        }
        let was_stale = known.stale.is_some();
        if let Some(text) = text {
            known.was = Some(std::mem::replace(&mut known.text, text));
            known.learned = moving::reworded(&known.learned, by.clone());
            known.stale = None;
            known.checked = None;
        }
        if let Some(why) = wrong {
            known.stale = Some(Stale::Disputed { why, by });
        }
        if let Some(about) = about {
            known.about = about;
        }
        if check.is_some() || update.drop_check {
            if matches!(known.stale, Some(Stale::Failed { .. })) {
                known.stale = None;
            }
            known.check = check;
            known.checked = None;
        }
        let is_stale = known.stale.is_some() && !update.retire;
        stale_moved = was_stale != is_stale;
        if update.retire {
            held.knowledge.retain(|known| known.id != id);
        }
    }
    if update.retire {
        state.wanted.remove(&id);
    }
    if let Some(held) = state.held.get_mut(&key) {
        waiting::recheck(held);
    }
    core.memory().save(&mut state, &key);
    drop(state);
    tell_clients(core, stale_moved);
    if let Some((line, agent)) = line {
        crate::server::chief::rule_changed(core, line, &agent);
    }
    if let Some(agent) = removed_from {
        removed::show(core, &agent);
    }
    if let Some(agent) = back {
        let rule = (core.memory().state.locked().held.get(&key))
            .and_then(|held| held.rules.iter().find(|rule| rule.id == id).cloned());
        if let Some(rule) = rule {
            let mut line = now_check::line(core, project, store, &rule, &agent, None, None);
            line.restored = true;
            crate::server::chief::rule_back(core, line, &agent);
        }
    }
    if let Some(words) = undone_already {
        crate::server::chief::rule_lines_back(core, project, |line| {
            line.rule == id && line.was.as_deref() == Some(words.as_str())
        });
    }
    Ok(())
}

/// What applies to `project`, with sensitive entries left out unless Sikemux's
/// own window reads it.
fn view(core: &Core, project: &str, peer: &Peer) -> MemoryView {
    let shows_sensitive = matches!(peer, Peer::App);
    let file = promote::agents_file(project);
    let state = core.memory().state.locked();
    let stores = state
        .applying(project)
        .map(|held| {
            let rules: Vec<ListedRule> = (held.rules.iter())
                .filter(|rule| shows_sensitive || !rule.sensitive)
                .map(|rule| ListedRule {
                    stays: promote::stays(rule, held.store, &file),
                    rule: rule.clone(),
                })
                .collect();
            let knowledge: Vec<Knowledge> = (held.knowledge.iter())
                .filter(|known| shows_sensitive || !known.sensitive)
                .cloned()
                .collect();
            StoreMemory {
                store: held.store,
                hidden: held.len() - rules.len() - knowledge.len(),
                rules,
                knowledge,
            }
        })
        .collect();
    let seeding = state.own(project).and_then(|held| held.seeding.clone());
    MemoryView { stores, seeding }
}

/// What the Playbook lists for `project`: the person's rules, then what agents noted and the
/// knowledge kept, the project's store first. Sensitive entries show only in Sikemux's own window.
pub(crate) fn for_playbook(core: &Core, project: &str, peer: &Peer) -> (Vec<Entry>, Vec<Entry>) {
    let shows_sensitive = matches!(peer, Peer::App);
    let me = moving::first_name(core);
    let someone = |learned: &Learned| {
        (learned.person.clone()).filter(|person| me.as_ref().is_some_and(|me| me != person))
    };
    let state = core.memory().state.locked();
    let offers = moving::Offers::new(&state, project);
    let shared = state.shared.get(&state.key_of(project));
    let shut = (state.shut.get(&state.key_of(project))).filter(|_| shared.is_none());
    let org = (shared.map(|shared| &shared.name)).or(shut).cloned();
    let (mut rules, mut notes) = (Vec::new(), Vec::new());
    for held in state.applying(project) {
        let came_from = state.key_for(project, held.store.other());
        let mine = |id: u64| {
            held.store != Store::Org
                || shared.is_some_and(|shared| held.writers.get(&id) == Some(&shared.member))
        };
        let someone = |id: u64, learned: &Learned| match (held.store, shared) {
            (Store::Org, Some(shared)) => (held.writers.get(&id))
                .filter(|writer| **writer != shared.member)
                .and_then(|writer| shared.members.get(writer).cloned().flatten()),
            _ => someone(learned),
        };
        let teammate = |id: u64| match (held.store, shared) {
            (Store::Org, Some(shared)) => (held.writers.get(&id))
                .filter(|writer| **writer != shared.member)
                .map(|writer| shared.members.contains_key(writer)),
            _ => None,
        };
        let undoable = |id: u64, moved: &Option<sikemux_view::memory::Moved>| {
            (moving::back(&state, moved, &came_from) && mine(id))
                .then(|| moved.as_ref().map(|moved| moved.by.clone()))
                .flatten()
        };
        let org_of = |id: u64| {
            (org.clone())
                .filter(|_| held.store != Store::Personal || held.refused.contains_key(&id))
        };
        let org = org.clone().filter(|_| held.store != Store::Personal);
        let local = held.store == Store::Project && org.is_some();
        let asks = local && shared.is_some_and(|shared| !shared.writes);
        let refused = |id: u64, sensitive: bool, text: &str| {
            let why = (held.refused.get(&id).filter(|_| shared.is_some()).cloned()).or_else(|| {
                (asks && waiting::Waiting::is_sensitive(sensitive, text))
                    .then(|| waiting::SENSITIVE.to_owned())
            });
            match why {
                Some(why) if sensitive && why == waiting::SENSITIVE => {
                    Some(waiting::SENSITIVE_ROW.to_owned())
                }
                why => why,
            }
        };
        let waiting = |id: u64| local && held.waiting.contains(&id);
        let dropped = |id: u64| (held.dropped.get(&id)).filter(|_| local).cloned();
        for rule in held
            .rules
            .iter()
            .filter(|rule| shows_sensitive || !rule.sensitive)
        {
            let kept = Entry {
                id: rule.id,
                text: rule.text.clone(),
                learned: rule.learned.clone(),
                knowledge: false,
                stale: false,
                sensitive: rule.sensitive,
                store: held.store,
                moved: undoable(rule.id, &rule.moved),
                someone: someone(rule.id, &rule.learned),
                theirs: teammate(rule.id).is_some(),
                left: teammate(rule.id) == Some(false),
                offer: (offers.for_rule(held, rule)).filter(|_| !waiting(rule.id)),
                org: org_of(rule.id),
                waiting: waiting(rule.id),
                dropped: dropped(rule.id),
                refused: refused(rule.id, rule.sensitive, &rule.text),
                asks,
                shut: local && shut.is_some(),
            };
            match rule.learned.agent {
                None => rules.push(kept),
                Some(_) => notes.push(kept),
            }
        }
        for known in held
            .knowledge
            .iter()
            .filter(|known| shows_sensitive || !known.sensitive)
        {
            notes.push(Entry {
                id: known.id,
                text: known.text.clone(),
                learned: known.learned.clone(),
                knowledge: true,
                stale: known.stale.is_some(),
                sensitive: known.sensitive,
                store: held.store,
                moved: undoable(known.id, &known.moved),
                someone: someone(known.id, &known.learned),
                theirs: teammate(known.id).is_some(),
                left: teammate(known.id) == Some(false),
                offer: None,
                org: org_of(known.id),
                waiting: waiting(known.id),
                dropped: dropped(known.id),
                refused: refused(known.id, known.sensitive, &known.text),
                asks,
                shut: local && shut.is_some(),
            });
        }
    }
    let newest = |kept: &Entry| std::cmp::Reverse(kept.learned.at);
    rules.sort_by_key(newest);
    notes.sort_by_key(newest);
    (rules, notes)
}

/// Stale knowledge, for the inbox.
pub(crate) struct StaleKnowledge {
    /// A folder of a project the store applies to.
    pub project: Option<PathBuf>,
    pub id: u64,
    /// What it says and why it no longer holds; none for sensitive
    /// knowledge, whose words the inbox never shows.
    pub said: Option<(String, String)>,
    pub at: u64,
    /// The agent that said it no longer holds.
    pub agent: Option<String>,
}

/// The first rule that applies to `project` and names `word`, in its own words.
pub(crate) fn rule_about(core: &Core, project: &str, word: &str) -> Option<String> {
    let names = |rule: &&MemoryRule| {
        (rule.text.to_lowercase().split(|c: char| !c.is_alphabetic())).any(|said| said == word)
    };
    let state = core.memory().state.locked();
    let rule = state
        .applying(project)
        .find_map(|held| held.rules.iter().find(names).map(|rule| rule.text.clone()));
    rule
}

pub(crate) fn stale(core: &Core) -> Vec<StaleKnowledge> {
    let state = core.memory().state.locked();
    let mut listed = Vec::new();
    for held in state.held.values() {
        for known in &held.knowledge {
            let (why, at, agent) = match &known.stale {
                None => continue,
                Some(Stale::Failed { since, why }) => (why.clone(), *since, None),
                Some(Stale::Disputed { why, by }) => (why.clone(), by.at, by.agent.clone()),
            };
            let project = match held.store {
                Store::Project | Store::Org => match state.folder_of_store(held) {
                    Some(folder) => Some(folder),
                    None => continue,
                },
                Store::Personal => state.keys.keys().next().map(PathBuf::from),
            };
            listed.push(StaleKnowledge {
                project,
                id: known.id,
                said: (!known.sensitive).then(|| (known.text.clone(), why)),
                at,
                agent,
            });
        }
    }
    listed
}

fn params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, String> {
    serde_json::from_value(params).map_err(|error| error.to_string())
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ToolAdd {
    Rule {
        text: String,
        #[serde(default)]
        goal: bool,
        #[serde(default)]
        sensitive: bool,
        #[serde(default)]
        yours: bool,
        #[serde(default)]
        person: bool,
    },
    Knowledge {
        text: String,
        #[serde(default)]
        about: Vec<String>,
        #[serde(default)]
        sensitive: bool,
        check: Option<MemoryCheck>,
        #[serde(default)]
        yours: bool,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolUpdate {
    id: u64,
    #[serde(default)]
    goal: bool,
    text: Option<String>,
    wrong: Option<String>,
    #[serde(default)]
    retire: bool,
    about: Option<Vec<String>>,
    check: Option<MemoryCheck>,
    #[serde(default)]
    drop_check: bool,
}

/// The goal card the agent's live card sits under.
fn goal_card(core: &Core, agent: &str) -> Result<u64, String> {
    board::goal_of(core, agent)
        .map(|goal| goal.card)
        .ok_or_else(|| {
            "You work on no card that is not done, so there is no goal for the rule".into()
        })
}

fn store_of(yours: bool) -> Store {
    match yours {
        true => Store::Personal,
        false => Store::Project,
    }
}

/// `memory_add`: a rule or knowledge for the agent's project, or a rule for
/// the goal its live card sits under. A rule the person said in the message that started the
/// turn is kept as theirs, and the chat says so with one line they can undo.
pub(crate) async fn tool_add(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "memory_add")?;
    let adding: ToolAdd = params(request.params.clone())?;
    let project = crate::server::caller::project_of(core, request);
    let core = core.clone();
    run(move || {
        let mut by = learned(&core, Some(&agent));
        let (new, said, paths) = match adding {
            ToolAdd::Rule {
                text,
                person: true,
                goal: false,
                sensitive,
                yours,
            } if said_by_person(&core, &agent, &text) => {
                by.with = (by.agent.take()).map(|agent| sikemux_view::memory::SaidTo {
                    agent,
                    name: by.name.take(),
                });
                let said = text.clone();
                let id = add(
                    &core,
                    &project,
                    NewMemory::Rule {
                        text,
                        goal: None,
                        sensitive,
                        store: store_of(yours),
                    },
                    by,
                )?;
                let note = format!("Kept as #{id}, as the person's rule.");
                let note = json!(with_check_asked(note, &said, &[]));
                let words = crate::text::one_line(&said);
                if say_kept(&core, &agent, &project, id, &words) {
                    return Ok(json!({ "note": note }));
                }
                return Ok(json!({ "note": note, KEPT_ELSEWHERE: { "id": id, "words": words } }));
            }
            ToolAdd::Rule {
                text,
                goal,
                sensitive,
                yours,
                ..
            } => {
                let goal = goal.then(|| goal_card(&core, &agent)).transpose()?;
                let said = text.clone();
                let rule = NewMemory::Rule {
                    text,
                    goal,
                    sensitive,
                    store: store_of(yours),
                };
                (rule, said, Vec::new())
            }
            ToolAdd::Knowledge {
                text,
                about,
                sensitive,
                check,
                yours,
            } => {
                let (said, paths) = (text.clone(), about.clone());
                let knowledge = NewMemory::Knowledge {
                    text,
                    about,
                    sensitive,
                    check,
                    store: store_of(yours),
                };
                (knowledge, said, paths)
            }
        };
        let note = keep(&core, &project, new, by)?;
        Ok(json!({ "note": with_check_asked(note, &said, &paths) }))
    })
    .await
}

/// `note` with, when the memory is about code, a request for the check that
/// would hold it: a note only an agent reads stops no one from breaking it.
fn with_check_asked(note: String, text: &str, about: &[String]) -> String {
    if !about_code(text, about) {
        return note;
    }
    let asked = "This is about code, so a check should hold it too, not only this note: add a lint (such as a Clippy deny or a disallowed method) or a `landChecks` entry in sikemux.json, which the land queue runs on every land touching its paths, and land it. If no check can catch it, say why in your answer.";
    match note.is_empty() {
        true => asked.to_owned(),
        false => format!("{note} {asked}"),
    }
}

/// Whether a memory names source files or code by its words.
fn about_code(text: &str, about: &[String]) -> bool {
    const SOURCES: [&str; 9] = [
        ".rs", ".ts", ".tsx", ".js", ".swift", ".kt", ".py", ".go", ".toml",
    ];
    const WORDS: [&str; 10] = [
        "`", "::", "()", "crate", "function", "method", "clippy", "compile", "struct", " fn ",
    ];
    let lower = text.to_lowercase();
    let names_source = |word: &str| SOURCES.iter().any(|ext| word.contains(ext));
    about.iter().any(|path| names_source(path))
        || lower.split_whitespace().any(names_source)
        || WORDS.iter().any(|word| lower.contains(word))
}

/// Whether what the person said in this turn, as the chat's record has it, holds most of the
/// rule's longer words, so an agent's own idea, or a message another agent or a trigger sent,
/// is kept as an agent's note rather than as the person's rule.
fn said_by_person(core: &Core, agent: &str, rule: &str) -> bool {
    let said = person_said(core, agent);
    let words = |text: &str| -> HashSet<String> {
        (text.split(|c: char| !c.is_alphanumeric()))
            .filter(|word| word.chars().count() >= 4)
            .map(str::to_lowercase)
            .collect()
    };
    let (said, rule) = (words(&said.join("\n")), words(rule));
    let held = rule.iter().filter(|word| said.contains(*word)).count();
    !rule.is_empty() && held * 2 >= rule.len()
}

/// What the person said in `agent`'s turn now, read from its chat, or from the linked host it
/// runs on while this host does its write.
pub(crate) fn person_said(core: &Core, agent: &str) -> Vec<String> {
    match core.chats().get(agent) {
        Some(chat) => chat.feed.person_said(),
        None => (core.memory().said_elsewhere.locked().get(agent).cloned()).unwrap_or_default(),
    }
}

/// Holds what the person said in `agent`'s turn on a linked host while this host does its write.
pub(crate) struct SaidElsewhere<'a> {
    core: &'a Core,
    agent: String,
}

pub(crate) fn said_elsewhere<'a>(
    core: &'a Core,
    agent: &str,
    said: Vec<String>,
) -> SaidElsewhere<'a> {
    (core.memory().said_elsewhere.locked()).insert(agent.to_owned(), said);
    SaidElsewhere {
        core,
        agent: agent.to_owned(),
    }
}

impl Drop for SaidElsewhere<'_> {
    fn drop(&mut self) {
        (self.core.memory().said_elsewhere.locked()).remove(&self.agent);
    }
}

/// Says in `agent`'s chat on this host that the rule its home kept from the person's words is
/// kept, when the home's `answer` names one.
pub(crate) fn kept_at_home(core: &Core, agent: &str, project: &str, answer: &mut Value) {
    let Some(kept) = (answer.as_object_mut()).and_then(|answer| answer.remove(KEPT_ELSEWHERE))
    else {
        return;
    };
    if let (Some(id), Some(words)) = (
        kept.get("id").and_then(Value::as_u64),
        kept.get("words").and_then(Value::as_str),
    ) {
        say_kept(core, agent, project, id, words);
    }
}

const KEPT_ELSEWHERE: &str = "keptForChat";

/// The line in the agent's chat that says rule `id` was kept from the person's words; answers
/// whether the chat is on this host.
fn say_kept(core: &Core, agent: &str, project: &str, id: u64, words: &str) -> bool {
    let Some(chat) = core.chats().get(agent) else {
        return false;
    };
    let Some(start) = chat.feed.start() else {
        return true;
    };
    let (org, waiting) = kept_for_org(core, project, id);
    let rule = sikemux_view::chat::KeptRule {
        id,
        project: project.to_owned(),
        words: words.to_owned(),
        undone: false,
        org,
        waiting,
    };
    chat.emit(
        sikemux_fold::ChatEventKind::SessionUpdate,
        json!({
            "sessionId": start.session_id,
            "update": sikemux_fold::update::rule_notice(rule),
        }),
    );
    true
}

/// The org whose store rule `id` of `project` went to, and whether it waits there for an
/// owner or admin; none when it stayed on this host.
pub(crate) fn kept_for_org(core: &Core, project: &str, id: u64) -> (Option<String>, bool) {
    let state = core.memory().state.locked();
    let shared = state.shared.get(&state.key_of(project));
    let waiting = (state.own(project)).is_some_and(|held| held.waiting.contains(&id));
    let holds = |shared: &&org::Shared| {
        waiting || (state.held.get(&shared.key())).is_some_and(|held| held.holds(id))
    };
    (
        shared.filter(holds).map(|shared| shared.name.clone()),
        waiting,
    )
}

/// `memory_update`: corrects, disputes or retires an entry, or changes what
/// knowledge is about and how it is tested.
pub(crate) async fn tool_update(
    core: &Arc<Core>,
    request: &HarnessRequest,
) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "memory_update")?;
    let given: ToolUpdate = params(request.params.clone())?;
    let project = crate::server::caller::project_of(core, request);
    let id = given.id;
    let said_again = (given.wrong.as_deref().map(str::trim_start))
        .and_then(|said| said.get(..10))
        .is_some_and(|label| label.eq_ignore_ascii_case("said again"));
    let core = core.clone();
    run(move || {
        renumbered_once(&core, &agent, &project, id)?;
        let corrected = (given.text.as_deref())
            .map(|text| with_check_asked(String::new(), text, given.about.as_deref().unwrap_or_default()))
            .filter(|asked| !asked.is_empty());
        let update = MemoryUpdateParams {
            goal: given.goal.then(|| goal_card(&core, &agent)).transpose()?,
            store: None,
            not_now: false,
            kept: false,
            words: None,
            project: project.clone(),
            id: given.id,
            text: given.text,
            wrong: given.wrong,
            retire: given.retire,
            about: given.about,
            check: given.check,
            drop_check: given.drop_check,
        };
        amend(&core, &project, update, learned(&core, Some(&agent)))?;
        Ok(match (said_again, corrected) {
            (true, _) => json!({ "id": id, "note": format!("Counted #{id} as said again; the rule is unchanged") }),
            (false, Some(asked)) => json!({ "id": id, "note": asked }),
            (false, None) => json!({ "id": id }),
        })
    })
    .await
}

/// Refuses, the first time this agent names it, a number that once named an entry since renumbered
/// and now names another one, since the agent may mean either.
fn renumbered_once(core: &Core, agent: &str, project: &str, id: u64) -> Result<(), String> {
    let state = core.memory().state.locked();
    let moved = (state.applying(project)).find_map(|held| held.renumbered.get(&id).copied());
    let holder = state.applying(project).find(|held| held.holds(id));
    let (Some(now), Some(holder)) = (moved, holder) else {
        return Ok(());
    };
    if !(core.memory().told_renumbered.locked()).insert((agent.to_owned(), id)) {
        return Ok(());
    }
    let words = holder.words(id).unwrap_or_default();
    let from = match holder.store {
        Store::Org => (state.shared.get(&state.key_of(project))).map_or_else(
            || "the org's rules".to_owned(),
            |shared| shared.name.clone(),
        ),
        Store::Project => "this project".to_owned(),
        Store::Personal => "your own rules".to_owned(),
    };
    let was = match state.applying(project).find_map(|held| held.words(now)) {
        Some(was) => format!("#{id} was \"{was}\", which is now #{now}."),
        None => format!("#{id} was renumbered to #{now}, which is no longer kept."),
    };
    Err(format!(
        "{was} #{id} is now \"{words}\" from {from}. Send it again with the number you mean."
    ))
}

fn said_by(learned: &Learned) -> &str {
    learned.name.as_deref().unwrap_or("the person")
}

/// `memory_list`: every rule, piece of knowledge and decision kept for the
/// agent's project, with the rules of the goal its live card sits under.
pub(crate) async fn tool_list(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "memory_list")?;
    let project = crate::server::caller::project_of(core, request);
    let goal = board::goal_of(core, &agent).map(|goal| (goal.card, goal::rules(core, &goal)));
    let decisions = goal::standing(core, &project);
    let state = core.memory().state.locked();
    let (mut rules, mut knowledge) = (Vec::new(), Vec::new());
    for held in state.applying(&project) {
        let yours = held.store == Store::Personal;
        for rule in &held.rules {
            rules.push(json!({
                "id": rule.id,
                "text": rule.text,
                "from": said_by(&rule.learned),
                "fromId": rule.learned.agent,
                "at": rule.learned.at,
                "yours": yours,
                "sensitive": rule.sensitive,
            }));
        }
        for known in &held.knowledge {
            let stale = known.stale.as_ref().map(|stale| match stale {
                Stale::Failed { why, .. } => why.clone(),
                Stale::Disputed { why, by } => format!("{} said: {why}", said_by(by)),
            });
            knowledge.push(json!({
                "id": known.id,
                "text": known.text,
                "about": known.about,
                "from": said_by(&known.learned),
                "fromId": known.learned.agent,
                "at": known.learned.at,
                "check": known.check,
                "stale": stale,
                "yours": yours,
                "sensitive": known.sensitive,
            }));
        }
    }
    drop(state);
    let set_aside: Vec<Value> = (now_check::set_aside(core, &project).into_iter())
        .map(|apart| {
            json!({
                "id": apart.rule.id,
                "text": apart.rule.text,
                "sensitive": apart.rule.sensitive,
                "setAsideBy": core.names().name_or_id(&apart.by),
                "at": apart.at,
                "why": now_check::why(&apart),
            })
        })
        .collect();
    let mut listed = json!({
        "rules": rules,
        "knowledge": knowledge,
        "decisions": decisions,
        "goal": goal.map(|(card, rules)| json!({ "card": card, "rules": rules })),
    });
    if let Some(fields) = listed.as_object_mut().filter(|_| !set_aside.is_empty()) {
        fields.insert("setAside".to_owned(), json!(set_aside));
        fields.insert("setAsideNote".to_owned(), json!(BRING_BACK));
    }
    Ok(listed)
}

const BRING_BACK: &str =
    "memory_update with only a set-aside rule's id brings it back, when the person asks for it";

async fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    blocking(move || work().map_err(Into::into))
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn person_view(
    core: &Arc<Core>,
    peer: Peer,
    params: MemoryParams,
) -> Result<MemoryView, String> {
    let project = project_at(core, &params.project)?;
    let core = core.clone();
    run(move || Ok(view(&core, &project, &peer))).await
}

/// Keeps a rule of the project's as the person's, as an Overrule makes one, and answers its id.
pub(crate) fn keep_persons_rule(core: &Core, project: &str, text: String) -> Result<u64, String> {
    let rule = NewMemory::Rule {
        text,
        goal: None,
        sensitive: false,
        store: Store::Project,
    };
    add(core, project, rule, learned(core, None))
}

/// Retires a rule of the project's in the person's name, as undoing an Overrule does, when what
/// has its number now still has `words`.
pub(crate) fn retire_persons_rule(
    core: &Core,
    project: &str,
    id: u64,
    words: &str,
) -> Result<(), String> {
    let id = line_entry(core, project, id, Some(words))?;
    let update = MemoryUpdateParams {
        project: project.to_owned(),
        id,
        text: None,
        wrong: None,
        retire: true,
        about: None,
        check: None,
        drop_check: false,
        goal: None,
        store: None,
        not_now: false,
        kept: false,
        words: None,
    };
    change(core, project, update, learned(core, None))
}

/// Whether an entry applying to the project still has the number a chat line's entry `id` has now.
pub(crate) fn still_held(core: &Core, project: &str, id: u64) -> bool {
    let state = core.memory().state.locked();
    let id = state.kept_id(project, id);
    let held = state.applying(project).any(|held| held.holds(id));
    held
}

pub(crate) async fn person_add(
    core: &Arc<Core>,
    peer: Peer,
    params: MemoryAddParams,
) -> Result<MemoryView, String> {
    let project = project_at(core, &params.project)?;
    let core = core.clone();
    run(move || {
        let set_aside = match &params.entry {
            NewMemory::Rule {
                text,
                goal: None,
                sensitive,
                ..
            } => now_check::bring_back_words(&core, &project, text, *sensitive),
            _ => None,
        };
        match set_aside {
            Some(brought) => brought?,
            None if now_check::kept_again(&core, &project, &params.entry) => {}
            None => {
                keep(&core, &project, params.entry, learned(&core, None))?;
            }
        }
        Ok(view(&core, &project, &peer))
    })
    .await
}

/// The number a chat line's entry has now, past any renumbering since the line was written.
/// Refused when the line's `words` were sent and no entry under that number has them.
pub(crate) fn line_entry(
    core: &Core,
    project: &str,
    id: u64,
    words: Option<&str>,
) -> Result<u64, String> {
    let state = core.memory().state.locked();
    let id = state.kept_id(project, id);
    let Some(words) = words.map(crate::text::one_line) else {
        return Ok(id);
    };
    let same = !words.is_empty()
        && state.applying(project).any(|held| {
            (held.rules.iter().map(|rule| (rule.id, &rule.text)))
                .chain(held.knowledge.iter().map(|known| (known.id, &known.text)))
                .any(|(at, text)| at == id && crate::text::one_line(text) == words)
        });
    match same {
        true => Ok(id),
        false => Err("That has changed since, so nothing was undone".to_owned()),
    }
}

pub(crate) async fn person_update(
    core: &Arc<Core>,
    peer: Peer,
    mut params: MemoryUpdateParams,
) -> Result<MemoryView, String> {
    let project = project_at(core, &params.project)?;
    let core = core.clone();
    run(move || {
        if params.kept {
            params.id = line_entry(&core, &project, params.id, params.words.as_deref())?;
        }
        amend(&core, &project, params, learned(&core, None))?;
        Ok(view(&core, &project, &peer))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_memory_add_offers_is_one_it_reads() {
        let tools: Value = serde_json::from_str(crate::cli::manifest::TOOLS).unwrap();
        let add = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "memory_add")
            .unwrap();
        let kinds = add["properties"]["kind"]["enum"].as_array().unwrap();
        assert!(!kinds.is_empty());
        for kind in kinds {
            params::<ToolAdd>(json!({ "kind": kind, "text": "kept" })).unwrap();
        }
    }
}
