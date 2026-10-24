//! What each agent is called: a word the core gives it from a built-in list
//! when it starts. No two open agents of a project share one. An agent holds
//! its name while it is open and lets it go when it closes; opened again, it
//! gets a new one. A name the person chose stays the agent's after it closes,
//! and comes back when its chat is opened again. Kept in `agent-names.json` in the data folder, so names
//! outlive a window reload, a restart and an update.

mod words;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::kept::Kept;
use words::{ANIME, NATURE};

const FILE: &str = "agent-names.json";
/// How long a name given to an agent that has not yet been seen open is kept,
/// so one still starting keeps it.
const STARTING_MS: u64 = 10 * 60 * 1000;
const MAX_NAME_CHARS: usize = 24;
/// How many agents below one the person started may start agents of their own.
const MAX_DEPTH: usize = 2;
const MAX_CHILDREN: usize = 6;
const MAX_SPAWNED_PER_PROJECT: usize = 20;
/// How many closed agents' sessions are kept, newest first.
const MAX_CLOSED_SESSIONS: usize = 512;
const MAX_TITLE_CHARS: usize = 200;
const MAX_BUDDY_BYTES: usize = 512;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    agent_id: String,
    name: String,
    project: String,
    /// The agent that started this one.
    parent: Option<String>,
    /// Its turns end without its final answer going to the agent that
    /// started it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    quiet: bool,
    /// A reviewer Auto started, which no agent may reach and which may reach
    /// none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    blind: bool,
    provider: Option<String>,
    /// The provider's own session.
    session_id: Option<String>,
    /// How many leads above it run on other hosts, for an agent whose own
    /// lead runs on another host.
    #[serde(default, skip_serializing_if = "is_zero")]
    above: usize,
    /// When it was named, in Unix milliseconds.
    at: u64,
    /// It has been open since it was named, so closing lets its name go.
    #[serde(default)]
    seen: bool,
    /// The person named it, so it keeps its name after it closes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    chosen: bool,
    /// The character the app last drew for it, which it wears again when
    /// opened again.
    #[serde(skip_serializing_if = "Option::is_none")]
    buddy: Option<Value>,
    /// What it was last working on, as the app titled it.
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

impl Entry {
    fn view(&self) -> AgentName {
        AgentName {
            agent_id: self.agent_id.clone(),
            name: self.name.clone(),
            project: self.project.clone(),
            parent: self.parent.clone(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Book {
    agents: Vec<Entry>,
    /// The lead of each closed agent that an agent still named was started
    /// under, so the chain of leads stays whole after a lead in it closes.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    closed_leads: HashMap<String, String>,
    /// The provider session each recently closed agent ran, so its commits
    /// still lead back to a session that can be resumed.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    closed_sessions: VecDeque<ClosedSession>,
    /// The list new agents' names are taken from.
    #[serde(default)]
    theme: NameTheme,
}

/// The list of words new agents are named from, picked in settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum NameTheme {
    Nature,
    #[default]
    Anime,
}

impl NameTheme {
    fn words(self) -> &'static [&'static str] {
        match self {
            Self::Nature => NATURE,
            Self::Anime => ANIME,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClosedSession {
    agent_id: String,
    #[serde(flatten)]
    ran: RanSession,
}

/// The provider session an agent ran, and the project it ran in.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RanSession {
    pub provider: String,
    pub session_id: String,
    pub project: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentName {
    pub agent_id: String,
    pub name: String,
    pub project: String,
    pub parent: Option<String>,
}

/// A closed agent the person named, which can be opened again under its name.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClosedAgent {
    pub agent_id: String,
    pub name: String,
    pub provider: String,
    pub session_id: String,
    pub title: Option<String>,
    pub buddy: Option<Value>,
}

/// An agent another agent starts.
pub(crate) struct Worker<'a> {
    pub agent_id: &'a str,
    pub lead: &'a str,
    /// The project it opens in, when not its lead's.
    pub project: Option<String>,
    pub provider: &'a str,
    /// The saved session it reopens.
    pub session: Option<&'a str>,
    /// Its final answer goes to its lead each time its turn ends.
    pub reports: bool,
}

/// An agent that wants a name.
pub(crate) struct Starting<'a> {
    pub agent_id: &'a str,
    pub project: String,
    pub provider: Option<&'a str>,
    /// The provider session it takes up again, if any.
    pub resume_id: Option<&'a str>,
}

/// An agent named on a linked host: a lead there whose worker runs here, or a
/// worker here that its lead's host named.
pub(crate) struct Adopted<'a> {
    pub agent_id: &'a str,
    pub name: &'a str,
    pub project: String,
    pub parent: Option<&'a str>,
    pub provider: &'a str,
    pub reports: bool,
    /// How far below the person's own agent its lead's host has it.
    pub depth: usize,
}

pub(crate) struct Names {
    book: Mutex<Book>,
    file: Kept,
    /// Agents running on linked hosts, which keep their names while they do.
    pinned: Mutex<HashSet<String>>,
}

impl Names {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let file = Kept::new(data_dir, FILE);
        Self {
            book: Mutex::new(file.load()),
            file,
            pinned: Mutex::new(HashSet::new()),
        }
    }

