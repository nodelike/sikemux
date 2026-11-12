//! What each agent is called: a word the core gives it from a built-in list
//! when it starts. No two open agents of a project share one. A chat is one
//! agent for life: closed, it keeps its id and name in its closed record, and
//! opened again it comes back as that agent. A closed chat's name goes to a
//! new agent only once the list has no word left, the longest-closed first,
//! and never one the person chose. Kept in `agent-names.json` in the data
//! folder, so names outlive a window reload, a restart and an update.

mod words;

use crate::server::changed::What;
use crate::server::parts::Parts;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::connection::blocking;
use super::kept::Kept;
use super::{Core, CoreResult};
use sikemux_view::workspace::AgentBuddy;
use words::{ANIME, NATURE};

const FILE: &str = "agent-names.json";

pub(crate) fn new_agent_id() -> String {
    format!("agent-{}", uuid::Uuid::new_v4().simple())
}
/// How long a name given to an agent that has not yet been seen open is kept,
/// so one still starting keeps it.
const STARTING_MS: u64 = 10 * 60 * 1000;
const MAX_NAME_CHARS: usize = 24;
/// How many agents below one the person started may start agents of their own.
const MAX_DEPTH: usize = 2;
const MAX_CHILDREN: usize = 16;
pub(crate) const MAX_SPAWNED_PER_PROJECT: usize = 64;
/// How many closed agents' sessions are kept, newest first.
const MAX_CLOSED_SESSIONS: usize = 4096;
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
    /// Auto started it for a card, so it counts as an agent that agents started.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    by_auto: bool,
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
    buddy: Option<AgentBuddy>,
    /// What it was last working on, as the app titled it.
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// The person's account it runs on, by profile id.
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    /// It runs in its CLI in a terminal rather than as a chat.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    terminal: bool,
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
    /// Each recently closed agent, newest first, so its commits still lead
    /// back to a session that can be resumed and its chat comes back as it.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    closed_sessions: VecDeque<ClosedSession>,
    /// The list new agents' names are taken from.
    #[serde(default)]
    theme: NameTheme,
}

/// The list of words new agents are named from, picked in settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NameTheme {
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
    /// None once a new agent took it because the list ran out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lead: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    quiet: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    blind: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    by_auto: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    chosen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buddy: Option<AgentBuddy>,
}

/// The provider session an agent ran, and the project it ran in.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RanSession {
    pub provider: String,
    pub session_id: String,
    pub project: String,
    /// The person's account it ran on, by profile id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// It ran in its CLI in a terminal rather than as a chat.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terminal: bool,
    /// What it was last working on, as the app titled it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentName {
    pub agent_id: String,
    pub name: String,
    pub project: String,
    pub parent: Option<String>,
}

/// A closed agent the person named, which can be opened again as itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedAgent {
    pub agent_id: String,
    pub name: String,
    pub provider: String,
    pub session_id: String,
    pub title: Option<String>,
    pub buddy: Option<AgentBuddy>,
}

/// A closed agent that came back as itself.
pub(crate) struct Returned {
    /// The character it wore.
    pub buddy: Option<AgentBuddy>,
    /// It kept its name, which no open agent of the project had taken.
    pub same_name: bool,
}

/// Why a lead may not start another agent.
pub(crate) enum Unhired {
    /// The lead, or its project, has as many agents open as it may.
    Full(String),
    Refused(String),
}

impl From<Unhired> for String {
    fn from(unhired: Unhired) -> Self {
        match unhired {
            Unhired::Full(why) | Unhired::Refused(why) => why,
        }
    }
}

