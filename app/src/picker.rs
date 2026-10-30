//! ⌘O, ⌘⇧S and ⌘⇧O: the projects with the folders under the person's project
//! roots, the SSH hosts, or both.

use std::path::Path;

use gpui_kit::{Context, PathPromptOptions, Window};
use serde_json::json;
use sikemux_core::server::SshHost;

use crate::core::Ask;
use crate::icons::Icon;
use crate::palette::{Mark, Row, Spec};
use crate::ui::path::{this_home, tilde};
use crate::workbench::{Strip, Workbench};

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "project.open")]
pub struct OpenProject;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "ssh.open")]
pub struct OpenHost;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "session.open")]
pub struct OpenPlace;

const PROJECT: &str = "project:";
const FOLDER: &str = "folder:";
const HOST: &str = "host:";

/// What happens once a place is picked, past bringing it forward.
#[derive(Clone, Copy, PartialEq)]
pub enum ThenPicked {
    Open,
    StartChat,
    /// Moves the agent `Workbench::moving` names into the project picked.
    MoveAgent,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Picking {
    Projects,
    Hosts,
    Both,
}

impl Picking {
    const ALL: [Picking; 3] = [Picking::Projects, Picking::Hosts, Picking::Both];

    fn id(self) -> &'static str {
        match self {
            Picking::Projects => "project.open",
            Picking::Hosts => "ssh.open",
            Picking::Both => "session.open",
        }
    }

    fn projects(self) -> bool {
        self != Picking::Hosts
    }

    fn hosts(self) -> bool {
        self != Picking::Projects
    }
}

fn host_line(host: &SshHost) -> String {
    let user = host
        .user
        .as_deref()
        .map(|user| format!("{user}@"))
        .unwrap_or_default();
    let port = host
        .port
        .filter(|port| *port != 22)
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let name = host.hostname.as_deref().unwrap_or(&host.alias);
    format!("{user}{name}{port}")
}

impl Workbench {
    pub fn toggle_picker(&mut self, picking: Picking, window: &mut Window, cx: &mut Context<Self>) {
        self.moving = None;
        self.open_picker(picking, ThenPicked::Open, window, cx);
    }

    /// The project picker, doing `then` with the project picked.
    pub fn pick_project(&mut self, then: ThenPicked, window: &mut Window, cx: &mut Context<Self>) {
        self.moving = None;
        self.open_picker(Picking::Projects, then, window, cx);
    }

    /// The project picker, moving `agent` into the project or folder picked, as the Mac's
    /// Move to Project… does; its own project is left out.
    pub fn pick_project_for(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.moving = Some(agent.to_owned());
        self.open_picker(Picking::Projects, ThenPicked::MoveAgent, window, cx);
    }

    /// The agent the picker moves, and the folder it is in now.
    fn moving_from(&self) -> Option<(&str, &Path)> {
        let agent = self.moving.as_deref()?;
        let held = self
            .view
            .as_ref()?
            .workspace
            .agents
            .iter()
            .find(|held| held.id == agent)?;
        Some((agent, held.cwd.as_path()))
    }

