//! What the core remembers for a project, or a folder above it when one
//! product spans several repositories: the rules agents follow there and
//! knowledge of how things are. Kept in `memory/` in the data folder, one file
//! per folder. Every entry says where it was learned. The core tests the
//! knowledge it can; knowledge that no longer holds is marked stale, stays
//! out of briefs and waits in the inbox until someone corrects or retires it.

use crate::server::changed::What;
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
mod promote;
mod seed;

pub(crate) use brief::{brief, for_chat};
pub(crate) use check::settle;
pub(crate) use promote::promote;
pub(crate) use seed::{again as seed_again, keep as keep_seeded};
pub(crate) use seed::{offers, seed_once};

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
            |core| &core.memory.wake,
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

/// A rule or knowledge to keep, as the person writes it.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NewMemory {
    Rule {
        text: String,
        /// The goal card it is kept on instead, for a rule only that goal's
        /// agents follow.
        goal: Option<u64>,
        sensitive: bool,
        /// A folder above the project, to keep it for every project under it.
        folder: Option<String>,
    },
    Knowledge {
        text: String,
        about: Vec<String>,
        sensitive: bool,
        check: Option<MemoryCheck>,
        /// A folder above the project, to keep it for every project under it.
        folder: Option<String>,
    },
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
    }))
}

fn add(core: &Core, project: &str, new: NewMemory, learned: Learned) -> Result<u64, String> {
    let (text, folder) = match &new {
        NewMemory::Rule { text, folder, .. } | NewMemory::Knowledge { text, folder, .. } => (
            to_keep(text, "text", MAX_TEXT_CHARS)?,
            folder_for(project, folder.as_deref())?,
        ),
    };
    let mut state = core.memory.state.locked();
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
    core.memory.save(&mut state, &folder);
    drop(state);
    tell_clients(core, false);
    Ok(id)
}

/// What changes about an entry. A rule is only corrected or retired;
/// knowledge may also be said not to hold, and have what it is about and how
/// it is tested changed.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUpdateParams {
    pub project: String,
    pub id: u64,
    /// What is so instead, correcting it.
    pub text: Option<String>,
    /// Why knowledge no longer holds, when what does is not known.
    pub wrong: Option<String>,
    #[serde(default)]
    pub retire: bool,
    pub about: Option<Vec<String>>,
    /// How the core tests knowledge from now on.
    pub check: Option<MemoryCheck>,
    /// The core stops testing knowledge.
    #[serde(default)]
    pub drop_check: bool,
    /// The goal card whose own rule `id` is.
    pub goal: Option<u64>,
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
    let mut state = core.memory.state.locked();
    if state.promoting == Some(id) {
        return Err(format!("#{id} is being written into AGENTS.md"));
    }
    let folder = state.folder_of(project, id)?;
    let held = state
        .held
        .get_mut(&folder)
        .ok_or("the folder went while it was being changed")?;
    let mut stale_moved = false;
    if let Some(rule) = held.rules.iter_mut().find(|rule| rule.id == id) {
        let rule_change =
            wrong.is_none() && about.is_none() && check.is_none() && !update.drop_check;
        if !rule_change || saying == 0 {
            return Err(format!(
                "#{id} is a rule: correct it with text, or retire it"
            ));
        }
        if let Some(text) = text {
            rule.was = Some(std::mem::replace(&mut rule.text, text));
            rule.learned = by;
        }
        if update.retire {
            held.rules.retain(|rule| rule.id != id);
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
    core.memory.save(&mut state, &folder);
    drop(state);
    tell_clients(core, stale_moved);
    Ok(())
}

/// What applies to `project`, with sensitive entries left out unless Sikemux's
/// own window reads it.
fn view(core: &Core, project: &str, peer: &Peer) -> MemoryView {
    let shows_sensitive = matches!(peer, Peer::App);
    let folders: Vec<String> = (core.memory.state.locked().applying(project))
        .map(|held| held.folder.clone())
        .collect();
    let files: HashMap<String, _> = folders
        .into_iter()
        .map(|folder| {
            let file = promote::agents_file(&folder);
            (folder, file)
        })
        .collect();
    let state = core.memory.state.locked();
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
    let state = core.memory.state.locked();
    let rule = state
        .applying(project)
        .find_map(|held| held.rules.iter().find(names).map(|rule| rule.text.clone()));
    rule
}

pub(crate) fn stale(core: &Core) -> Vec<StaleKnowledge> {
    let state = core.memory.state.locked();
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
/// the goal its live card sits under.
pub(crate) async fn tool_add(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = crate::server::caller::agent_of(request, "memory_add")?;
    let adding: ToolAdd = params(request.params.clone())?;
    let project = crate::server::caller::project_of(core, request);
    let core = core.clone();
    run(move || {
        let new = match adding {
            ToolAdd::Rule {
                text,
                goal,
                sensitive,
                folder,
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
        let note = keep(&core, &project, new, learned(&core, Some(&agent)))?;
        Ok(json!({ "note": note }))
    })
    .await
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
        Ok(json!({ "id": id }))
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
    let state = core.memory.state.locked();
    let (mut rules, mut knowledge) = (Vec::new(), Vec::new());
    for held in state.applying(&project) {
        let folder = (held.folder != project).then_some(held.folder.as_str());
        for rule in &held.rules {
            rules.push(json!({
                "id": rule.id,
                "text": rule.text,
                "from": said_by(&rule.learned),
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
                "at": known.learned.at,
                "check": known.check,
                "stale": stale,
                "folder": folder,
                "sensitive": known.sensitive,
            }));
        }
    }
    drop(state);
    Ok(json!({
        "rules": rules,
        "knowledge": knowledge,
        "goal": goal.map(|(card, standing)| json!({
            "card": card,
            "rules": standing.rules,
            "decisions": standing.decisions,
        })),
    }))
}

async fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    blocking(move || work().map_err(Into::into))
        .await
        .map_err(|error| error.to_string())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryParams {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAddParams {
    pub project: String,
    pub entry: NewMemory,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntryParams {
    pub project: String,
    pub id: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepSeededParams {
    pub project: String,
    /// The offered rules to keep; the rest are dropped.
    pub keep: Vec<u64>,
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

pub(crate) async fn person_add(
    core: &Arc<Core>,
    peer: Peer,
    params: MemoryAddParams,
) -> Result<MemoryView, String> {
    let project = project_at(core, &params.project)?;
    let core = core.clone();
    run(move || {
        keep(&core, &project, params.entry, learned(&core, None))?;
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