/// An agent another agent starts.
#[derive(Clone)]
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
    pub(crate) fn open_with(&self, mut open: HashSet<String>) -> HashSet<String> {
        open.extend(self.pinned.locked().iter().cloned());
        open
    }

    /// The agent's name, or its id for one with none.
    pub(crate) fn name_or_id(&self, agent_id: &str) -> String {
        self.name(agent_id).unwrap_or_else(|| agent_id.to_owned())
    }

    pub(crate) fn is_pinned(&self, agent_id: &str) -> bool {
        self.pinned.locked().contains(agent_id)
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
            Ok(name) if !taken && !same(&name, crate::server::chief::NAME) => name,
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
            by_auto: false,
            provider: Some(adopted.provider.to_owned()),
            session_id: None,
            above,
            at: epoch_ms(),
            seen: false,
            chosen: false,
            buddy: None,
            title: None,
            profile: None,
            terminal: false,
        };
        let named = entry.view();
        book.agents.push(entry);
        self.save(&book);
        named
    }

    /// Names a project's chief, @Chief, as the person would have.
    pub(crate) fn name_chief(&self, agent_id: &str, project: &str) {
        let mut book = self.book.locked();
        if let Some(entry) = book.entry_mut(agent_id) {
            entry.name = crate::server::chief::NAME.to_owned();
            entry.chosen = true;
        } else {
            book.agents.push(Entry {
                agent_id: agent_id.to_owned(),
                name: crate::server::chief::NAME.to_owned(),
                project: project.to_owned(),
                parent: None,
                quiet: false,
                blind: false,
                by_auto: false,
                provider: None,
                session_id: None,
                above: 0,
                at: epoch_ms(),
                seen: false,
                chosen: true,
                buddy: None,
                title: None,
                profile: None,
                terminal: false,
            });
        }
        self.save(&book);
    }

    /// Calls an agent what the host it runs on calls it.
    pub(crate) fn set_name(&self, agent_id: &str, name: &str) {
        let mut book = self.book.locked();
        if let Some(entry) = book.entry_mut(agent_id).filter(|entry| entry.name != name) {
            entry.name = name.to_owned();
            self.save(&book);
        }
    }

    pub(crate) fn fill(&self, agents: &mut [sikemux_view::workspace::WorkspaceAgent]) {
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
        let mut changed = book.release(&open);
        let away = self.pinned.locked().clone();
        let (named, given) = book.give(starting, &open, &away);
        changed |= given;
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
    ) -> Result<AgentName, Unhired> {
        let mut book = self.book.locked();
        let open = self.open_with(open);
        book.release(&open);
        let lead = book
            .agents
            .iter()
            .find(|entry| entry.agent_id == worker.lead)
            .cloned()
            .ok_or_else(|| {
                Unhired::Refused("Sikemux has not named you yet; try again in a moment".into())
            })?;
        if book.depth(worker.lead) >= MAX_DEPTH {
            return Err(Unhired::Refused(format!(
                "{} is already {MAX_DEPTH} levels below an agent the person started, so it cannot start agents; do this work yourself or hand it back to your lead",
                lead.name
            )));
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
            return Err(Unhired::Full(format!(
                "{} already has {MAX_CHILDREN} agents of its own open, the most one agent may have; give one of them the work, or close one that is done with agent_close",
                lead.name
            )));
        }
        if book.started_open(&project, &open) >= MAX_SPAWNED_PER_PROJECT {
            return Err(Unhired::Full(format!(
                "this project already has {MAX_SPAWNED_PER_PROJECT} agents open that agents or Auto started, the most it may have; give one of them the work, or close one of yours that is done with agent_close"
            )));
        }
        let away = self.pinned.locked().clone();
        if book
            .restore(worker.agent_id, &project, &open, &away)
            .is_some()
        {
            let leads_its_lead = book.leads(worker.agent_id, worker.lead);
            let entry = book
                .entry_mut(worker.agent_id)
                .expect("restore puts the agent back in the book");
            if !leads_its_lead {
                entry.parent = Some(worker.lead.to_owned());
                entry.quiet = !worker.reports;
            }
            entry.provider = Some(worker.provider.to_owned());
            if let Some(session) = worker.session {
                entry.session_id = Some(session.to_owned());
            }
            let named = entry.view();
            self.save(&book);
            return Ok(named);
        }
        let entry = Entry {
            agent_id: worker.agent_id.to_owned(),
            name: book.fresh(&project, worker.agent_id, &open),
            project,
            parent: Some(worker.lead.to_owned()),
            quiet: !worker.reports,
            blind: false,
            by_auto: false,
            provider: Some(worker.provider.to_owned()),
            session_id: worker.session.map(str::to_owned),
            above: 0,
            at: epoch_ms(),
            seen: false,
            chosen: false,
            buddy: None,
            title: None,
            profile: None,
            terminal: false,
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

    /// How many more agents the project may have open that agents or Auto started.
    pub(crate) fn room_for_started(&self, project: &str, open: HashSet<String>) -> usize {
        let book = self.book.locked();
        let open = self.open_with(open);
        MAX_SPAWNED_PER_PROJECT.saturating_sub(book.started_open(project, &open))
    }

    pub(crate) fn count_as_started(&self, agent_id: &str) {
        let mut book = self.book.locked();
        if let Some(entry) = book.entry_mut(agent_id).filter(|entry| !entry.by_auto) {
            entry.by_auto = true;
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

    /// The agents `lead` started itself, oldest first.
    pub(crate) fn workers(&self, lead: &str) -> Vec<String> {
        (self.book.locked().agents.iter())
            .filter(|entry| entry.parent.as_deref() == Some(lead))
            .map(|entry| entry.agent_id.clone())
            .collect()
    }

    /// Whether `lead` started `agent_id`, or started an agent that did.
    pub(crate) fn leads(&self, lead: &str, agent_id: &str) -> bool {
        self.book.locked().leads(lead, agent_id)
    }

    /// The agent that started `agent_id`, the one that started that, and so on up.
    pub(crate) fn leads_of(&self, agent_id: &str) -> Vec<String> {
        let book = self.book.locked();
        let mut leads = Vec::new();
        let mut current = agent_id;
        for _ in 0..=book.agents.len() + book.closed_leads.len() {
            match book.parent(current) {
                Some(parent) => {
                    leads.push(parent.to_owned());
                    current = parent;
                }
                None => break,
            }
        }
        leads
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
                profile: entry.profile.clone(),
                terminal: entry.terminal,
                title: entry.title.clone(),
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

    /// Whether the agent is among `open`, or closed with its session since taken
    /// up again by one that is, under a new id.
    pub(crate) fn still_open(&self, agent_id: &str, open: &HashSet<String>) -> bool {
        if open.contains(agent_id) {
            return true;
        }
        let Some(session) = self.last_session(agent_id) else {
            return false;
        };
        (self.book.locked().agents.iter()).any(|entry| {
            open.contains(&entry.agent_id) && entry.session_id.as_deref() == Some(&*session)
        })
    }

    /// The session of the newest closed agent with the id or name `wanted` that `lead` started,
    /// or started an agent that did.
    pub(crate) fn closed_worker(&self, lead: &str, wanted: &str) -> Option<RanSession> {
        let book = self.book.locked();
        book.closed_sessions
            .iter()
            .find(|closed| {
                (closed.agent_id == wanted
                    || closed
                        .name
                        .as_deref()
                        .is_some_and(|called| same(called, wanted)))
                    && closed
                        .lead
                        .as_deref()
                        .is_some_and(|by| by == lead || book.leads(lead, by))
            })
            .map(|closed| closed.ran.clone())
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

    /// The agents here that run or recently ran `session_id`.
    pub(crate) fn agents_of_session(&self, session_id: &str) -> HashSet<String> {
        let book = self.book.locked();
        let open = (book.agents.iter())
            .filter(|entry| entry.session_id.as_deref() == Some(session_id))
            .map(|entry| entry.agent_id.clone());
        let closed = (book.closed_sessions.iter())
            .filter(|closed| closed.ran.session_id == session_id)
            .map(|closed| closed.agent_id.clone());
        open.chain(closed).collect()
    }

    /// Keeps the account each agent of `runs` runs on, by profile id, and whether it runs in a
    /// terminal, so both are still known once the agent closes.
    pub(crate) fn note_runs(&self, runs: Vec<(String, Option<String>, bool)>) {
        let mut book = self.book.locked();
        let mut changed = false;
        for (agent_id, profile, terminal) in runs {
            if let Some(entry) = book
                .entry_mut(&agent_id)
                .filter(|entry| entry.profile != profile || entry.terminal != terminal)
            {
                entry.profile = profile;
                entry.terminal = terminal;
                changed = true;
            }
        }
        if changed {
            self.save(&book);
        }
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
        if same(&name, crate::server::chief::NAME) {
            return Err(format!("{name} is the name of the project's chief"));
        }
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
        for closed in &mut book.closed_sessions {
            if closed.ran.project == project
                && closed.name.as_deref().is_some_and(|held| same(held, &name))
            {
                closed.name = None;
            }
        }
        let entry = book
            .entry_mut(agent_id)
            .ok_or("Sikemux has not named that agent yet")?;
        entry.name = name;
        entry.chosen = true;
        let named = entry.view();
        self.save(&book);
        Ok(named)
    }

    /// The agent id a chat taken up again from `session` comes back as: the closed agent that
    /// last ran it, unless an agent here has that id now, else a new id. `open` holds the
    /// agents open here.
    pub(crate) fn returning_id(
        &self,
        provider: Option<&str>,
        session: &str,
        open: HashSet<String>,
    ) -> String {
        let mut book = self.book.locked();
        let open = self.open_with(open);
        if book.release(&open) {
            self.save(&book);
        }
        let book = &*book;
        (book.closed_sessions.iter())
            .find(|closed| {
                closed.ran.session_id == session
                    && provider.is_none_or(|provider| closed.ran.provider == provider)
            })
            .map(|closed| &closed.agent_id)
            .filter(|id| {
                !open.contains(*id) && !book.agents.iter().any(|entry| &entry.agent_id == *id)
            })
            .map_or_else(new_agent_id, String::clone)
    }

    /// Brings a closed agent back as itself when its chat opens again under its id, and the
    /// character it wore. `open` holds the agents open here.
    pub(crate) fn come_back(
        &self,
        agent_id: &str,
        project: &str,
        open: HashSet<String>,
    ) -> Option<Returned> {
        let mut book = self.book.locked();
        let open = self.open_with(open);
        book.release(&open);
        let away = self.pinned.locked().clone();
        let returned = book.restore(agent_id, project, &open, &away)?;
        self.save(&book);
        Some(returned)
    }

    /// Keeps how the app draws the agent and what it works on, for when it is
    /// opened again after closing. What the app leaves out stays as it was.
    pub(crate) fn note_look(
        &self,
        agent_id: &str,
        buddy: Option<AgentBuddy>,
        title: Option<String>,
    ) {
        let buddy = buddy.filter(|buddy| {
            serde_json::to_vec(buddy).is_ok_and(|bytes| bytes.len() <= MAX_BUDDY_BYTES)
        });
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
        (book.closed_sessions.iter())
            .filter(|closed| {
                closed.chosen && closed.ran.project == project && !open.contains(&closed.agent_id)
            })
            .filter_map(|closed| {
                Some(ClosedAgent {
                    agent_id: closed.agent_id.clone(),
                    name: closed.name.clone()?,
                    provider: closed.ran.provider.clone(),
                    session_id: closed.ran.session_id.clone(),
                    title: closed.ran.title.clone(),
                    buddy: closed.buddy.clone(),
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
                self.closed_sessions.retain(|closed| {
                    closed.agent_id != entry.agent_id
                        && (closed.ran.provider != provider || closed.ran.session_id != session_id)
                });
                self.closed_sessions.push_front(ClosedSession {
                    agent_id: entry.agent_id.clone(),
                    ran: RanSession {
                        provider,
                        session_id,
                        project: entry.project.clone(),
                        profile: entry.profile.clone(),
                        terminal: entry.terminal,
                        title: entry.title.clone(),
                    },
                    name: Some(entry.name.clone()),
                    lead: entry.parent.clone(),
                    quiet: entry.quiet,
                    blind: entry.blind,
                    by_auto: entry.by_auto,
                    chosen: entry.chosen,
                    buddy: entry.buddy.clone(),
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

    fn started_open(&self, project: &str, open: &HashSet<String>) -> usize {
        (self.agents.iter())
            .filter(|entry| open.contains(&entry.agent_id) && entry.project == project)
            .filter(|entry| entry.parent.is_some() || entry.by_auto || entry.blind)
            .count()
    }

    /// The agent's name, and whether the book changed to give it.
    fn give(
        &mut self,
        starting: Starting<'_>,
        open: &HashSet<String>,
        away: &HashSet<String>,
    ) -> (AgentName, bool) {
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
        if self.restore(agent_id, &project, open, away).is_some() {
            let entry = self
                .entry_mut(agent_id)
                .expect("restore puts the agent back in the book");
            if let Some(provider) = provider {
                entry.provider = Some(provider.to_owned());
            }
            if let Some(session) = resume_id {
                entry.session_id = Some(session.to_owned());
            }
            return (entry.view(), true);
        }
        let name = self.fresh(&project, agent_id, open);
        let entry = Entry {
            agent_id: agent_id.to_owned(),
            name,
            project,
            parent: None,
            quiet: false,
            blind: false,
            by_auto: false,
            provider: provider.map(str::to_owned),
            session_id: resume_id.map(str::to_owned),
            above: 0,
            at: epoch_ms(),
            seen: false,
            chosen: false,
            buddy: None,
            title: None,
            profile: None,
            terminal: false,
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

    /// Puts the closed agent `agent_id` back in the book as it closed, in `project`, under its
    /// old name unless an agent of the project has it now. A lead running on a linked host in
    /// `away` was told it had gone, so it comes back with none.
    fn restore(
        &mut self,
        agent_id: &str,
        project: &str,
        open: &HashSet<String>,
        away: &HashSet<String>,
    ) -> Option<Returned> {
        if self.agents.iter().any(|entry| entry.agent_id == agent_id) {
            return None;
        }
        let index = (self.closed_sessions.iter()).position(|closed| closed.agent_id == agent_id)?;
        let closed = self.closed_sessions.remove(index)?;
        self.closed_sessions.retain(|other| {
            other.ran.provider != closed.ran.provider
                || other.ran.session_id != closed.ran.session_id
        });
        let kept = (closed.name).filter(|name| !self.taken(project, name, agent_id));
        let same_name = kept.is_some();
        let chosen = closed.chosen && same_name;
        let name = kept.unwrap_or_else(|| self.fresh(project, agent_id, open));
        let buddy = closed.buddy.clone();
        let lead = closed
            .lead
            .filter(|lead| !away.contains(lead) && self.knows(lead));
        self.agents.push(Entry {
            agent_id: closed.agent_id,
            name,
            project: project.to_owned(),
            parent: lead,
            quiet: closed.quiet,
            blind: closed.blind,
            by_auto: closed.by_auto,
            provider: Some(closed.ran.provider),
            session_id: Some(closed.ran.session_id),
            above: 0,
            at: epoch_ms(),
            seen: false,
            chosen,
            buddy: closed.buddy,
            title: closed.ran.title,
            profile: closed.ran.profile,
            terminal: closed.ran.terminal,
        });
        Some(Returned { buddy, same_name })
    }

    /// Whether the agent is named here, or closed here with its record still kept.
    fn knows(&self, agent_id: &str) -> bool {
        self.agents.iter().any(|entry| entry.agent_id == agent_id)
            || self.closed_leads.contains_key(agent_id)
            || (self.closed_sessions.iter()).any(|closed| closed.agent_id == agent_id)
    }

    /// A word no open or closed agent of the project has, starting differently from the open
    /// ones' so they are told apart at a glance. With none left, the name of the project's
    /// longest-closed agent the person did not name.
    fn fresh(&mut self, project: &str, agent_id: &str, open: &HashSet<String>) -> String {
        let others: Vec<&Entry> = self
            .agents
            .iter()
            .filter(|entry| entry.project == project && entry.agent_id != agent_id)
            .collect();
        let closed_names = (self.closed_sessions.iter())
            .filter(|closed| closed.ran.project == project && closed.agent_id != agent_id)
            .filter_map(|closed| closed.name.as_deref());
        let used: HashSet<String> = (others.iter().map(|entry| entry.name.as_str()))
            .chain(closed_names)
            .map(str::to_lowercase)
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
        let taken_by = |name: &str| others.iter().any(|entry| same(&entry.name, name));
        let longest_closed = (self.closed_sessions.iter_mut().rev()).find(|closed| {
            closed.ran.project == project
                && closed.agent_id != agent_id
                && !closed.chosen
                && closed.name.as_deref().is_some_and(|name| !taken_by(name))
        });
        if let Some(name) = longest_closed.and_then(|closed| closed.name.take()) {
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
            entry.seen || now.saturating_sub(entry.at) >= STARTING_MS
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

#[derive(Serialize, Deserialize)]
pub struct NameThemeParams {
    pub theme: NameTheme,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NameAgentParams {
    agent_id: String,
    cwd: PathBuf,
    provider: Option<String>,
    resume_id: Option<String>,
    /// How the app draws it, kept for when it is opened again.
    buddy: Option<AgentBuddy>,
    title: Option<String>,
}

/// An agent's name, with the character it wore before it closed when it
/// opens again under a name the person chose.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NamedAgent {
    #[serde(flatten)]
    named: AgentName,
    #[serde(skip_serializing_if = "Option::is_none")]
    buddy: Option<AgentBuddy>,
}

/// The name the core gave an agent the app starts or opens again, given now
/// when it has none.
pub(crate) async fn name_agent(
    core: &Arc<Core>,
    params: NameAgentParams,
) -> CoreResult<NamedAgent> {
    if !crate::cli::protocol::is_tool_agent_id(&params.agent_id) {
        return Err("that is not an agent id".into());
    }
    if !params.cwd.is_absolute() {
        return Err("an agent's folder must be an absolute path".into());
    }
    let core = core.clone();
    blocking(move || {
        let (named, kept) = core.name_agent(
            &params.agent_id,
            &params.cwd,
            params.provider.as_deref(),
            params.resume_id.as_deref(),
        );
        let buddy = match kept.and_then(|back| back.buddy) {
            Some(buddy) => Some(buddy),
            None => {
                core.names()
                    .note_look(&params.agent_id, params.buddy, params.title);
                None
            }
        };
        Ok(NamedAgent { named, buddy })
    })
    .await
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameAgentParams {
    pub agent_id: String,
    pub name: String,
}

pub(crate) async fn rename_agent(
    core: &Arc<Core>,
    params: RenameAgentParams,
) -> CoreResult<AgentName> {
    if crate::server::chief::is_chief(core, &params.agent_id) {
        return Err("@Chief keeps its name".into());
    }
    let core = core.clone();
    blocking(move || {
        let named = core
            .names()
            .rename(&params.agent_id, &params.name, core.open_agents())?;
        core.changed(What::View);
        Ok(named)
    })
    .await
}

/// The names of the agents running here and of the workers they started on
/// linked hosts, each with the agent that started it, so a device can list
/// workers after their lead.
pub(crate) async fn agent_names(core: &Arc<Core>) -> CoreResult<Vec<AgentName>> {
    let core = core.clone();
    blocking(move || {
        let mut agents = core.running_agents();
        agents.extend(core.hosts().team.ids());
        Ok(core.names().of(&agents))
    })
    .await
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedAgentsParams {
    pub cwd: PathBuf,
}

/// The closed agents of the project at `cwd` that keep the names the person
/// gave them, newest first.
pub(crate) async fn closed_agents(
    core: &Arc<Core>,
    params: ClosedAgentsParams,
) -> CoreResult<Vec<ClosedAgent>> {
    let core = core.clone();
    blocking(move || {
        let project = core.workspaces().project_of(&params.cwd);
        Ok(core.names().closed(&project, core.open_agents()))
    })
    .await
}

pub(crate) fn name_theme(core: &Core) -> NameThemeParams {
    NameThemeParams {
        theme: core.names().theme(),
    }
}

pub(crate) fn set_name_theme(core: &Core, params: NameThemeParams) -> NameThemeParams {
    core.names().set_theme(params.theme);
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_reopened_from_its_session_comes_back_as_the_same_agent() {
        let names = Names::new(None);
        let lead = Starting {
            agent_id: "agent-lead",
            project: "/p".into(),
            provider: Some("claude"),
            resume_id: None,
        };
        names.give(lead, HashSet::from(["agent-lead".to_owned()]));
        let worker = |agent_id| Worker {
            agent_id,
            lead: "agent-lead",
            project: None,
            provider: "claude",
            session: Some("ses-1"),
            reports: true,
        };
        let open = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect();
        let first = names
            .hire(worker("agent-w"), open(&["agent-lead"]))
            .ok()
            .unwrap();
        names.settle(open(&["agent-lead", "agent-w"]));
        names.settle(open(&["agent-lead"]));
        let back = names.returning_id(Some("claude"), "ses-1", open(&["agent-lead"]));
        assert_eq!(back, "agent-w");
        let again = names
            .hire(worker(&back), open(&["agent-lead"]))
            .ok()
            .unwrap();
        assert_eq!(again, first);
    }

    #[test]
    fn a_chat_the_person_opens_again_comes_back_as_the_same_agent() {
        let names = Names::new(None);
        let starting = |agent_id| Starting {
            agent_id,
            project: "/p".into(),
            provider: Some("claude"),
            resume_id: Some("ses-1"),
        };
        let open = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect();
        let (first, _) = names.give(starting("agent-a"), open(&["agent-a"]));
        names.settle(open(&[]));
        let other = Starting {
            resume_id: Some("ses-2"),
            ..starting("agent-b")
        };
        let (other, _) = names.give(other, open(&["agent-b"]));
        assert_ne!(other.name, first.name);
        let back = names.returning_id(Some("claude"), "ses-1", open(&["agent-b"]));
        assert_eq!(back, "agent-a");
        let returned = names.come_back(&back, "/p", open(&["agent-b"])).unwrap();
        assert!(returned.same_name);
        let (again, _) = names.give(starting(&back), open(&["agent-a", "agent-b"]));
        assert_eq!(again, first);
    }
}