    /// The agents open here with those running on linked hosts.
    fn open_with(&self, mut open: HashSet<String>) -> HashSet<String> {
        open.extend(self.pinned.locked().iter().cloned());
        open
    }

    /// The agents on linked hosts this host knows, whose names it keeps.
    pub(crate) fn pin(&self, agent_ids: HashSet<String>) {
        *self.pinned.locked() = agent_ids;
    }

    /// How many agents up the chain of leads the person's own agent is.
    pub(crate) fn depth(&self, agent_id: &str) -> usize {
        self.book.locked().depth(agent_id)
    }

    /// Takes in an agent named on a linked host, under that name unless an
    /// open agent of its project here has it. `open` holds the agents
    /// open here.
    pub(crate) fn adopt(&self, adopted: Adopted<'_>, open: HashSet<String>) -> AgentName {
        let mut book = self.book.locked();
        let open = self.open_with(open);
        book.release(&open);
        book.remove(|entry| entry.agent_id == adopted.agent_id);
        let taken = book.agents.iter().any(|entry| {
            entry.project == adopted.project
                && open.contains(&entry.agent_id)
                && same(&entry.name, adopted.name)
        });
        let name = match valid_name(adopted.name) {
            Ok(name) if !taken => name,
            _ => book.fresh(&adopted.project, adopted.agent_id, &open),
        };
        book.remove(|entry| {
            entry.project == adopted.project
                && !open.contains(&entry.agent_id)
                && same(&entry.name, &name)
        });
        let above = match adopted.parent {
            Some(_) => 0,
            None => adopted.depth,
        };
        let entry = Entry {
            agent_id: adopted.agent_id.to_owned(),
            name,
            project: adopted.project,
            parent: adopted.parent.map(str::to_owned),
            quiet: !adopted.reports,
            blind: false,
            provider: Some(adopted.provider.to_owned()),
            session_id: None,
            above,
            at: epoch_ms(),
            seen: false,
            chosen: false,
            buddy: None,
            title: None,
        };
        let named = entry.view();
        book.agents.push(entry);
        self.save(&book);
        named
    }

    /// Calls an agent what the host it runs on calls it.
    pub(crate) fn set_name(&self, agent_id: &str, name: &str) {
        let mut book = self.book.locked();
        if let Some(entry) = book.entry_mut(agent_id).filter(|entry| entry.name != name) {
            entry.name = name.to_owned();
            self.save(&book);
        }
    }

    /// Gives each agent the name it goes by.
    pub(crate) fn fill(&self, agents: &mut [crate::protocol::WorkspaceAgent]) {
        let book = self.book.locked();
        let names: HashMap<&str, &str> = book
            .agents
            .iter()
            .map(|entry| (entry.agent_id.as_str(), entry.name.as_str()))
            .collect();
        for agent in agents {
            agent.name = names.get(agent.id.as_str()).map(|name| (*name).to_owned());
        }
    }

    pub(crate) fn name(&self, agent_id: &str) -> Option<String> {
        self.book
            .locked()
            .agents
            .iter()
            .find(|entry| entry.agent_id == agent_id)
            .map(|entry| entry.name.clone())
    }

    pub(crate) fn get(&self, agent_id: &str) -> Option<AgentName> {
        self.book
            .locked()
            .agents
            .iter()
            .find(|entry| entry.agent_id == agent_id)
            .map(Entry::view)
    }

