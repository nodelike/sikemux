//! Shortcuts that move between places and the surfaces of the one in front: closing
//! a project or host, going back to the last one, jumping between projects and hosts,
//! and bringing forward a project's files, agents, terminals or board.

use gpui_kit::{App, Context, Window};
use serde_json::json;
use sikemux_wire::protocol::{Pane, PaneKind, ProjectInfo};

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
                return self.close_tabs_asking(strip, tabs, cx);
            }
            Strip::Project(id) => self
                .projects()
                .iter()
                .find(|project| project.id == *id)
                .cloned(),
        };
        if let Some(project) = project {
            self.close_project(project, cx);
        }
    }

    /// Stops offering a project, asking first when agents are at work in it or files are unsaved.
    pub fn close_project(&mut self, project: ProjectInfo, cx: &mut Context<Self>) {
        if self.projects().len() < 2 {
            return;
        }
        let working = self
            .agents_of(&project.id)
            .iter()
            .filter(|agent| agent.busy)
            .count();
        let unsaved = self
            .editors
            .read(cx)
            .unsaved_under(&self.place(), &project.path)
            .count();
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
            return self.parts.redraw_stage(cx);
        }
        let (Some(strip), Some(tab)) = (self.strip(), self.tab().cloned()) else {
            return;
        };
        let views: Vec<_> = crate::panes::views(&tab.pane).collect();
        let split = views.len() > 1;
        let shown = match split {
            true => views
                .iter()
                .find(|view| self.panes.focused.as_deref() == Some(view.id.as_str()))
                .or(views.last()),
            false => views.first(),
        };
        let Some((pane, kind, target)) =
            shown.map(|view| (view.id.clone(), view.kind, view.target.clone()))
        else {
            return;
        };
        let place = self.place();
        let closing = match split {
            true => crate::editor::files_of(
                &place,
                &Pane::View {
                    id: pane.clone(),
                    kind,
                    target: target.clone(),
                },
            ),
            false => crate::editor::files_of(&place, &tab.pane),
        };
        let tab_id = tab.id.clone();
        let close = move |bench: &mut Workbench| match split {
            true => bench.close_pane(&strip, &tab_id, &pane),
            false => bench.close_tabs(&strip, std::slice::from_ref(&tab_id)),
        };
        if kind == PaneKind::Agent {
            let title = format!(
                "Close {}?",
                self.saved_title(&target)
                    .unwrap_or_else(|| "the agent".to_owned())
            );
            return self.ask_to_stop(vec![target], title, cx, move |bench, workers| {
                close(bench);
                if !workers.is_empty() {
                    bench.call("closeAgents", json!({ "agentIds": workers }));
                }
            });
        }
        crate::editor::close_unsaved(self, closing, move |bench, _| close(bench), cx);
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
        if !self.rails.tab_open {
            self.rails.tab_open = true;
            self.keep_rails(cx);
        }
        self.show_rail_page(page, cx);
        cx.notify();
    }
}
