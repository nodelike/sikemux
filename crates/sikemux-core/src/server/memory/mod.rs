//! What the core remembers for a project, or a folder above it when one
//! product spans several repositories: the rules agents follow there and
//! knowledge of how things are. Kept in `memory/` in the data folder, one file
//! per folder. Every entry says where it was learned. The core tests the
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
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_view::memory::{
    FolderMemory, Knowledge, Learned, ListedRule, MemoryCheck, MemoryRule, MemoryView, Seeding,
    Stale,
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
mod now_check;
mod promote;
mod removed;
mod seed;

pub(crate) use brief::{brief, for_chat};
pub(crate) use check::settle;
pub(crate) use now_check::{set_aside, why as set_aside_why, SetAside};
pub(crate) use promote::promote;
pub(crate) use removed::{chat_ready, take_note as take_removed_note};
pub(crate) use seed::{again as seed_again, keep as keep_seeded};

const FOLDER: &str = "memory";
const MAX_FOLDERS: usize = 256;
const MAX_ENTRIES: usize = 200;
const MAX_TEXT_CHARS: usize = 1000;
const MAX_ABOUT: usize = 12;
const MAX_ABOUT_CHARS: usize = 200;
const MAX_CHECK_CHARS: usize = 1000;

/// What one folder keeps. Kept even once empty, so `next` never goes back.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Held {
    folder: String,
    next: u64,
    rules: Vec<MemoryRule>,
    knowledge: Vec<Knowledge>,
    /// The search of a project's past sessions for rules, kept on the
    /// project's own folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seeding: Option<Seeding>,
    /// Rules of the person's that left the list as a check or by an agent, kept to bring back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    set_aside: Vec<now_check::SetAside>,
    /// Rules kept from a chat's words that the person removed, until that chat's agent hears it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed: Vec<removed::Removed>,
}

impl Held {
    fn len(&self) -> usize {
        self.rules.len() + self.knowledge.len()
    }

    fn holds(&self, id: u64) -> bool {
        self.rules.iter().any(|rule| rule.id == id)
            || self.knowledge.iter().any(|known| known.id == id)
    }
}

#[derive(Default)]
struct State {
    held: BTreeMap<String, Held>,
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
}

impl State {
    /// The folders whose entries apply to `project`, nearest first.
    fn applying<'a>(&'a self, project: &'a str) -> impl Iterator<Item = &'a Held> + 'a {
        let mut held: Vec<&Held> = self
            .held
            .values()
            .filter(|held| Path::new(project).starts_with(&held.folder))
            .collect();
        held.sort_by_key(|held| std::cmp::Reverse(held.folder.len()));
        held.into_iter()
    }

    /// The folder applying to `project` that keeps entry `id`.
    fn folder_of(&self, project: &str, id: u64) -> Result<String, String> {
        self.applying(project)
            .find(|held| held.holds(id))
            .map(|held| held.folder.clone())
            .ok_or_else(|| format!("Nothing Sikemux keeps for this project is #{id}"))
    }

    /// An id no entry has ever had.
    fn next_id(&mut self, folder: &str) -> u64 {
        let id = self.held.values().map(|held| held.next).max().unwrap_or(1);
        if let Some(held) = self.held.get_mut(folder) {
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
}

impl Memory {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let folder = data_dir.map(|dir| dir.join(FOLDER));
        if let Some(folder) = &folder {
            if let Err(error) = std::fs::create_dir_all(folder) {
                eprintln!("sikemux core: what Sikemux keeps cannot be saved: {error}");
            }
        }
        let held = kept::load_all::<Held>(folder.as_deref())
            .into_iter()
            .map(|mut held| {
                held.seeding = seed::settled(held.seeding.take());
                (held.folder.clone(), held)
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
        }
    }

    /// Hands the folder's file to be written on a thread of its own, and
    /// answers it, to wait for with [`Kept::flush`].
    fn save(&self, state: &mut State, folder: &str) -> Option<Arc<Kept>> {
        self.wake.now();
        let State {
            held,
            files,
            promoting,
            ..
        } = state;
        let held = held.get(folder)?;
        let file = files.entry(folder.to_owned()).or_insert_with(|| {
            Arc::new(Kept::new(self.folder.as_deref(), &kept::file_for(folder)))
        });
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
}

impl crate::server::part::Part for Memory {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![crate::server::part::on_wake(
            core,
            |core| &core.memory().wake,
            settle,
        )]
    }

    fn flush(&self) {
        let files: Vec<Arc<Kept>> = self.state.locked().files.values().cloned().collect();
        for file in files {
            file.flush();
        }
    }
}

/// Clients read what is kept again, and the inbox moves when knowledge went
/// stale or holds again.
fn tell_clients(core: &Core, stale_moved: bool) {
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

/// The folder an entry is kept for: the project's own, or one above it.
fn folder_for(project: &str, folder: Option<&str>) -> Result<String, String> {
    let Some(folder) = folder else {
        return Ok(project.to_owned());
    };
    let canonical = std::fs::canonicalize(folder)
        .map_err(|error| format!("folder {folder} cannot be read: {error}"))?;
    if canonical.parent().is_none() || !Path::new(project).starts_with(&canonical) {
        return Err(format!(
            "folder must be this project, {project}, or a folder above it other than /"
        ));
    }
    Ok(canonical.to_string_lossy().into_owned())
}

/// What `folder` keeps, kept from now on when it kept nothing yet.
fn holding<'a>(state: &'a mut State, folder: &str) -> Result<&'a mut Held, String> {
    if !state.held.contains_key(folder) && state.held.len() >= MAX_FOLDERS {
        return Err(format!(
            "Sikemux already keeps rules and knowledge for {MAX_FOLDERS} folders, as many as it can"
        ));
    }
    Ok(state.held.entry(folder.to_owned()).or_insert_with(|| Held {
        folder: folder.to_owned(),
        next: 1,
        rules: Vec::new(),
        knowledge: Vec::new(),
        seeding: None,
        set_aside: Vec::new(),
        removed: Vec::new(),
    }))
}

