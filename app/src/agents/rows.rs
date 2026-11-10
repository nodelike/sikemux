//! An agent as the rails and tabs show it, and which agents each place lists.

use gpui_kit::{AnyElement, App};
use sikemux_core::server::call;
use sikemux_core::server::workspace::PinnedParams;
use sikemux_view::device::{DeviceView, RecentInfo};
use sikemux_view::inbox::{AgentShown, TeamLink};
use sikemux_view::layout::PaneKind;
use sikemux_view::workspace::{AgentFace, PinnedChat};
use sikemux_wire::protocol::{AgentGroup, AgentStatus};

use crate::copies::Folded;
use crate::hosts::Place;
use crate::tabs::shows;
use crate::workbench::Workbench;

#[derive(Clone)]
pub struct AgentRow {
    pub id: String,
    pub provider: String,
    pub title: String,
    pub name: Option<String>,
    pub status: AgentStatus,
    pub subagents: u32,
    pub tasks: u32,
    pub project: String,
    /// The face it wears, else none and its agent's mark is drawn.
    pub face: Option<AgentFace>,
    pub group: AgentGroup,
    /// Mid-turn or waiting on an answer, so stopping it now loses work.
    pub busy: bool,
    /// It still has subagents or shells running.
    pub background: bool,
    /// When it last worked, in Unix milliseconds; 0 when the core does not know.
    pub active_at: u64,
    /// When the soonest trigger set on it sends it a prompt, in Unix milliseconds.
    pub next_trigger: Option<u64>,
    /// How many triggers it set on its own chat, which pause when it closes.
    pub triggers: u32,
    /// A build it started waits for its turn on the host.
    pub build_queued: bool,
    /// When it was last named with `@` or messaged by another agent, in Unix milliseconds.
    pub called_at: Option<u64>,
    /// Its place on its lead's team, which the rows after a lead belong to.
    pub team: Option<TeamLink>,
    /// The host it runs on.
    pub place: Place,
    /// It is out of the main list until then, in Unix milliseconds.
    pub snoozed_until: Option<u64>,
    /// When it was settled out of the main list.
    pub settled_at: Option<u64>,
}

impl AgentRow {
    /// Its face, or its agent's mark while it wears none, `size` across.
    pub fn mark(&self, size: f32, cx: &App) -> AnyElement {
        crate::ui::agent_mark(
            &self.id,
            &self.provider,
            self.face.as_ref(),
            self.group,
            size,
            cx,
        )
    }
}

impl Workbench {
    /// The agent the tab in front shows.
    pub fn agent(&self) -> Option<AgentRow> {
        let project = self.project()?;
        let target = shows(self.tab()?, PaneKind::Agent)?;
        self.agents_of(&project.id)
            .into_iter()
            .find(|agent| &agent.id == target)
    }

    pub fn agent_by_id(&self, id: &str) -> Option<AgentRow> {
        let place = self.place();
        rows_of(self.view_at(&place)?)
            .find(|row| row.agent_id == id)
            .and_then(|row| agent_row(row, &place))
    }

    /// The project's agents on the host in front, those running first as the core ranks them.
    pub fn agents_of(&self, project: &str) -> Vec<AgentRow> {
        self.agents_at(&self.place(), project)
    }

    /// The agents of the project `project` of the host at `place`, as its core ranks them.
    pub fn agents_at(&self, place: &Place, project: &str) -> Vec<AgentRow> {
        let Some(view) = self.view_at(place) else {
            return Vec::new();
        };
        rows_of(view)
            .filter(|row| row.project.as_deref() == Some(project))
            .filter_map(|row| agent_row(row, place))
            .collect()
    }

    /// The agents of every copy of `folded`, as one list.
    pub fn agents_of_folded(&self, folded: &Folded) -> Vec<AgentRow> {
        merged(
            (folded.copies.iter())
                .map(|copy| self.agents_at(&copy.place, &copy.project))
                .collect(),
        )
    }

    /// The rows the agent rail lists: this project's, or every project's.
    pub fn listed_agents(&self) -> Vec<AgentRow> {
        let focus = self.rails.goal_focus.as_ref();
        let mut rows: Vec<AgentRow> = if self.rails.all_projects || focus.is_some() {
            self.folded()
                .iter()
                .flat_map(|folded| self.agents_of_folded(folded))
                .collect()
        } else {
            self.folded_in_front()
                .map(|folded| self.agents_of_folded(&folded))
                .unwrap_or_default()
        };
        if let Some(focus) = focus {
            rows.retain(|row| focus.agents.contains(&row.id));
        }
        let filter = self.filter_text();
        rows.retain(|row| row.title.to_lowercase().contains(filter));
        rows
    }

    /// The project's saved chats no agent holds, which the agent list already shows.
    pub fn recent_of(&self, project: &str) -> Vec<&RecentInfo> {
        self.chats_of(project)
            .into_iter()
            .filter(|recent| recent.agent_id.is_none())
            .collect()
    }