    fn open_picker(
        &mut self,
        picking: Picking,
        then: ThenPicked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if picking.projects() {
            let roots = self.preferences().project_roots.clone();
            if !roots.is_empty() {
                let _ = self.asks.send(Ask::Answer {
                    key: "pickerFolders",
                    method: "projectRoots",
                    params: json!({ "roots": roots }),
                });
            }
        }
        if picking.hosts() {
            let _ = self.asks.send(Ask::Answer {
                key: "sshHosts",
                method: "sshHosts",
                params: json!({}),
            });
        }
        let moved = self.moving.as_deref().and_then(|agent| {
            let held = self
                .view
                .as_ref()?
                .workspace
                .agents
                .iter()
                .find(|held| held.id == agent)?;
            Some(match &held.name {
                Some(name) => format!("@{name}"),
                None => held.title.clone(),
            })
        });
        let placeholder = match (picking, then) {
            (_, ThenPicked::MoveAgent) => {
                format!("move {} to…", moved.unwrap_or_default())
            }
            (_, ThenPicked::StartChat) => "start a chat in a project…".to_owned(),
            (Picking::Projects, _) => "open a project…".to_owned(),
            (Picking::Hosts, _) => "connect to a host…".to_owned(),
            (Picking::Both, _) => "open a project or host…".to_owned(),
        };
        let spec = Spec::new(
            picking.id(),
            placeholder,
            self.places(picking),
            move |row, bench, window, cx| {
                bench.open_place(&row.key, then, window, cx);
            },
        )
        .empty(if picking == Picking::Hosts {
            "no hosts in ~/.ssh/config match"
        } else {
            "no projects match"
        });
        let spec = match picking.projects() {
            true => spec.aside(Icon::Folder, "Open folder", move |bench, window, cx| {
                bench.close_palette(window, cx);
                let picked = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: None,
                });
                cx.spawn_in(window, async move |bench, cx| {
                    let Ok(Ok(Some(paths))) = picked.await else {
                        return;
                    };
                    let Some(path) = paths.first() else {
                        return;
                    };
                    let key = format!("{FOLDER}{}", path.to_string_lossy());
                    let _ = bench.update_in(cx, |bench, window, cx| {
                        bench.open_place(&key, then, window, cx)
                    });
                })
                .detach();
            }),
            false => spec,
        };
        self.toggle_palette(spec, window, cx);
    }

    /// Lists what the core answered since the picker opened.
    pub fn picker_answered(&mut self, cx: &mut Context<Self>) {
        let Some(picking) = Picking::ALL.into_iter().find(|picking| {
            self.overlays
                .palette
                .as_ref()
                .is_some_and(|open| open.id() == picking.id())
        }) else {
            return;
        };
        let rows = self.places(picking);
        if let Some(palette) = self.overlays.palette.as_mut() {
            palette.refill(rows, cx);
        }
        self.parts.redraw(&self.parts.finder, cx);
    }

    fn places(&self, picking: Picking) -> Vec<Row> {
        let mut rows = Vec::new();
        if picking.projects() {
            let projects = self.projects();
            let home = self.home_at(&self.place());
            let from = self.moving_from().map(|(_, from)| from);
            for project in projects
                .iter()
                .filter(|project| Some(project.path.as_path()) != from)
            {
                let path = tilde(&project.path.to_string_lossy(), home.as_deref());
                let folder = project
                    .path
                    .parent()
                    .map(|folder| tilde(&folder.to_string_lossy(), home.as_deref()))
                    .unwrap_or_default();
                rows.push(
                    Row::new(format!("{PROJECT}{}", project.id), project.name.clone())
                        .path(folder)
                        .mark(Mark::Place(Icon::Folder))
                        .group("Projects")
                        .also(&path),
                );
            }
            let added = |path: &str| {
                projects
                    .iter()
                    .any(|project| project.path == Path::new(path))
            };
            for folder in &self.reads.picker_folders {
                let (name, path) = (folder.name.as_str(), folder.path.as_str());
                if added(path) {
                    continue;
                }
                let shown = tilde(path, this_home().as_deref());
                let folder = shown
                    .strip_suffix(name)
                    .map(|folder| folder.trim_end_matches('/'))
                    .unwrap_or(&shown)
                    .to_owned();
                rows.push(
                    Row::new(format!("{FOLDER}{path}"), name.to_owned())
                        .path(folder)
                        .mark(Mark::Place(Icon::FolderPlus))
                        .group("Folders")
                        .also(&shown),
                );
            }
        }
        if picking.hosts() {
            for host in &self.reads.ssh_hosts {
                let alias = host.alias.as_str();
                let line = host_line(host);
                rows.push(
                    Row::new(format!("{HOST}{alias}"), alias.to_owned())
                        .meta(line.clone())
                        .mark(Mark::Icon(Icon::Server))
                        .group("SSH hosts")
                        .also(&line),
                );
            }
        }
        rows
    }

    pub(crate) fn open_place(
        &mut self,
        key: &str,
        then: ThenPicked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if then == ThenPicked::MoveAgent {
            let Some(agent) = self.moving.take() else {
                return;
            };
            if let Some(project) = key.strip_prefix(PROJECT) {
                self.call(
                    "moveAgentToProject",
                    json!({ "agentId": agent, "project": project }),
                );
            } else if let Some(path) = key.strip_prefix(FOLDER) {
                self.call(
                    "moveAgentToFolder",
                    json!({ "agentId": agent, "path": path }),
                );
            }
            return;
        }
        if let Some(project) = key.strip_prefix(PROJECT) {
            self.activate_project(project);
            if then == ThenPicked::StartChat {
                self.start_chat_in(project, window, cx);
            }
        } else if let Some(path) = key.strip_prefix(FOLDER) {
            let added = crate::core::request(&self.asks, "addProject", json!({ "path": path }));
            cx.spawn_in(window, async move |bench, cx| {
                let project = match added.await {
                    Ok(project) => project["id"].as_str().unwrap_or_default().to_owned(),
                    Err(error) => {
                        return eprintln!("sikemux: the core would not add the project: {error}")
                    }
                };
                let _ = bench.update_in(cx, |bench, window, cx| {
                    bench.activate_project(&project);
                    if then == ThenPicked::StartChat {
                        bench.start_chat_in(&project, window, cx);
                    }
                });
            })
            .detach();
        } else if let Some(alias) = key.strip_prefix(HOST) {
            self.show_strip(&Strip::Host(alias.to_owned()), cx);
        }
    }
}