    /// The names of the agents in `agent_ids`, each with the agent that
    /// started it.
    pub(crate) fn of(&self, agent_ids: &HashSet<String>) -> Vec<AgentName> {
        self.book
            .locked()
            .agents
            .iter()
            .filter(|entry| agent_ids.contains(&entry.agent_id))
            .map(Entry::view)
            .collect()
    }

    /// The agent's name, given now when it has none, and whether that changed
    /// anything. `open` holds the agents open here.
    pub(crate) fn give(&self, starting: Starting<'_>, open: HashSet<String>) -> (AgentName, bool) {
        let mut book = self.book.locked();
        let open = self.open_with(open);
        let (named, mut changed) = book.give(starting, &open);
        changed |= book.release(&open);
        if changed {
            self.save(&book);
        }
        (named, changed)
    }

    /// Names a worker for `worker.lead`, in the project it opens in, unless
    /// the lead may not start another. `open` holds the agents open here.
    pub(crate) fn hire(
        &self,
        worker: Worker<'_>,
        open: HashSet<String>,
    ) -> Result<AgentName, String> {
        let mut book = self.book.locked();
        let open = self.open_with(open);
        book.release(&open);
        let lead = book
            .agents
            .iter()
            .find(|entry| entry.agent_id == worker.lead)
            .cloned()
            .ok_or("Sikemux has not named you yet; try again in a moment")?;
        if book.depth(worker.lead) >= MAX_DEPTH {
            return Err(format!(
                "{} is already {MAX_DEPTH} levels below an agent the person started, so it cannot start agents; do this work yourself or hand it back to your lead",
                lead.name
            ));
        }
        let project = worker.project.unwrap_or_else(|| lead.project.clone());
        let live = |entry: &&Entry| open.contains(&entry.agent_id);
        let children = book
            .agents
            .iter()
            .filter(live)
            .filter(|entry| entry.parent.as_deref() == Some(worker.lead))
            .count();
        if children >= MAX_CHILDREN {
            return Err(format!(
                "{} already has {MAX_CHILDREN} agents of its own open, the most one agent may have; give one of them the work, or ask the person to close one that is done",
                lead.name
            ));
        }
        let spawned = book
            .agents
            .iter()
            .filter(live)
            .filter(|entry| entry.project == project && entry.parent.is_some())
            .count();
        if spawned >= MAX_SPAWNED_PER_PROJECT {
            return Err(format!(
                "this project already has {MAX_SPAWNED_PER_PROJECT} agents that agents started open, the most it may have; give one of them the work, or ask the person to close one that is done"
            ));
        }
        let entry = Entry {
            agent_id: worker.agent_id.to_owned(),
            name: book.fresh(&project, worker.agent_id, &open),
            project,
            parent: Some(worker.lead.to_owned()),
            quiet: !worker.reports,
            blind: false,
            provider: Some(worker.provider.to_owned()),
            session_id: worker.session.map(str::to_owned),
            above: 0,
            at: epoch_ms(),
            seen: false,
            chosen: false,
            buddy: None,
            title: None,
        };
        let named = entry.view();
        book.agents.push(entry);
        self.save(&book);
        Ok(named)
    }

    /// Forgets a worker that never started, so its name is free again.
    pub(crate) fn forget(&self, agent_id: &str) {
        let mut book = self.book.locked();
        if book.remove(|entry| entry.agent_id == agent_id) {
            self.save(&book);
        }
    }

    pub(crate) fn make_blind(&self, agent_id: &str) {
        let mut book = self.book.locked();
        if let Some(entry) = book.entry_mut(agent_id).filter(|entry| !entry.blind) {
            entry.blind = true;
            self.save(&book);
        }
    }

    pub(crate) fn is_blind(&self, agent_id: &str) -> bool {
        self.book
            .locked()
            .agents
            .iter()
            .any(|entry| entry.agent_id == agent_id && entry.blind)
    }

    /// The agent that hears how the agent's turns end, if any.
    pub(crate) fn reports_to(&self, agent_id: &str) -> Option<String> {
        self.book
            .locked()
            .agents
            .iter()
            .find(|entry| entry.agent_id == agent_id && !entry.quiet)
            .and_then(|entry| entry.parent.clone())
    }