    /// The project's pinned chats, held by an agent or not, in the order the person keeps them.
    pub fn pinned_of(&self, project: &str) -> Vec<&RecentInfo> {
        let order = self.pin_order();
        let mut pins: Vec<&RecentInfo> = self
            .chats_of(project)
            .into_iter()
            .filter(|recent| recent.pinned)
            .collect();
        pins.sort_by_key(|pin| order.iter().position(|id| *id == pin.id));
        pins
    }

    /// Every pinned chat, as the core lists it, in the order the person keeps them.
    fn pin_order(&self) -> Vec<String> {
        self.shown().map_or_else(Vec::new, |view| {
            (view.workspace.pinned.iter())
                .map(|pin| format!("{}:{}", pin.provider, pin.session_id))
                .collect()
        })
    }

    /// Moves the pinned chat `chat` to `index` among the project's pinned chats, keeping the
    /// other projects' where they are.
    pub fn move_pin(&self, chat: &str, index: usize) {
        let Some(view) = self.shown() else { return };
        let Some(project) = self.project().map(|project| project.id.clone()) else {
            return;
        };
        let mut shown: Vec<String> = self
            .pinned_of(&project)
            .iter()
            .map(|pin| pin.id.clone())
            .collect();
        let Some(from) = shown.iter().position(|id| id == chat) else {
            return;
        };
        let moved = shown.remove(from);
        shown.insert(index.min(shown.len()), moved);
        let mut pins = view.workspace.pinned.clone();
        let id = |pin: &PinnedChat| format!("{}:{}", pin.provider, pin.session_id);
        let slots: Vec<usize> = (0..pins.len())
            .filter(|&at| shown.contains(&id(&pins[at])))
            .collect();
        let kept = pins.clone();
        for (slot, chat) in slots.iter().zip(&shown) {
            if let Some(pin) = kept.iter().find(|pin| id(pin) == *chat) {
                pins[*slot] = pin.clone();
            }
        }
        if pins != view.workspace.pinned {
            self.target()
                .call_typed::<call::SetPinnedChats>(PinnedParams { chats: pins });
        }
    }

    /// The saved chat the agent holds, once the core lists it.
    pub fn chat_of(&self, agent: &str) -> Option<&RecentInfo> {
        self.shown()?
            .recent
            .iter()
            .find(|recent| recent.agent_id.as_deref() == Some(agent))
    }

    pub(crate) fn chats_of(&self, project: &str) -> Vec<&RecentInfo> {
        let provider = self.launcher().map(|launcher| launcher.provider.as_str());
        self.saved_chats(project)
            .into_iter()
            .filter(|recent| self.rails.all_harnesses || provider == Some(recent.provider.as_str()))
            .filter(|recent| recent.title.to_lowercase().contains(self.filter_text()))
            .collect()
    }
}

/// Every agent the view lists, running or kept.
fn rows_of(view: &DeviceView) -> impl Iterator<Item = &AgentShown> {
    view.inbox.agents.iter().chain(&view.inbox.kept)
}

/// Each host's ranked rows as one list: a lead keeps its workers after it, and the lists are
/// taken by whose next row stands highest, as each core ranks its own.
fn merged(lists: Vec<Vec<AgentRow>>) -> Vec<AgentRow> {
    let mut teams: Vec<std::collections::VecDeque<Vec<AgentRow>>> = (lists.into_iter())
        .map(|list| {
            let mut teams: std::collections::VecDeque<Vec<AgentRow>> = Default::default();
            for row in list {
                match (row.team, teams.back_mut()) {
                    (Some(TeamLink::Member | TeamLink::Last), Some(team)) => team.push(row),
                    _ => teams.push_back(vec![row]),
                }
            }
            teams
        })
        .collect();
    let mut rows = Vec::new();
    loop {
        let next = (teams.iter().enumerate())
            .filter_map(|(at, list)| Some((at, &list.front()?[0])))
            .min_by_key(|(at, head)| (head.group, std::cmp::Reverse(head.active_at), *at))
            .map(|(at, _)| at);
        let Some(at) = next else {
            return rows;
        };
        rows.extend(teams[at].pop_front().unwrap_or_default());
    }
}

/// `row` as the rails and tabs show it, or none for one outside every project, such as an agent
/// the core only noticed running in a terminal.
fn agent_row(row: &AgentShown, place: &Place) -> Option<AgentRow> {
    Some(AgentRow {
        id: row.agent_id.clone(),
        provider: row.cli.clone(),
        title: match &row.name {
            Some(name) if !row.titled => format!("@{name}"),
            _ => row.title.clone(),
        },
        name: row.name.clone().filter(|_| row.titled),
        status: row.status,
        subagents: row.subagents,
        tasks: row.tasks,
        project: row.project.clone()?,
        face: row.face.clone(),
        group: row.group,
        busy: row.busy,
        background: row.background,
        active_at: row.active_at.unwrap_or(0),
        next_trigger: row.next_trigger,
        triggers: row.triggers,
        build_queued: row.build_queued_at.is_some(),
        called_at: row.called_at,
        team: row.team,
        place: place.clone(),
        snoozed_until: row.snoozed_until,
        settled_at: row.settled_at,
    })
}
