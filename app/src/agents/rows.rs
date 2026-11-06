//! An agent as the rails and tabs show it, and which agents each place lists.

use gpui_kit::{AnyElement, App};
use sikemux_view::device::{DeviceView, RecentInfo};
use sikemux_view::inbox::TeamLink;
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
        let view = self.shown()?;
        let agent = view.workspace.agents.iter().find(|agent| agent.id == id)?;
        Some(self.agent_row(view, agent))
    }

    pub fn agents_of(&self, project: &str) -> Vec<AgentRow> {
        let Some(view) = self.shown() else {
            return Vec::new();
        };
        let ranked: Vec<&str> = view
            .inbox
            .agents
            .iter()
            .map(|agent| agent.agent_id.as_str())
            .collect();
        let mut rows: Vec<AgentRow> = view
            .workspace
            .agents
            .iter()
            .filter(|agent| agent.project == project)
            .map(|agent| self.agent_row(view, agent))
            .collect();
        rows.sort_by_key(|row| {
            ranked
                .iter()
                .position(|id| *id == row.id)
                .unwrap_or(usize::MAX)
        });
        rows
    }

    pub(crate) fn agent_row(
        &self,
        view: &DeviceView,
        agent: &sikemux_view::workspace::WorkspaceAgent,
    ) -> AgentRow {
        let shown = (view.inbox.agents.iter())
            .chain(&view.inbox.kept)
            .find(|row| row.agent_id == agent.id);
        let status = shown.map_or(AgentStatus::Idle, |row| row.status);
        AgentRow {
            active_at: shown.and_then(|row| row.active_at).unwrap_or(0),
            next_trigger: shown.and_then(|row| row.next_trigger),
            called_at: shown.and_then(|row| row.called_at),
            team: shown.and_then(|row| row.team),
            id: agent.id.clone(),
            provider: agent.provider.clone(),
            title: self.title_of(agent),
            name: agent.name.clone().filter(|_| agent.titled),
            status,
            group: status.group(),
            busy: shown.is_some_and(|row| row.busy),
            background: shown.is_some_and(|row| row.background),
            subagents: shown.map_or(0, |row| row.subagents),
            tasks: shown.map_or(0, |row| row.tasks),
            project: agent.project.clone(),
            face: agent.face.clone(),
        }
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

    /// What the agent is called in its header, tab and rail row: until it has a title of its
    /// own, its @name.
    pub fn title_of(&self, agent: &sikemux_view::workspace::WorkspaceAgent) -> String {
        match &agent.name {
            Some(name) if !agent.titled => format!("@{name}"),
            _ => agent.title.clone(),
        }
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
