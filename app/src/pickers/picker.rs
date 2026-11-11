//! ⌘O, ⌘⇧S and ⌘⇧O: the projects with the folders under the person's project
//! roots, the SSH hosts, or both.

use sikemux_core::server::calls::NoParams;
use sikemux_core::server::project_roots::ScanParams;
use std::path::Path;

use gpui_kit::{Context, PathPromptOptions, Window};
use sikemux_core::server::ssh::SshHost;

use crate::core::Target;
use crate::icons::Icon;
use crate::palette::{Mark, Row, Spec};
use crate::reads::Read;
use crate::toast;
use crate::ui::path::{this_home, tilde};
use crate::workbench::{Strip, Workbench};
use sikemux_core::server::call;
use sikemux_core::server::move_agent::{MoveParams, MoveToFolder};
use sikemux_core::server::workspace::AddProjectParams;

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

/// The folder a typed absolute or `~` path names, to open as a project.
fn folder_typed(query: &str) -> Option<Row> {
    let home = this_home();
    let path = match query.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", home.as_deref()?)
        }
        Some(_) => return None,
        None if query.starts_with('/') => query.to_owned(),
        None => return None,
    };
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    let folder = Path::new(path).is_dir();
    let shown = tilde(path, home.as_deref());
    Some(
        Row::new(format!("{FOLDER}{path}"), shown)
            .meta(if folder {
                "open as project"
            } else {
                "no folder here"
            })
            .mark(Mark::Place(Icon::FolderPlus))
            .disabled(!folder),
    )
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

    /// The host whose projects the picker lists and acts on: the moving chat's own, else the
    /// host in front.
    fn picking_place(&self) -> crate::hosts::Place {
        match self.moving.as_deref() {
            Some(agent) => self.chat_target(agent).place().clone(),
            None => self.place(),
        }
    }

    /// The agent the picker moves, and the folder it is in now.
    fn moving_from(&self) -> Option<(&str, &Path)> {
        let agent = self.moving.as_deref()?;
        let held = self
            .view_at(&self.picking_place())?
            .workspace
            .agents
            .iter()
            .find(|held| held.id == agent)?;
        Some((agent, held.cwd.as_path()))
    }

    /// Asks again what lies under the project roots, for either project picker to offer.
    pub(crate) fn read_project_folders(&self) {
        let roots = &self.preferences().project_roots;
        if !roots.is_empty() {
            Target::here(&self.asks).answer::<call::ProjectRoots>(
                Read::PickerFolders,
                ScanParams {
                    roots: roots.clone(),
                },
            );
        }
    }

    /// Makes the folder at `path` a project, answering its id, or tells the person why not.
    pub(crate) fn add_project(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Task<Option<String>> {
        let added = Target::here(&self.asks).request_typed::<call::AddProject>(AddProjectParams {
            path: path.into(),
            name: None,
            id: None,
        });
        cx.spawn(async move |bench, cx| match added.await {
            Ok(project) => Some(project.id),
            Err(error) => {
                let note = toast::error(format!("The folder did not open as a project: {error}"));
                let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
                None
            }
        })
    }

    fn open_picker(
        &mut self,
        picking: Picking,
        then: ThenPicked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if picking.projects() {
            self.read_project_folders();
        }
        if picking.hosts() {
            Target::here(&self.asks).answer::<call::SshHosts>(Read::SshHosts, NoParams {});
        }
        let moved = self.moving.as_deref().and_then(|agent| {
            let held = self
                .view_at(&self.picking_place())?
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
            (_, ThenPicked::StartChat) => "start a chat in a project or type a path…".to_owned(),
            (Picking::Projects, _) => "open a project or type a path…".to_owned(),
            (Picking::Hosts, _) => "connect to a host…".to_owned(),
            (Picking::Both, _) => "open a project or host, or type a path…".to_owned(),
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
            true => spec.typed(folder_typed).aside(
                Icon::Folder,
                "Open folder",
                move |bench, window, cx| {
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
                },
            ),
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
            let place = self.picking_place();
            let projects =
                (self.view_at(&place)).map_or(&[][..], |view| view.workspace.projects.as_slice());
            let home = self.home_at(&place);
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
            let found =
                (self.reads.picker_folders.iter()).filter(|_| place == crate::hosts::Place::This);
            for folder in found {
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
            let host = self.chat_target(&agent);
            if let Some(project) = key.strip_prefix(PROJECT) {
                host.call_typed::<call::MoveAgentToProject>(MoveParams {
                    agent_id: agent,
                    project: project.to_owned(),
                });
            } else if let Some(path) = key.strip_prefix(FOLDER) {
                host.call_typed::<call::MoveAgentToFolder>(MoveToFolder {
                    agent_id: agent,
                    path: path.into(),
                });
            }
            return;
        }
        if let Some(project) = key.strip_prefix(PROJECT) {
            self.activate_project(project);
            if then == ThenPicked::StartChat {
                self.start_chat_in(project, window, cx);
            }
        } else if let Some(path) = key.strip_prefix(FOLDER) {
            let added = self.add_project(path, cx);
            cx.spawn_in(window, async move |bench, cx| {
                let Some(project) = added.await else {
                    return;
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