fn add(core: &Core, project: &str, new: NewMemory, learned: Learned) -> Result<u64, String> {
    let (text, folder) = match &new {
        NewMemory::Rule { text, folder, .. } | NewMemory::Knowledge { text, folder, .. } => (
            to_keep(text, "text", MAX_TEXT_CHARS)?,
            folder_for(project, folder.as_deref())?,
        ),
    };
    let mut state = core.memory().state.locked();
    let held = holding(&mut state, &folder)?;
    if held.len() >= MAX_ENTRIES {
        return Err(format!(
            "{folder} already keeps {MAX_ENTRIES} rules and knowledge; retire some first"
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
    let id = state.next_id(&folder);
    let held = state
        .held
        .get_mut(&folder)
        .ok_or("the folder went while it was being written to")?;
    match new {
        NewMemory::Rule { sensitive, .. } => {
            held.rules.push(MemoryRule {
                id,
                text,
                sensitive,
                learned,
                was: None,
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
        }),
    }
    core.memory().save(&mut state, &folder);
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
            folder,
        } => {
            if sensitive || folder.is_some() {
                return Err("A goal's rule shows on its card and belongs to its project; keep sensitive or shared rules without goal".into());
            }
            let rule = goal::Change::of(goal::List::Rules, None, Some(&text), None, false)?;
            goal::change(core, project, card, goal::List::Rules, rule, learned)
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
    goal::change(core, project, card, goal::List::Rules, rule, by).map(drop)
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
        (Some(agent), Some(text)) => said_by_person(core, agent, text).is_ok(),
        _ => false,
    };
    let mut state = core.memory().state.locked();
    if state.promoting == Some(id) {
        return Err(format!("#{id} is being written into AGENTS.md"));
    }
    let folder = state.folder_of(project, id)?;
    let persons_rule = (state.held.get(&folder))
        .and_then(|held| held.rules.iter().find(|rule| rule.id == id))
        .filter(|rule| rule.learned.agent.is_none());
    if let Some(rule) = persons_rule.filter(|_| acting.is_some()) {
        now_check::agent_may_change(rule, overruled, persons_turn)?;
    }
    let note_id =
        (acting.is_some() && persons_rule.is_some() && text.is_some() && !new_words_persons)
            .then(|| state.next_id(&folder));
    let held = state
        .held
        .get_mut(&folder)
        .ok_or("the folder went while it was being changed")?;
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
                rule.learned = learned;
                line = Some((
                    now_check::line(core, project, rule, &agent, None, Some(was.clone())),
                    agent,
                ));
                rule.was = Some(was);
            }
            (Some(text), Some(agent)) => {
                let at = (held.rules.iter().position(|rule| rule.id == id))
                    .ok_or("the rule went while it was being changed")?;
                let rule = held.rules.remove(at);
                line = Some((
                    now_check::line(core, project, &rule, &agent, None, None),
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
                rule.learned = by;
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
                        now_check::line(core, project, &rule, &agent, None, None),
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
            known.learned = by.clone();
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
    core.memory().save(&mut state, &folder);
    drop(state);
    tell_clients(core, stale_moved);
    if let Some((line, agent)) = line {
        crate::server::chief::rule_changed(core, line, &agent);
    }
    if let Some(agent) = removed_from {
        removed::show(core, &agent);
    }
    if let Some(agent) = back {
        let rule = (core.memory().state.locked().held.get(&folder))
            .and_then(|held| held.rules.iter().find(|rule| rule.id == id).cloned());
        if let Some(rule) = rule {
            let mut line = now_check::line(core, project, &rule, &agent, None, None);
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
    let folders: Vec<String> = (core.memory().state.locked().applying(project))
        .map(|held| held.folder.clone())
        .collect();
    let files: HashMap<String, _> = folders
        .into_iter()
        .map(|folder| {
            let file = promote::agents_file(&folder);
            (folder, file)
        })
        .collect();
    let state = core.memory().state.locked();
    let folders = state
        .applying(project)
        .map(|held| {
            // A folder that came since its file was read is not offered to move yet.
            let file = (files.get(&held.folder).cloned())
                .unwrap_or(Err(sikemux_view::memory::Stays::NoCheckout));
            let rules: Vec<ListedRule> = (held.rules.iter())
                .filter(|rule| shows_sensitive || !rule.sensitive)
                .map(|rule| ListedRule {
                    stays: promote::stays(rule, &file),
                    rule: rule.clone(),
                })
                .collect();
            let knowledge: Vec<Knowledge> = (held.knowledge.iter())
                .filter(|known| shows_sensitive || !known.sensitive)
                .cloned()
                .collect();
            FolderMemory {
                folder: held.folder.clone(),
                hidden: held.len() - rules.len() - knowledge.len(),
                rules,
                knowledge,
            }
        })
        .collect();
    let seeding = state
        .held
        .get(project)
        .and_then(|held| held.seeding.clone());
    MemoryView { folders, seeding }
}

/// What the Playbook lists for `project`: the person's rules, then what agents noted and the
/// knowledge kept, nearest folder first. Sensitive entries show only in Sikemux's own window.
pub(crate) fn for_playbook(core: &Core, project: &str, peer: &Peer) -> (Vec<Entry>, Vec<Entry>) {
    let shows_sensitive = matches!(peer, Peer::App);
    let state = core.memory().state.locked();
    let (mut rules, mut notes) = (Vec::new(), Vec::new());
    for held in state.applying(project) {
        let folder = (held.folder != project).then(|| held.folder.clone());
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
                folder: folder.clone(),
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
                folder: folder.clone(),
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
    pub folder: String,
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
            listed.push(StaleKnowledge {
                folder: held.folder.clone(),
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
        folder: Option<String>,
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
        folder: Option<String>,
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
        let new = match adding {
            ToolAdd::Rule {
                text,
                person: true,
                goal,
                sensitive,
                folder,
            } => {
                if goal {
                    return Err(
                        "person: true keeps the person's rule for the project; leave out goal"
                            .into(),
                    );
                }
                said_by_person(&core, &agent, &text)?;
                by.with = (by.agent.take()).map(|agent| sikemux_view::memory::SaidTo {
                    agent,
                    name: by.name.take(),
                });
                let id = add(
                    &core,
                    &project,
                    NewMemory::Rule {
                        text,
                        goal: None,
                        sensitive,
                        folder,
                    },
                    by,
                )?;
                say_kept(&core, &agent, &project, id);
                return Ok(json!({ "note": format!("Kept as #{id}, as the person's rule.") }));
            }
            ToolAdd::Rule {
                text,
                goal,
                sensitive,
                folder,
                ..
            } => {
                let goal = goal.then(|| goal_card(&core, &agent)).transpose()?;
                NewMemory::Rule {
                    text,
                    goal,
                    sensitive,
                    folder,
                }
            }
            ToolAdd::Knowledge {
                text,
                about,
                sensitive,
                check,
                folder,
            } => NewMemory::Knowledge {
                text,
                about,
                sensitive,
                check,
                folder,
            },
        };
        let note = keep(&core, &project, new, by)?;
        Ok(json!({ "note": note }))
    })
    .await
}

/// Whether what the person said in this turn, as the chat's record has it, holds most of the
/// rule's longer words, so an agent cannot pass its own idea, or a message another agent or a
/// trigger sent, off as the person's.
fn said_by_person(core: &Core, agent: &str, rule: &str) -> Result<(), String> {
    const NOT_THEIRS: &str =
        "person: true is for a rule the person said to you in this turn; keep it without person";
    const ADDED: &str = "Nothing is kept yet: the rule holds words the person did not say. Keep it again with person: true in their own words, without what you added, then keep your additions, such as examples, as a note of your own without person. If they said no such rule, keep it without person. Their rule is not turned down, so don't tell them it was.";
    let said = (core.chats().get(agent))
        .map(|chat| chat.feed.person_said())
        .unwrap_or_default();
    if said.is_empty() {
        return Err(NOT_THEIRS.into());
    }
    let prompt = said.join("\n");
    let words = |text: &str| -> HashSet<String> {
        (text.split(|c: char| !c.is_alphanumeric()))
            .filter(|word| word.chars().count() >= 4)
            .map(str::to_lowercase)
            .collect()
    };
    let (said, rule) = (words(&prompt), words(rule));
    let held = rule.iter().filter(|word| said.contains(*word)).count();
    match !rule.is_empty() && held * 2 >= rule.len() {
        true => Ok(()),
        false => Err(ADDED.into()),
    }
}

/// The line in the agent's chat that says rule `id` was kept from the person's words.
fn say_kept(core: &Core, agent: &str, project: &str, id: u64) {
    let Some(chat) = core.chats().get(agent) else {
        return;
    };
    let Some(start) = chat.feed.start() else {
        return;
    };
    let rule = sikemux_view::chat::KeptRule {
        id,
        project: project.to_owned(),
        undone: false,
    };
    chat.emit(
        sikemux_fold::ChatEventKind::SessionUpdate,
        json!({
            "sessionId": start.session_id,
            "update": sikemux_fold::update::rule_notice(rule),
        }),
    );
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
        let update = MemoryUpdateParams {
            goal: given.goal.then(|| goal_card(&core, &agent)).transpose()?,
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
        Ok(match said_again {
            true => json!({ "id": id, "note": format!("Counted #{id} as said again; the rule is unchanged") }),
            false => json!({ "id": id }),
        })
    })
    .await
}

fn said_by(learned: &Learned) -> &str {
    learned.name.as_deref().unwrap_or("the person")
}

/// `memory_list`: every rule and piece of knowledge kept for the agent's
/// project, with the decisions of the goal its live card sits under.
pub(crate) async fn tool_list(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "memory_list")?;
    let project = crate::server::caller::project_of(core, request);
    let goal = board::goal_of(core, &agent).map(|goal| (goal.card, goal::standing(core, &goal)));
    let state = core.memory().state.locked();
    let (mut rules, mut knowledge) = (Vec::new(), Vec::new());
    for held in state.applying(&project) {
        let folder = (held.folder != project).then_some(held.folder.as_str());
        for rule in &held.rules {
            rules.push(json!({
                "id": rule.id,
                "text": rule.text,
                "from": said_by(&rule.learned),
                "fromId": rule.learned.agent,
                "at": rule.learned.at,
                "folder": folder,
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
                "folder": folder,
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
        "goal": goal.map(|(card, standing)| json!({
            "card": card,
            "rules": standing.rules,
            "decisions": standing.decisions,
        })),
    });
    if !set_aside.is_empty() {
        listed["setAside"] = json!(set_aside);
        listed["setAsideNote"] = json!(BRING_BACK);
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
        folder: None,
    };
    add(core, project, rule, learned(core, None))
}

/// Retires a rule of the project's in the person's name, as undoing an Overrule does.
pub(crate) fn retire_persons_rule(core: &Core, project: &str, id: u64) -> Result<(), String> {
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
    };
    change(core, project, update, learned(core, None))
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
                folder: None,
                sensitive,
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

pub(crate) async fn person_update(
    core: &Arc<Core>,
    peer: Peer,
    params: MemoryUpdateParams,
) -> Result<MemoryView, String> {
    let project = project_at(core, &params.project)?;
    let core = core.clone();
    run(move || {
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
