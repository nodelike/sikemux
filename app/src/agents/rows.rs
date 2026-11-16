//! An agent as the rails and tabs show it, and which agents each place lists.

use gpui_kit::{AnyElement, App};
use sikemux_calls::call;
use sikemux_calls::workspace::PinnedParams;
use sikemux_view::device::{DeviceView, RecentInfo};
use sikemux_view::inbox::{AgentShown, RemoteAgent};
use sikemux_view::layout::PaneKind;
use sikemux_view::workspace::{AgentFace, PinnedChat, ProjectInfo};
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
    /// Whether the chat has a title of its own, drawn with the name beside it; an untitled one
    /// shows its `@name` as the title.
    pub titled: bool,
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
    /// When it started, which orders the rows, in Unix milliseconds.
    pub started_at: Option<u64>,
    /// When the soonest trigger set on it sends it a prompt, in Unix milliseconds.
    pub next_trigger: Option<u64>,
    /// How many triggers it set on its own chat, which pause when it closes.
    pub triggers: u32,
    /// Where a heavy command it started stands in the host's line, while it waits.
    pub line: Option<sikemux_view::inbox::InLine>,
    /// When it was last named with `@` or messaged by another agent, in Unix milliseconds.
    pub called_at: Option<u64>,
    /// The agent it hangs under, by id: the one that spawned it, or the nearest above that.
    pub lead: Option<String>,
    /// The host it runs on.
    pub place: Place,
    /// That host answers now. One that does not leaves the row as it last said.
    pub reachable: bool,
    /// It is out of the main list until then, in Unix milliseconds.
    pub snoozed_until: Option<u64>,
    /// When it was settled out of the main list.
    pub settled_at: Option<u64>,
    /// How its turn stands, as the core said; none before its first turn.
    pub turn: Option<sikemux_view::turn::TurnState>,
    /// It is its project's chief, @Chief.
    pub chief: bool,
    /// The person's labels for it, lower case.
    pub tags: Vec<String>,
}

impl AgentRow {
    /// Its face, or its agent's mark while it wears none, `size` across; the chief wears
    /// Sikemux's own mark.
    pub fn mark(&self, size: f32, cx: &App) -> AnyElement {
        if self.chief {
            return crate::ui::chief_mark(size);
        }
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
    /// Where calls about `row`'s agent go, as [`asked_place`] picks.
    pub fn asked_at(&self, row: &AgentRow) -> Place {
        let views = std::iter::once((Place::This, &self.feeds.this))
            .chain((self.feeds.hosts()).map(|(host, feed)| (Place::Account(host.clone()), feed)))
            .filter_map(|(place, feed)| Some((place, feed.view.as_ref()?)));
        let lists = |view: &DeviceView| {
            (view.inbox.remote_agents.iter()).any(|worker| worker.shown.agent_id == row.id)
        };
        asked_place(
            &row.place,
            self.view_at(&row.place).is_some(),
            views.map(|(place, view)| (place, lists(view))),
        )
    }

    /// The worker on a linked host this host's core lists under `id`, as its row.
    pub fn worker_row(&self, id: &str) -> Option<AgentRow> {
        let workers = &self.this_view()?.inbox.remote_agents;
        worker(workers.iter().find(|worker| worker.shown.agent_id == id)?)
    }

    /// The agent the tab in front shows.
    pub fn agent(&self) -> Option<AgentRow> {
        let project = self.project()?;
        let target = shows(self.tab()?, PaneKind::Agent)?;
        self.agent_in(&project.id, target)
    }

    /// The agent `id` of the project `project` on the host in front.
    pub fn agent_in(&self, project: &str, id: &str) -> Option<AgentRow> {
        let place = self.place();
        rows_of(self.view_at(&place)?)
            .find(|row| row.agent_id == id && row.project.as_deref() == Some(project))
            .and_then(|row| agent_row(row, &place))
    }

    pub fn agent_by_id(&self, id: &str) -> Option<AgentRow> {
        let place = self.place();
        rows_of(self.view_at(&place)?)
            .find(|row| row.agent_id == id)
            .and_then(|row| agent_row(row, &place))
    }

    /// The project's agents on the host in front, in the order the core lists them.
    pub fn agents_of(&self, project: &str) -> Vec<AgentRow> {
        self.agents_at(&self.place(), project)
    }

    /// The agents of the project `project` of the host at `place`, as its core orders them.
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
        let rows: Vec<AgentRow> = if self.rails.all_projects {
            self.folded()
                .iter()
                .flat_map(|folded| self.agents_of_folded(folded))
                .collect()
        } else {
            self.folded_in_front()
                .map(|folded| self.agents_of_folded(&folded))
                .unwrap_or_default()
        };
        let mut rows: Vec<(usize, AgentRow)> = (rows.into_iter())
            .filter_map(|row| Some((self.filter_place(Some(&row.id), None, &row.title)?, row)))
            .collect();
        rows.sort_by_key(|(place, _)| *place);
        let mut rows: Vec<AgentRow> = rows.into_iter().map(|(_, row)| row).collect();
        if let Some(tag) = &self.rails.tag {
            rows.retain(|row| row.tags.contains(tag));
        }
        rows
    }

    /// The project's saved chats no agent holds, which the agent list already shows.
    pub fn recent_of(&self, project: &str) -> Vec<&RecentInfo> {
        self.chats_of(project)
            .into_iter()
            .filter(|recent| recent.agent_id.is_none())
            .collect()
    }