    /// The agent at the top of `agent_id`'s chain of leads, the one the person
    /// started, which names its team.
    pub(crate) fn team_of(&self, agent_id: &str) -> String {
        let book = self.book.locked();
        let mut current = agent_id;
        for _ in 0..=book.agents.len() + book.closed_leads.len() {
            match book.parent(current) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        current.to_owned()
    }

    /// Whether `lead` started `agent_id`, or started an agent that did.
    pub(crate) fn leads(&self, lead: &str, agent_id: &str) -> bool {
        self.book.locked().leads(lead, agent_id)
    }

    /// `lead` and every named agent it started, or that one of those started.
    pub(crate) fn team(&self, lead: &str) -> Vec<String> {
        let book = self.book.locked();
        let mut team = vec![lead.to_owned()];
        team.extend(
            book.agents
                .iter()
                .filter(|entry| entry.agent_id != lead && book.leads(lead, &entry.agent_id))
                .map(|entry| entry.agent_id.clone()),
        );
        team
    }

    /// Lets go of the names of agents closed since. `open` holds the agents
    /// open here.
    pub(crate) fn settle(&self, open: HashSet<String>) {
        let mut book = self.book.locked();
        let open = self.open_with(open);
        if book.release(&open) {
            self.save(&book);
        }
    }

    /// The provider session the agent runs, once it is known.
    pub(crate) fn session_of(&self, agent_id: &str) -> Option<String> {
        self.book
            .locked()
            .agents
            .iter()
            .find(|entry| entry.agent_id == agent_id)
            .and_then(|entry| entry.session_id.clone())
    }

    /// The provider and session the agent ran, whether it is open or closed
    /// since.
    pub(crate) fn ran_session(&self, agent_id: &str) -> Option<RanSession> {
        let book = self.book.locked();
        if let Some(entry) = book.agents.iter().find(|entry| entry.agent_id == agent_id) {
            return Some(RanSession {
                provider: entry.provider.clone()?,
                session_id: entry.session_id.clone()?,
                project: entry.project.clone(),
            });
        }
        book.closed_sessions
            .iter()
            .find(|closed| closed.agent_id == agent_id)
            .map(|closed| closed.ran.clone())
    }

    /// The provider session the agent ran, whether it is open or closed since,
    /// for one whose provider is not known too.
    pub(crate) fn last_session(&self, agent_id: &str) -> Option<String> {
        let book = self.book.locked();
        match book.agents.iter().find(|entry| entry.agent_id == agent_id) {
            Some(entry) => entry.session_id.clone(),
            None => book
                .closed_sessions
                .iter()
                .find(|closed| closed.agent_id == agent_id)
                .map(|closed| closed.ran.session_id.clone()),
        }
    }

    /// Whether an agent here runs or recently ran `session_id`.
    pub(crate) fn knows_session(&self, session_id: &str) -> bool {
        let book = self.book.locked();
        book.agents
            .iter()
            .any(|entry| entry.session_id.as_deref() == Some(session_id))
            || book
                .closed_sessions
                .iter()
                .any(|closed| closed.ran.session_id == session_id)
    }

    pub(crate) fn note_session(&self, agent_id: &str, session_id: &str) {
        let mut book = self.book.locked();
        let Some(entry) = book.entry_mut(agent_id) else {
            return;
        };
        if entry.session_id.as_deref() == Some(session_id) {
            return;
        }
        entry.session_id = Some(session_id.to_owned());
        self.save(&book);
    }

    /// The person's own name for the agent, unless an open agent of the
    /// project has it.
    pub(crate) fn rename(
        &self,
        agent_id: &str,
        name: &str,
        open: HashSet<String>,
    ) -> Result<AgentName, String> {
        let name = valid_name(name)?;
        let mut book = self.book.locked();
        let open = self.open_with(open);
        book.release(&open);
        let project = book
            .entry_mut(agent_id)
            .ok_or("Sikemux has not named that agent yet")?
            .project
            .clone();
        let holder = book.agents.iter().find(|entry| {
            entry.agent_id != agent_id && entry.project == project && same(&entry.name, &name)
        });
        if let Some(holder) = holder.filter(|holder| open.contains(&holder.agent_id)) {
            return Err(format!(
                "another agent in this project is called {}",
                holder.name
            ));
        }
        book.remove(|entry| {
            entry.agent_id != agent_id && entry.project == project && same(&entry.name, &name)
        });
        let entry = book
            .entry_mut(agent_id)
            .ok_or("Sikemux has not named that agent yet")?;
        entry.name = name;
        entry.chosen = true;
        let named = entry.view();
        self.save(&book);
        Ok(named)
    }

    /// Gives a new agent opening a closed agent's chat again that agent's
    /// name, when the person chose it, and the character it wore. `open`
    /// holds the agents open here.
    pub(crate) fn reopen(
        &self,
        starting: &Starting<'_>,
        open: HashSet<String>,
    ) -> Option<Option<Value>> {
        let session = starting.resume_id?;
        let mut book = self.book.locked();
        if book.entry_mut(starting.agent_id).is_some() {
            return None;
        }
        let open = self.open_with(open);
        let entry = book.agents.iter_mut().find(|entry| {
            entry.chosen
                && entry.project == starting.project
                && !open.contains(&entry.agent_id)
                && entry.session_id.as_deref() == Some(session)
                && starting
                    .provider
                    .is_none_or(|provider| entry.provider.as_deref() == Some(provider))
        })?;
        entry.agent_id = starting.agent_id.to_owned();
        entry.seen = false;
        entry.at = epoch_ms();
        let buddy = entry.buddy.clone();
        self.save(&book);
        Some(buddy)
    }

    /// Keeps how the app draws the agent and what it works on, for when it is
    /// opened again after closing. What the app leaves out stays as it was.
    pub(crate) fn note_look(&self, agent_id: &str, buddy: Option<Value>, title: Option<String>) {
        let buddy = buddy.filter(|buddy| buddy.to_string().len() <= MAX_BUDDY_BYTES);
        let title = title.filter(|title| title.chars().count() <= MAX_TITLE_CHARS);
        let mut book = self.book.locked();
        let Some(entry) = book.entry_mut(agent_id) else {
            return;
        };
        let buddy = buddy.or_else(|| entry.buddy.clone());
        let title = title.or_else(|| entry.title.clone());
        if entry.buddy == buddy && entry.title == title {
            return;
        }
        entry.buddy = buddy;
        entry.title = title;
        self.save(&book);
    }

    /// The closed agents of `project` the person named, newest first.
    pub(crate) fn closed(&self, project: &str, open: HashSet<String>) -> Vec<ClosedAgent> {
        let book = self.book.locked();
        let open = self.open_with(open);
        let mut closed: Vec<&Entry> = book
            .agents
            .iter()
            .filter(|entry| {
                entry.chosen && entry.project == project && !open.contains(&entry.agent_id)
            })
            .collect();
        closed.sort_by_key(|entry| std::cmp::Reverse(entry.at));
        closed
            .into_iter()
            .filter_map(|entry| {
                Some(ClosedAgent {
                    agent_id: entry.agent_id.clone(),
                    name: entry.name.clone(),
                    provider: entry.provider.clone()?,
                    session_id: entry.session_id.clone()?,
                    title: entry.title.clone(),
                    buddy: entry.buddy.clone(),
                })
            })
            .collect()
    }

    pub(crate) fn theme(&self) -> NameTheme {
        self.book.locked().theme
    }

    /// Names agents that start from now on from `theme`'s words. Open
    /// agents keep the names they have.
    pub(crate) fn set_theme(&self, theme: NameTheme) {
        let mut book = self.book.locked();
        if book.theme != theme {
            book.theme = theme;
            self.save(&book);
        }
    }

    fn save(&self, book: &Book) {
        self.file.save(book);
    }
}

impl Book {
    fn parent(&self, agent_id: &str) -> Option<&str> {
        match self.agents.iter().find(|entry| entry.agent_id == agent_id) {
            Some(entry) => entry.parent.as_deref(),
            None => self.closed_leads.get(agent_id).map(String::as_str),
        }
    }

