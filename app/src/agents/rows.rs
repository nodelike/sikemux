//! An agent as the rails and tabs show it, and which agents each place lists.

use gpui_kit::{AnyElement, App};
use sikemux_view::device::{DeviceView, RecentInfo};
use sikemux_view::inbox::{AgentShown, TeamLink};
use sikemux_view::layout::PaneKind;
use sikemux_view::workspace::AgentFace;
use sikemux_wire::protocol::{AgentGroup, AgentStatus};

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
    /// When it was last named with `@` or messaged by another agent, in Unix milliseconds.
    pub called_at: Option<u64>,
    /// Its place on its lead's team, which the rows after a lead belong to.
    pub team: Option<TeamLink>,
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
        rows_of(self.shown()?)
            .find(|row| row.agent_id == id)
            .and_then(agent_row)
    }

    /// The project's agents, those running first as the core ranks them.
    pub fn agents_of(&self, project: &str) -> Vec<AgentRow> {
        let Some(view) = self.shown() else {
            return Vec::new();
        };
        rows_of(view)
            .filter(|row| row.project.as_deref() == Some(project))
            .filter_map(agent_row)
            .collect()
    }

    /// The rows the agent rail lists: this project's, or every project's.
    pub fn listed_agents(&self) -> Vec<AgentRow> {
        let focus = self.rails.goal_focus.as_ref();
        let mut rows: Vec<AgentRow> = if self.rails.all_projects || focus.is_some() {
            self.projects()
                .iter()
                .flat_map(|project| self.agents_of(&project.id))
                .collect()
        } else {
            self.project()
                .map(|project| self.agents_of(&project.id))
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

    /// The project's pinned chats, held by an agent or not.
    pub fn pinned_of(&self, project: &str) -> Vec<&RecentInfo> {
        self.chats_of(project)
            .into_iter()
            .filter(|recent| recent.pinned)
            .collect()
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

/// `row` as the rails and tabs show it, or none for one outside every project, such as an agent
/// the core only noticed running in a terminal.
fn agent_row(row: &AgentShown) -> Option<AgentRow> {
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
        called_at: row.called_at,
        team: row.team,
    })
}