    /// How many saved chats Recent has for the project in all, loaded or not; none while
    /// the filter narrows them.
    pub fn recent_total(&self, project: &ProjectInfo) -> Option<usize> {
        if !self.filter_text().is_empty() {
            return None;
        }
        let provider = self.launcher().map(|launcher| launcher.provider.as_str());
        let saved: usize = (project.saved_chats.iter())
            .filter(|(kind, _)| self.rails.all_harnesses || provider == Some(kind.as_str()))
            .map(|(_, count)| count)
            .sum();
        let pinned_apart = match self.rails.all_projects {
            true => 0,
            false => (self.pinned_of(&project.id).iter())
                .filter(|pin| pin.agent_id.is_none())
                .count(),
        };
        Some(saved.saturating_sub(pinned_apart))
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
        let mut chats: Vec<(usize, &RecentInfo)> = (self.saved_chats(project).into_iter())
            .filter(|recent| self.rails.all_harnesses || provider == Some(recent.provider.as_str()))
            .filter_map(|recent| {
                let agent = recent.agent_id.as_deref();
                let place = self.filter_place(agent, Some(&recent.id), &recent.title)?;
                Some((place, recent))
            })
            .collect();
        chats.sort_by_key(|(place, _)| *place);
        chats.into_iter().map(|(_, chat)| chat).collect()
    }
}

/// Every agent the view lists, running or kept.
fn rows_of(view: &DeviceView) -> impl Iterator<Item = &AgentShown> {
    view.inbox.agents.iter().chain(&view.inbox.kept)
}

/// Each host's ranked rows as one list: a lead keeps its workers after it, and the lists are
/// taken by whose next row started first, as each core orders its own.
fn merged(lists: Vec<Vec<AgentRow>>) -> Vec<AgentRow> {
    let mut teams: Vec<std::collections::VecDeque<Vec<AgentRow>>> = (lists.into_iter())
        .map(|list| {
            let mut teams: std::collections::VecDeque<Vec<AgentRow>> = Default::default();
            for row in list {
                let lead = row.lead.as_deref();
                match teams.back_mut() {
                    Some(team) if team.iter().any(|mate| Some(mate.id.as_str()) == lead) => {
                        team.push(row)
                    }
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
            .min_by_key(|(at, head)| (head.started_at.unwrap_or(u64::MAX), *at))
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
            _ if row.chief => CHIEF.to_owned(),
            Some(name) if !row.titled => format!("@{name}"),
            _ => row.title.clone(),
        },
        name: row.name.clone(),
        titled: row.titled && !row.chief,
        status: row.status,
        subagents: row.subagents,
        tasks: row.tasks,
        project: row.project.clone()?,
        face: row.face.clone(),
        group: row.group,
        busy: row.busy,
        background: row.background,
        active_at: row.active_at.unwrap_or(0),
        started_at: row.started_at,
        next_trigger: row.next_trigger,
        triggers: row.triggers,
        line: row.line.clone(),
        called_at: row.called_at,
        lead: row.lead.clone(),
        place: place.clone(),
        reachable: true,
        snoozed_until: row.snoozed_until,
        settled_at: row.settled_at,
        turn: row.turn.clone(),
        chief: row.chief,
        tags: row.tags.clone(),
    })
}

/// A worker on a linked host as its lead's host lists it, for when the feed of the host it runs
/// on does not list it here.
/// Which core a row's calls go to: the first that lists its agent as a worker of its own
/// agents, as that core keeps the row and passes calls on; else the host the agent runs on
/// while this window `holds` its view; else this host's core. `cores` says, for each core
/// this window holds a view of, whether it lists the agent as a worker.
pub fn asked_place(
    runs_on: &Place,
    holds: bool,
    cores: impl IntoIterator<Item = (Place, bool)>,
) -> Place {
    match cores.into_iter().find(|(_, lists)| *lists) {
        Some((place, _)) => place,
        None if holds => runs_on.clone(),
        None => Place::This,
    }
}

pub fn worker(worker: &RemoteAgent) -> Option<AgentRow> {
    let mut row = agent_row(&worker.shown, &Place::Account(worker.host_id.clone()))?;
    row.reachable = worker.reachable;
    Some(row)
}

impl AgentRow {
    /// The name drawn beside the title, which an untitled row already shows as its title.
    pub fn handle(&self) -> Option<&str> {
        self.name.as_deref().filter(|_| self.titled)
    }

    /// The quiet words after its title: the chief's role, else its `@name` when titled.
    pub fn beside(&self) -> Option<String> {
        match self.chief {
            true => Some(chief_role()),
            false => self.handle().map(|name| format!("@{name}")),
        }
    }
}

/// What every list titles a project's chief.
pub const CHIEF: &str = "@Chief";

/// The chief's role, drawn after its title.
pub fn chief_role() -> String {
    t!("Chief of staff").into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_row_seen_through_its_hosts_view_asks_the_core_that_lists_it() {
        let nexus = Place::Account("nexus".into());
        let cores = [(Place::This, true), (nexus.clone(), false)];
        assert_eq!(asked_place(&nexus, true, cores), Place::This);
    }

    #[test]
    fn a_worker_another_host_lists_is_asked_through_that_host() {
        let (nexus, temple) = (
            Place::Account("nexus".into()),
            Place::Account("temple".into()),
        );
        let cores = [
            (Place::This, false),
            (nexus.clone(), true),
            (temple.clone(), false),
        ];
        assert_eq!(asked_place(&temple, true, cores), nexus);
    }

    #[test]
    fn an_agent_no_core_lists_as_a_worker_is_asked_where_it_runs() {
        let nexus = Place::Account("nexus".into());
        let cores = [(Place::This, false), (nexus.clone(), false)];
        assert_eq!(asked_place(&nexus, true, cores), nexus);
        assert_eq!(
            asked_place(&nexus, false, [(Place::This, false)]),
            Place::This
        );
    }
}