    fn leads(&self, lead: &str, agent_id: &str) -> bool {
        let mut current = agent_id;
        for _ in 0..=self.agents.len() + self.closed_leads.len() {
            match self.parent(current) {
                Some(parent) if parent == lead => return true,
                Some(parent) => current = parent,
                None => break,
            }
        }
        false
    }

    /// Takes out the entries `gone` picks, and whether there were any.
    fn remove(&mut self, mut gone: impl FnMut(&mut Entry) -> bool) -> bool {
        let mut removed = Vec::new();
        self.agents.retain_mut(|entry| {
            let keep = !gone(entry);
            if !keep {
                removed.push(entry.clone());
            }
            keep
        });
        if removed.is_empty() {
            return false;
        }
        for entry in removed {
            if let (Some(provider), Some(session_id)) = (entry.provider, entry.session_id) {
                self.closed_sessions
                    .retain(|closed| closed.agent_id != entry.agent_id);
                self.closed_sessions.push_front(ClosedSession {
                    agent_id: entry.agent_id.clone(),
                    ran: RanSession {
                        provider,
                        session_id,
                        project: entry.project.clone(),
                    },
                });
                self.closed_sessions.truncate(MAX_CLOSED_SESSIONS);
            }
            if let Some(parent) = entry.parent {
                self.closed_leads.insert(entry.agent_id, parent);
            }
        }
        self.prune_closed_leads();
        true
    }

