//! Shortcuts that move between places and the surfaces of the one in front: closing
//! a project or host, going back to the last one, jumping between projects and hosts,
//! and bringing forward a project's files, agents, terminals or board.

use gpui_kit::{App, Context, Window};
use serde_json::json;
use sikemux_wire::protocol::{AgentStatus, Pane, PaneKind};

use crate::tabs;
use crate::workbench::{RailPage, Strip, Workbench};

/// Defines unit actions named by today's command ids.
macro_rules! commands {
    ($($(#[$doc:meta])* $name:ident = $id:literal,)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Default, PartialEq, gpui::Action)]
        #[action(name = $id)]
        pub struct $name;
    )*};
}

commands! {
    /// Closes the project or host in front, asking first when it would stop agents at
    /// work or lose unsaved files.
    ClosePlace = "session.close",
    /// Goes back to the project or host in front before this one.
    LastPlace = "session.lastUsed",
    /// Jumps from the projects to the hosts, or back.
    NextGroup = "session.nextGroup",
    /// The tab rail's terminals.
    FocusTerminals = "session.command",
    FocusFiles = "window.files",
    FocusAgents = "window.agents",
    FocusBoard = "window.board",
}

impl Workbench {
    /// Notes the place in front, keeping the one before it for [`LastPlace`].
    pub fn note_place(&mut self) {
        let now = self.strip();
        if now != self.place {
            self.last_place = std::mem::replace(&mut self.place, now);
        }
    }

    pub fn go_to_last_place(&self, cx: &mut App) {
        let open = self.strips_in_order();
        if let Some(last) = self.last_place.as_ref().filter(|last| open.contains(last)) {
            self.show_strip(last, cx);
        }
    }

    /// The first place of the next group the rail shows: projects, hosts, then terminals.
    pub fn next_group(&self, cx: &mut App) {
        let group = |strip: &Strip| match strip {
            Strip::Project(_) => 0,
            Strip::Host(_) => 1,
            Strip::Terminals => 2,
        };
        let open = self.strips_in_order();
        let now = self.strip().as_ref().map_or(0, group);
        let next = open
            .iter()
            .filter(|strip| group(strip) != now)
            .min_by_key(|strip| (group(strip) + 3 - now) % 3);
        if let Some(next) = next {
            self.show_strip(next, cx);
        }
    }

    pub fn close_place(&mut self, cx: &mut Context<Self>) {
        let Some(strip) = self.strip() else {
            return;
        };
        let project = match &strip {
            Strip::Host(_) | Strip::Terminals => {
                let tabs: Vec<String> =
                    self.tabs(&strip).iter().map(|tab| tab.id.clone()).collect();
                return self.close_tabs(&strip, &tabs);
            }
            Strip::Project(id) => self
                .projects()
                .iter()
                .find(|project| project.id == *id)
                .cloned(),
        };
        let Some(project) = project.filter(|_| self.projects().len() > 1) else {
            return;
        };
        let working = self
            .agents_of(&project.id)
            .iter()
            .filter(|agent| matches!(agent.status, AgentStatus::Working | AgentStatus::Blocked))
            .count();
        let unsaved = self.editors.dirty_under(&self.place(), &project.path);
        let path = project.path.to_string_lossy().into_owned();
        let close =
            move |bench: &mut Workbench| bench.call("removeProject", json!({ "path": path }));
        if working == 0 && unsaved == 0 {
            return close(self);
        }
        let mut lost = Vec::new();
        if working > 0 {
            lost.push(format!(
                "{working} agent{} at work stop{}",
                if working == 1 { "" } else { "s" },
                if working == 1 { "s" } else { "" }
            ));
        }
        if unsaved > 0 {
            lost.push(format!(
                "{unsaved} unsaved file{} {} lost",
                if unsaved == 1 { "" } else { "s" },
                if unsaved == 1 { "is" } else { "are" }
            ));
        }
        self.confirm(
            format!("Close {}?", project.name),
            format!("{}.", lost.join(", and ")),
            "Close",
            true,
            move |bench, yes, _, _| {
                if yes {
                    close(bench);
                }
            },
            cx,
        );
    }

    /// Brings forward the project's files: the last file tab in front, else the tab rail's file tree.
    pub fn focus_files(&mut self, cx: &mut Context<Self>) {
        let file = self.strip().and_then(|strip| {
            let tab = self.tabs(&strip).iter().rev().find(|tab| {
                tabs::first_view(&tab.pane).is_some_and(|(kind, _)| kind == PaneKind::Files)
            })?;
            Some((strip.clone(), tab.id.clone()))
        });
        match file {
            Some((strip, tab)) => self.activate_tab(&strip, &tab),
            None => self.open_rail_on(RailPage::Files, cx),
        }
    }

    /// Brings forward the project's first agent, else offers to start one.
    pub fn focus_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(strip @ Strip::Project(_)) = self.strip() else {
            return;
        };
        self.open_rail_on(RailPage::Agents, cx);
        let in_front = self
            .tab()
            .and_then(|tab| tabs::first_view(&tab.pane))
            .is_some_and(|(kind, _)| kind == PaneKind::Agent);
        if in_front {
            return;
        }
        let agent = self
            .tabs(&strip)
            .iter()
            .find(|tab| {
                tabs::first_view(&tab.pane).is_some_and(|(kind, _)| kind == PaneKind::Agent)
            })
            .map(|tab| tab.id.clone());
        match agent {
            Some(tab) => self.activate_tab(&strip, &tab),
            None => crate::keymap::dispatch("agent.choose", window, cx),
        }
    }

    pub fn focus_board(&self, cx: &mut App) {
        if let Some(project) = self.project() {
            self.take_keys_when_shown(crate::workbench::TakesKeys::Board, cx);
            self.call_in_front(
                "openTab",
                json!({ "project": project.id, "kind": "board", "target": project.path.to_string_lossy() }),
            );
        }
    }

    /// Closes the view in front: a desk's page, the focused view of a split tab, else the tab.
    /// It asks first when that would stop an agent mid-turn or lose a file's unsaved changes.
    pub fn close_front(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.close_shown_page(window, cx) {
            return self.parts.redraw(&self.parts.stage, cx);
        }
        let (Some(strip), Some(tab)) = (self.strip(), self.tab().cloned()) else {
            return;
        };
        let mut views = Vec::new();
        views_of(&tab.pane, &mut views);
        let split = views.len() > 1;
        let shown = match split {
            true => views
                .iter()
                .find(|(id, _, _)| self.panes.focused.as_deref() == Some(id.as_str()))
                .or(views.last()),
            false => views.first(),
        };
        let Some((pane, kind, target)) = shown.cloned() else {
            return;
        };
        let tab_id = tab.id.clone();
        let close = move |bench: &mut Workbench| match split {
            true => bench.close_pane(&strip, &tab_id, &pane),
            false => bench.close_tab(&strip, &tab_id),
        };
        let mid_turn = (kind == PaneKind::Agent)
            .then(|| {
                self.view
                    .as_ref()?
                    .chats
                    .iter()
                    .find(|chat| chat.agent_id == target)
                    .filter(|chat| {
                        matches!(chat.status, AgentStatus::Working | AgentStatus::Blocked)
                    })
                    .map(|_| ())
            })
            .flatten();
        let title = self
            .saved_title(&target)
            .unwrap_or_else(|| "the agent".to_owned());
        if mid_turn.is_some() {
            return self.confirm(
                format!("Stop {title}?"),
                "It is in the middle of a turn. Closing stops it.",
                "Stop and close",
                true,
                move |bench, yes, _, _| {
                    if yes {
                        close(bench);
                    }
                },
                cx,
            );
        }
        if kind == PaneKind::Files
            && self
                .editors
                .is_dirty(&crate::editor::FileKey::new(self.place(), &target))
        {
            let name = std::path::Path::new(&target).file_name().map_or_else(
                || target.clone(),
                |name| name.to_string_lossy().into_owned(),
            );
            return self.confirm(
                format!("Close {name} without saving?"),
                "Its unsaved changes are lost.",
                "Close",
                true,
                move |bench, yes, _, _| {
                    if yes {
                        close(bench);
                    }
                },
                cx,
            );
        }
        close(self);
    }

    fn saved_title(&self, agent: &str) -> Option<String> {
        self.view
            .as_ref()?
            .workspace
            .agents
            .iter()
            .find(|saved| saved.id == agent)
            .map(|saved| saved.title.clone())
    }

    /// Opens the tab rail on `page`.
    pub fn open_rail_on(&mut self, page: RailPage, cx: &mut Context<Self>) {
        if !self.tab_rail_open {
            self.tab_rail_open = true;
            self.keep_rails(cx);
        }
        self.show_rail_page(page, cx);
        cx.notify();
    }
}

/// Every view of a tab, by its id, kind and what it shows.
fn views_of(pane: &Pane, views: &mut Vec<(String, PaneKind, String)>) {
    match pane {
        Pane::View { id, kind, target } => views.push((id.clone(), *kind, target.clone())),
        Pane::Split { panes, .. } => panes.iter().for_each(|pane| views_of(pane, views)),
    }
}
