//! What a host's sealed notification holds once the phone opens it. The phone's
//! notification code reads it in Swift and Kotlin as well as TypeScript, all
//! generated from this file.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationKind {
    /// A chat agent asks to do something; the card may carry Allow and Reject.
    Permission,
    /// A terminal agent waits for the person to type.
    Input,
    /// A turn that ran a while has ended.
    Finished,
    /// An agent reported an error or stopped unexpectedly.
    Problem,
    /// A device asks to pair with the host; tapping opens where it is let in.
    Pairing,
    /// Removes the card with the same `collapseId`; shows nothing.
    Clear,
}

/// The Android channel a card goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationChannel {
    NeedsYou,
    Finished,
    Problems,
}

/// The iOS category, which decides the card's buttons. Only `permission`
/// has any: Reject and Allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationCategory {
    Permission,
    NeedsYou,
    Finished,
    Problem,
}

impl NotificationKind {
    pub fn channel(self) -> Option<NotificationChannel> {
        match self {
            Self::Permission | Self::Input | Self::Pairing => Some(NotificationChannel::NeedsYou),
            Self::Finished => Some(NotificationChannel::Finished),
            Self::Problem => Some(NotificationChannel::Problems),
            Self::Clear => None,
        }
    }
}

/// What the phone reads once it opens a sealed notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationCard {
    pub v: u8,
    pub kind: NotificationKind,
    /// `None` for `clear`.
    pub channel: Option<NotificationChannel>,
    /// `None` for `clear`. `permission` only when both answers are offered.
    pub category: Option<NotificationCategory>,
    /// The same id the push carried, for removing this card later.
    pub collapse_id: String,
    /// Groups one agent's cards: `<hostKey>/<agentId>`.
    pub thread: String,
    pub host_key: String,
    pub host_name: String,
    pub agent_id: String,
    pub provider: String,
    pub chat_title: Option<String>,
    pub title: String,
    pub body: String,
    /// One line to show in monospace, such as the command an agent wants to run.
    pub detail: Option<String>,
    /// Where tapping the card goes.
    pub url: String,
    /// The permission request to answer with `answer_permission`.
    pub request_id: Option<String>,
    pub allow_option_id: Option<String>,
    pub reject_option_id: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub expires_at: u64,
    /// The call it puts to the person, answered with `answerCall`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// The answers a call of taste offers as buttons, the asker's pick first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<NotificationOption>,
}

/// One answer a card offers, by the call option it picks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationOption {
    pub id: String,
    pub label: String,
}

/// The agents on one host at a glance, as a phone's Live Activity and widgets show them however many there
/// are: the counts, then what needs the person, else the projects most at work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityGlance {
    /// How many things wait on the person.
    pub needed: u32,
    /// How many agents work.
    pub working: u32,
    /// How many finished a turn the person has not seen.
    pub done: u32,
    /// What needs the person, as the rail ranks it, each team folded into its lead's row.
    pub rows: Vec<ActivityRow>,
    /// How many more need the person than `rows` shows.
    pub more: u32,
    /// While nothing needs the person: the projects most at work.
    pub projects: Vec<ActivityProject>,
}

/// One agent, or a team under its lead, that needs the person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    /// The agent that needs the person, which tapping the row opens.
    pub agent_id: String,
    /// The CLI the row's agent or lead runs, such as `claude`, whose mark it shows.
    pub cli: String,
    pub title: String,
    /// What the core calls the row's agent or lead, such as Otter.
    pub name: Option<String>,
    /// How many of the lead's team work, for a team's row.
    pub team_working: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityProject {
    pub name: String,
    pub working: u32,
}

impl ActivityGlance {
    /// How many rows a glance lists at most.
    pub const ROWS: usize = 3;
    /// How many projects a glance lists at most.
    pub const PROJECTS: usize = 2;

    /// Whether anything works or waits on the person, so a Live Activity has reason to show.
    pub fn active(&self) -> bool {
        self.needed > 0 || self.working > 0
    }

    /// The glance of the inbox's agents, in the order the rail ranks them, with each lead's team after it.
    pub fn of(inbox: &crate::inbox::Inbox) -> Self {
        use crate::inbox::TeamLink;
        use sikemux_wire::protocol::AgentGroup;
        let shown: Vec<_> = (inbox.agents.iter())
            .filter(|agent| agent.settled_at.is_none())
            .collect();
        let mut units: Vec<Vec<&crate::inbox::AgentShown>> = Vec::new();
        let mut in_team = false;
        for agent in &shown {
            match agent.team {
                Some(TeamLink::Member | TeamLink::Last) if in_team => {
                    if let Some(team) = units.last_mut() {
                        team.push(agent);
                    }
                    in_team = agent.team == Some(TeamLink::Member);
                }
                link => {
                    units.push(vec![agent]);
                    in_team = link == Some(TeamLink::Head);
                }
            }
        }
        let needing: Vec<ActivityRow> = (units.iter())
            .filter_map(|unit| {
                let asking = unit
                    .iter()
                    .find(|agent| agent.group == AgentGroup::Blocked)?;
                let lead = unit.first()?;
                let team_working = (unit.iter().skip(1))
                    .filter(|agent| agent.group == AgentGroup::Working)
                    .count();
                Some(ActivityRow {
                    agent_id: asking.agent_id.clone(),
                    cli: lead.cli.clone(),
                    title: lead.title.clone(),
                    name: lead.name.clone(),
                    team_working: u32::try_from(team_working).unwrap_or(u32::MAX),
                })
            })
            .collect();
        let mut projects: Vec<ActivityProject> = Vec::new();
        if needing.is_empty() {
            for agent in shown
                .iter()
                .filter(|agent| agent.group == AgentGroup::Working)
            {
                let name = agent.project_name.clone().unwrap_or_default();
                match projects.iter_mut().find(|project| project.name == name) {
                    Some(project) => project.working += 1,
                    None => projects.push(ActivityProject { name, working: 1 }),
                }
            }
            projects.truncate(Self::PROJECTS);
        }
        let more = needing.len().saturating_sub(Self::ROWS);
        let small = |count: usize| u32::try_from(count).unwrap_or(u32::MAX);
        Self {
            needed: small(inbox.needed.max(needing.len())),
            working: small(inbox.working),
            done: small(
                shown
                    .iter()
                    .filter(|agent| agent.group == AgentGroup::Done)
                    .count(),
            ),
            rows: needing.into_iter().take(Self::ROWS).collect(),
            more: small(more),
            projects,
        }
    }
}