    /// Keeps only the closed leads some named agent's chain still runs through.
    fn prune_closed_leads(&mut self) {
        let mut needed: HashSet<&str> = HashSet::new();
        for entry in &self.agents {
            let mut current = entry.parent.as_deref();
            while let Some(id) = current {
                if self.agents.iter().any(|named| named.agent_id == id) || !needed.insert(id) {
                    break;
                }
                current = self.closed_leads.get(id).map(String::as_str);
            }
        }
        let needed: HashSet<String> = needed.into_iter().map(str::to_owned).collect();
        self.closed_leads.retain(|id, _| needed.contains(id));
    }

    /// How many agents up the chain of leads the person's own agent is.
    fn depth(&self, agent_id: &str) -> usize {
        let mut depth = 0;
        let mut current = agent_id;
        while let Some(parent) = self.parent(current) {
            depth += 1;
            if depth > self.agents.len() + self.closed_leads.len() {
                break;
            }
            current = parent;
        }
        let above = self
            .agents
            .iter()
            .find(|entry| entry.agent_id == current)
            .map_or(0, |entry| entry.above);
        depth + above
    }

    fn entry_mut(&mut self, agent_id: &str) -> Option<&mut Entry> {
        self.agents
            .iter_mut()
            .find(|entry| entry.agent_id == agent_id)
    }

    /// The agent's name, and whether the book changed to give it.
    fn give(&mut self, starting: Starting<'_>, open: &HashSet<String>) -> (AgentName, bool) {
        let Starting {
            agent_id,
            project,
            provider,
            resume_id,
        } = starting;
        let current = self
            .agents
            .iter()
            .find(|entry| entry.agent_id == agent_id)
            .map(|entry| (entry.project != project, entry.name.clone()));
        if let Some((moved, kept)) = current {
            let name = if moved && self.taken(&project, &kept, agent_id) {
                self.fresh(&project, agent_id, open)
            } else {
                kept
            };
            if let Some(entry) = self.entry_mut(agent_id) {
                let mut changed = moved;
                if moved {
                    entry.project = project;
                    entry.name = name;
                    entry.at = epoch_ms();
                }
                if let Some(provider) = provider.filter(|p| entry.provider.as_deref() != Some(*p)) {
                    entry.provider = Some(provider.to_owned());
                    changed = true;
                }
                if let Some(session) = resume_id.filter(|s| entry.session_id.as_deref() != Some(*s))
                {
                    entry.session_id = Some(session.to_owned());
                    changed = true;
                }
                return (entry.view(), changed);
            }
        }
        let name = self.fresh(&project, agent_id, open);
        let entry = Entry {
            agent_id: agent_id.to_owned(),
            name,
            project,
            parent: None,
            quiet: false,
            blind: false,
            provider: provider.map(str::to_owned),
            session_id: resume_id.map(str::to_owned),
            above: 0,
            at: epoch_ms(),
            seen: false,
            chosen: false,
            buddy: None,
            title: None,
        };
        let named = entry.view();
        self.agents.push(entry);
        (named, true)
    }

    fn taken(&self, project: &str, name: &str, except: &str) -> bool {
        self.agents.iter().any(|entry| {
            entry.agent_id != except && entry.project == project && same(&entry.name, name)
        })
    }

    /// A word no agent of the project has, starting differently from the
    /// open ones' so they are told apart at a glance.
    fn fresh(&mut self, project: &str, agent_id: &str, open: &HashSet<String>) -> String {
        let others: Vec<&Entry> = self
            .agents
            .iter()
            .filter(|entry| entry.project == project && entry.agent_id != agent_id)
            .collect();
        let used: HashSet<String> = others
            .iter()
            .map(|entry| entry.name.to_lowercase())
            .collect();
        let open_starts: HashSet<String> = others
            .iter()
            .filter(|entry| open.contains(&entry.agent_id))
            .map(|entry| start(&entry.name))
            .collect();
        let words = self.theme.words();
        let unused: Vec<&str> = words
            .iter()
            .copied()
            .filter(|word| !used.contains(&word.to_lowercase()))
            .collect();
        let distinct: Vec<&str> = unused
            .iter()
            .copied()
            .filter(|word| !open_starts.contains(&start(word)))
            .collect();
        if let Some(word) = pick(&distinct).or_else(|| pick(&unused)) {
            return word.to_owned();
        }
        let oldest_past = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.project == project
                    && entry.agent_id != agent_id
                    && !entry.chosen
                    && !open.contains(&entry.agent_id)
            })
            .min_by_key(|(_, entry)| entry.at)
            .map(|(index, _)| index);
        if let Some(index) = oldest_past {
            let name = self.agents[index].name.clone();
            let gone = self.agents[index].agent_id.clone();
            self.remove(|entry| entry.agent_id == gone);
            return name;
        }
        let word = pick(words).unwrap_or("Agent");
        (2..)
            .map(|number| format!("{word}{number}"))
            .find(|name| !used.contains(&name.to_lowercase()))
            .unwrap_or_else(|| word.to_owned())
    }

    /// Lets go of the names of agents no longer open, and whether the book
    /// changed.
    fn release(&mut self, open: &HashSet<String>) -> bool {
        let now = epoch_ms();
        let mut seen = false;
        let removed = self.remove(|entry| {
            if open.contains(&entry.agent_id) {
                seen |= !entry.seen;
                entry.seen = true;
                return false;
            }
            !entry.chosen && (entry.seen || now.saturating_sub(entry.at) >= STARTING_MS)
        });
        removed || seen
    }
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn start(name: &str) -> String {
    name.chars().take(2).collect::<String>().to_lowercase()
}

fn pick<'a>(words: &[&'a str]) -> Option<&'a str> {
    if words.is_empty() {
        return None;
    }
    let roll = uuid::Uuid::new_v4().as_u128();
    words.get((roll % words.len() as u128) as usize).copied()
}

fn valid_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let mut chars = name.chars();
    let fits = chars.next().is_some_and(char::is_alphabetic)
        && chars.all(char::is_alphanumeric)
        && name.chars().count() <= MAX_NAME_CHARS;
    if fits {
        Ok(name.to_owned())
    } else {
        Err(format!(
            "a name is one word of up to {MAX_NAME_CHARS} letters and digits, starting with a letter"
        ))
    }
}

/// The project an agent working in `cwd` belongs to: the app's project that
/// holds it, else `cwd` itself. A folder in a git worktree counts as the same
/// folder in the main checkout.
pub(crate) fn project_of(cwd: &Path, projects: &[PathBuf]) -> String {
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let cwd = canonical(cwd);
    let path = in_main_checkout(&cwd).unwrap_or(cwd);
    projects
        .iter()
        .map(|project| canonical(project))
        .filter(|project| path.starts_with(project))
        .max_by_key(|project| project.as_os_str().len())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The folder in the main checkout matching `cwd` in a linked git worktree.
fn in_main_checkout(cwd: &Path) -> Option<PathBuf> {
    let root = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    let main = main_checkout(root)?;
    let inside = cwd.strip_prefix(root).ok()?;
    Some(if inside.as_os_str().is_empty() {
        main
    } else {
        main.join(inside)
    })
}

/// Where a linked git worktree's main checkout is. A linked worktree's `.git`
/// is a file naming its folder under the main checkout's `.git`.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    let link = std::fs::read_to_string(root.join(".git")).ok()?;
    let gitdir = root.join(link.strip_prefix("gitdir:")?.trim());
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = std::fs::canonicalize(gitdir.join(common.trim())).ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}
