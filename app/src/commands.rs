//! ⌘⇧P, the command deck: every command the window can do and the person's own
//! commands, the ones used last first.

use crate::i18n::tr;
use crate::workbench::actions;
use gpui_kit::{App, Context, SharedString, Window};
use sikemux_calls::call;
use sikemux_calls::tasks::ConfigStatus;
use sikemux_calls::tasks::ProjectConfigRead;
use sikemux_calls::tasks::ProjectParams;
use sikemux_corekit::tasks::config::ConfigLoad;
use sikemux_corekit::tasks::config::Stated;
use sikemux_view::layout::PaneKind;
use tokio::sync::oneshot;

use sikemux_agents::git::worktree::GitWorktree;
use sikemux_calls::git::call as git_call;

use crate::core;
use crate::keymap;
use crate::palette::{Row, Spec};
use crate::toast;
use crate::workbench::{Strip, Workbench};
use sikemux_calls::custom_command::RunCustomParams;

const REMEMBERED: usize = 20;

macro_rules! picks {
    ($($kind:ident = $prefix:literal,)*) => {
        /// What a row of the deck does; its string form is how the last used are remembered.
        #[derive(Clone, Copy)]
        enum Pick<'a> {
            $($kind(&'a str),)*
        }

        impl<'a> Pick<'a> {
            fn key(self) -> SharedString {
                match self {
                    $(Pick::$kind(id) => format!("{}{id}", $prefix).into(),)*
                }
            }

            fn read(key: &'a str) -> Option<Self> {
                $(if let Some(id) = key.strip_prefix($prefix) {
                    return Some(Pick::$kind(id));
                })*
                None
            }
        }
    };
}

picks! {
    Builtin = "builtin:",
    Custom = "custom:",
    Standalone = "standalone:",
    OpenWorktree = "worktree.open:",
    RemoveWorktree = "worktree.remove:",
    RunTask = "task.run:",
    RestartTask = "task.restart:",
    StopTask = "task.stop:",
    StartPreview = "project.preview.start:",
    OpenPreview = "project.preview.open:",
    ConfigInvalid = "project.config.invalid",
    Action = "project.action:",
}

/// A command no shortcut does: what it is called, when it is offered and what it does.
struct Standalone {
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    category: &'static str,
    offered: fn(&Workbench) -> bool,
    run: fn(&mut Workbench, &mut Window, &mut Context<Workbench>),
}

fn language_servers_allowed(bench: &Workbench) -> Option<bool> {
    let project = bench.project()?;
    let path = project.path.to_string_lossy();
    Some(crate::editor::language_servers_allowed(bench, &path))
}

fn allow_language_servers(bench: &mut Workbench, allow: bool) {
    if let Some(project) = bench.project() {
        let path = project.path.to_string_lossy().into_owned();
        crate::editor::allow_language_servers(bench, path, allow);
    }
}

const STANDALONE: [Standalone; 8] = [
    Standalone {
        id: "project.language-servers.stop",
        title: n!("Stop language servers for this project"),
        detail: n!("Stop them now and do not start them again"),
        category: n!("Project · Language servers"),
        offered: |bench| language_servers_allowed(bench) == Some(true),
        run: |bench, _, _| allow_language_servers(bench, false),
    },
    Standalone {
        id: "project.language-servers.allow",
        title: n!("Allow language servers for this project"),
        detail: n!("Start them when you open a file here"),
        category: n!("Project · Language servers"),
        offered: |bench| language_servers_allowed(bench) == Some(false),
        run: |bench, _, _| allow_language_servers(bench, true),
    },
    Standalone {
        id: "support.diagnostics",
        title: n!("Open runtime diagnostics"),
        detail: n!("Inspect redacted runtime and agent-detection health"),
        category: n!("Support"),
        offered: |_| true,
        run: |bench, _, cx| crate::screens::experience::open_diagnostics(bench, cx),
    },
    Standalone {
        id: "support.whats-new",
        title: n!("Open What’s New"),
        detail: n!("Review the latest Sikemux release notes"),
        category: n!("Support"),
        offered: |_| true,
        run: |bench, _, cx| crate::screens::experience::open_whats_new(bench, cx),
    },
    Standalone {
        id: "support.onboarding",
        title: n!("Show welcome"),
        detail: n!("Open the first-run screen"),
        category: n!("Support"),
        offered: |_| true,
        run: |bench, _, cx| crate::screens::experience::open_welcome(bench, cx),
    },
    Standalone {
        id: "window.duplicate",
        title: n!("Duplicate active terminal"),
        detail: n!("Clone the active window into a new terminal tab"),
        category: n!("Window"),
        offered: |bench| bench.terminal_in_front().is_some(),
        run: |bench, _, _| {
            if let Some((strip, tab)) = bench.terminal_in_front() {
                bench.duplicate_tab(&strip, &tab);
            }
        },
    },
    Standalone {
        id: "app.quit-and-stop-everything",
        title: n!("Quit and Stop Everything"),
        detail: n!("Stop every terminal, terminal agent and process, then quit. Plain Quit leaves them running."),
        category: n!("Application"),
        offered: |_| true,
        run: |_, window, cx| keymap::dispatch(actions::QuitAndStopEverything::ID, window, cx),
    },
    Standalone {
        id: "agents.reload-manifests",
        title: n!("Reload agent manifests"),
        detail: n!("Reload agent-state detection rules from disk"),
        category: n!("Agents"),
        offered: |_| true,
        run: |bench, _, cx| bench.reload_manifests(cx),
    },
];

/// The deck's rows for the preview the project's `sikemux.json` checks in: start its command,
/// and open its address.
fn preview_rows(config: &ProjectConfigRead, project: &str) -> Vec<Row> {
    let ConfigLoad::Valid { config, .. } = &config.load else {
        return Vec::new();
    };
    let Some(preview) = config.preview.as_ref().and_then(Stated::given) else {
        return Vec::new();
    };
    let url = preview.url.as_deref();
    let mut rows = Vec::new();
    if preview
        .command
        .as_deref()
        .is_some_and(|command| !command.is_empty())
    {
        let detail = url.map_or_else(
            || t!("Run the checked-in preview command").to_owned(),
            |url| t!("Serve {url}", url = url),
        );
        rows.push(
            Row::new(
                Pick::StartPreview(project).key(),
                t!("Start project preview"),
            )
            .meta(t!("Project · Preview"))
            .also(&detail)
            .also("Project · Preview"),
        );
    }
    if let Some(url) = url {
        rows.push(
            Row::new(Pick::OpenPreview(url).key(), t!("Open project preview"))
                .meta(t!("Project · Preview"))
                .also(url)
                .also("Project · Preview"),
        );
    }
    rows
}

/// The project's sikemux.json actions that give themselves a shortcut, as `(id, label, keys)`.
pub(crate) fn action_keys(config: &ProjectConfigRead) -> Vec<(String, String, String)> {
    let ConfigLoad::Valid { config, .. } = &config.load else {
        return Vec::new();
    };
    (config.actions.iter())
        .filter_map(|action| {
            let keys = action.keybinding.clone()?;
            Some((action.id.clone(), action.label.clone(), keys))
        })
        .collect()
}

/// The deck's rows for the project's `sikemux.json` actions, each run once the person trusts
/// the file.
fn action_rows(config: &ProjectConfigRead, cx: &App) -> Vec<Row> {
    let ConfigLoad::Valid { config, path, .. } = &config.load else {
        return Vec::new();
    };
    (config.actions.iter())
        .map(|action| {
            let id = action.id.as_str();
            let detail = Some(action.description.as_str())
                .filter(|detail| !detail.is_empty())
                .map_or_else(|| t!("Run from {path}", path = path), str::to_owned);
            Row::new(Pick::Action(id).key(), &action.label)
                .keys(keymap::action_hint(id, cx))
                .meta(t!("Project · sikemux.json"))
                .also(&detail)
                .also("Project · sikemux.json")
        })
        .collect()
}

/// The deck's rows for each worktree of the project's repository: open it as a project, and
/// remove it unless it is the main checkout or the one in front.
fn worktree_rows(worktrees: &Vec<GitWorktree>) -> Vec<Row> {
    let mut rows = Vec::new();
    for worktree in worktrees {
        let path = worktree.path.as_str();
        if worktree.bare {
            continue;
        }
        let current = worktree.current;
        let branch = worktree.branch.as_deref().unwrap_or(t!("detached"));
        let title = if current {
            t!("Open current worktree").to_owned()
        } else {
            t!("Open worktree: {branch}", branch = branch)
        };
        rows.push(
            Row::new(Pick::OpenWorktree(path).key(), title)
                .meta(t!("worktrees"))
                .also(path),
        );
        if !current && !worktree.is_main {
            rows.push(
                Row::new(
                    Pick::RemoveWorktree(path).key(),
                    t!("Remove worktree: {branch}", branch = branch),
                )
                .meta(t!("worktrees"))
                .also(path)
                .also(branch),
            );
        }
    }
    rows
}

/// What `run_command` runs: a custom command or a `sikemux.json` action, by id.
pub(crate) enum Which {
    Command(String),
    Action(String),
}

impl Workbench {
    /// What is wrong with the project's `sikemux.json`, each as where in the file and why.
    fn config_errors(&self) -> Vec<String> {
        (self.feeds.in_front().tasks.as_ref())
            .filter(|read| read.config_status == ConfigStatus::Invalid)
            .and_then(|read| read.config_errors.as_ref())
            .into_iter()
            .flatten()
            .map(|error| format!("{} {}", error.path, error.message))
            .collect()
    }

    /// The project's tasks to run, and the one running to restart or stop.
    fn task_rows(&self) -> Vec<Row> {
        let Some(read) = self.feeds.in_front().tasks.as_ref() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        if read.config_status == ConfigStatus::Invalid {
            let first = (read.config_errors.as_ref())
                .and_then(|errors| errors.first())
                .map_or(t!("sikemux.json is invalid"), |error| {
                    error.message.as_str()
                });
            rows.push(
                Row::new(
                    Pick::ConfigInvalid("").key(),
                    t!("Project config needs attention"),
                )
                .meta(t!("Project · sikemux.json"))
                .also(first)
                .also("Project · sikemux.json"),
            );
        }
        let tasks = &read.tasks;
        rows.extend(tasks.iter().map(|task| {
            Row::new(Pick::RunTask(&task.id).key(), task.label.clone())
                .meta(t!("Processes"))
                .also(&t!("Run process from {cwd}", cwd = task.cwd))
                .also("Processes")
        }));
        let Some(running) = self.person_runs().0 else {
            return rows;
        };
        let task = &running.run.task_id;
        if tasks.iter().any(|listed| listed.id == *task) {
            rows.push(
                Row::new(Pick::RestartTask(task).key(), t!("Restart active process"))
                    .meta(t!("Processes"))
                    .also(
                        "Stop the current project process and start its current trusted definition",
                    ),
            );
        }
        rows.push(
            Row::new(
                Pick::StopTask(&running.run.execution_id).key(),
                t!("Stop active process"),
            )
            .meta(t!("Processes"))
            .also("Stop only the exact terminal owned by the current project process"),
        );
        rows
    }

    pub fn toggle_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut rows: Vec<Row> = keymap::shortcuts()
            .iter()
            .filter(|shortcut| cx.build_action(shortcut.id, None).is_ok())
            .map(|shortcut| {
                Row::new(Pick::Builtin(shortcut.id).key(), tr(shortcut.label))
                    .meta(tr(shortcut.category))
                    .keys(keymap::hint_named(shortcut.id, cx))
                    .also(tr(shortcut.detail))
                    .also(shortcut.label)
                    .also(shortcut.detail)
                    .also(shortcut.category)
                    .also(shortcut.id)
            })
            .collect();
        rows.extend(
            STANDALONE
                .iter()
                .filter(|command| (command.offered)(self))
                .map(|command| {
                    Row::new(Pick::Standalone(command.id).key(), tr(command.title))
                        .meta(tr(command.category))
                        .also(tr(command.detail))
                        .also(command.title)
                        .also(command.detail)
                        .also(command.category)
                }),
        );
        rows.extend(self.task_rows());
        if let Some(strip) = self.strip() {
            let kind = match strip {
                Strip::Project(_) => "project",
                Strip::Host(_) => "ssh",
                Strip::Terminals => "command",
            };
            let custom = self
                .preferences()
                .custom_commands
                .clone()
                .into_iter()
                .filter(|command| {
                    command.contexts.is_empty() || command.contexts.iter().any(|at| at == kind)
                });
            rows.extend(custom.map(|command| {
                Row::new(Pick::Custom(&command.id).key(), command.title)
                    .meta(t!("custom"))
                    .also(&command.detail)
                    .also(&command.command)
            }));
        }
        let recent = |row: &Row| {
            self.recent_commands
                .iter()
                .position(|key| *key == row.key)
                .unwrap_or(usize::MAX)
        };
        rows.sort_by_key(recent);
        let base = rows.clone();
        let spec = Spec::new(
            "palette.commands",
            t!("type a command…"),
            rows,
            |row, bench, window, cx| {
                bench.remember_command(row.key.clone(), cx);
                match Pick::read(&row.key) {
                    Some(Pick::Builtin(id)) => keymap::dispatch_named(id, window, cx),
                    Some(Pick::Custom(id)) => bench.run_custom_command(id, cx),
                    Some(Pick::Action(id)) => {
                        bench.run_command(Which::Action(id.to_string()), row.title.to_string(), cx)
                    }
                    Some(Pick::Standalone(id)) => {
                        if let Some(command) = STANDALONE.iter().find(|command| command.id == id) {
                            (command.run)(bench, window, cx);
                        }
                    }
                    Some(Pick::OpenWorktree(path)) => bench.open_place(
                        &format!("folder:{path}"),
                        crate::pickers::picker::ThenPicked::Open,
                        window,
                        cx,
                    ),
                    Some(Pick::RunTask(task) | Pick::RestartTask(task)) => {
                        bench.start_task(task, cx)
                    }
                    Some(Pick::StopTask(execution)) => bench.stop_run(execution),
                    Some(Pick::StartPreview(project)) => bench.start_preview(project, cx),
                    Some(Pick::OpenPreview(url)) => cx.open_url(url),
                    Some(Pick::ConfigInvalid(_)) => {
                        let errors = bench.config_errors().join(" · ");
                        bench.notify(
                            toast::error(format!("sikemux.json: {errors}")).autohide(false),
                            cx,
                        );
                    }
                    Some(Pick::RemoveWorktree(path)) => {
                        let branch = row.search.rsplit(' ').next().unwrap_or_default();
                        bench.remove_worktree_at(path.to_owned(), branch.to_owned(), cx)
                    }
                    None => {}
                }
            },
        )
        .empty(t!("no commands match"))
        .deck();
        self.toggle_palette(spec, window, cx);
        self.list_more(base, cx);
    }

    /// Adds the project's preview and its worktrees to the open deck once the core and git
    /// say what they are.
    fn list_more(&self, base: Vec<Row>, cx: &mut Context<Self>) {
        let (Some(Strip::Project(_)), Some(project)) = (self.strip(), self.project()) else {
            return;
        };
        let (id, folder) = (
            project.id.clone(),
            project.path.to_string_lossy().into_owned(),
        );
        let target = self.target();
        let config = target.request_typed::<call::ProjectConfig>(ProjectParams { project: folder });
        let listed = self
            .git_repo()
            .map(|repo| target.request_typed::<git_call::Worktrees>(git_call::Worktrees { repo }));
        cx.spawn(async move |this, cx| {
            let config = config.await.ok();
            let worktrees = match listed {
                Some(listed) => listed.await.ok(),
                None => None,
            };
            let _ = this.update(cx, |bench, cx| {
                let mut rows = base;
                rows.extend(config.iter().flat_map(|config| preview_rows(config, &id)));
                rows.extend(config.iter().flat_map(|config| action_rows(config, cx)));
                rows.extend(worktrees.iter().flat_map(worktree_rows));
                if let Some(palette) = bench
                    .overlays
                    .palette
                    .as_mut()
                    .filter(|open| open.id() == "palette.commands")
                {
                    palette.refill(rows, cx);
                }
                bench.parts.redraw(&bench.parts.finder, cx);
            });
        })
        .detach();
    }

    /// Removes the worktree at `path` once the person agrees; git refuses a dirty one, and
    /// the core one a project or agent still works in.
    fn remove_worktree_at(&mut self, path: String, branch: String, cx: &mut Context<Self>) {
        let Some(repo) = self.git_repo() else {
            return;
        };
        self.confirm(
            t!("Remove worktree {branch}?", branch = branch),
            t!("Dirty worktrees will be refused."),
            t!("Remove"),
            true,
            move |bench, yes, _, cx| {
                if !yes {
                    return;
                }
                let removed = bench.target().request_typed::<git_call::WorktreeRemove>(
                    git_call::WorktreeRemove {
                        repo: repo.clone(),
                        path: path.clone(),
                        force: false,
                    },
                );
                let branch = branch.clone();
                bench.report(
                    removed,
                    t!("Could not remove the worktree"),
                    move |bench, _, cx| {
                        bench.notify(
                            toast::success(t!("Removed worktree {branch}", branch = branch)),
                            cx,
                        )
                    },
                    cx,
                );
            },
            cx,
        );
    }

    /// The tab in front, when it is a project's terminal.
    fn terminal_in_front(&self) -> Option<(Strip, String)> {
        let strip @ Strip::Project(_) = self.strip()? else {
            return None;
        };
        let tab = self.tab()?;
        let (kind, _) = crate::tabs::first_view(&tab.pane)?;
        (kind == PaneKind::Terminal).then(|| (strip, tab.id.clone()))
    }

    fn reload_manifests(&self, cx: &mut Context<Self>) {
        let (reloaded, reload) = oneshot::channel();
        let _ = self.asks.send(core::Ask::Detection {
            detection: core::Detection::Reload,
            reply: reloaded,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Err(error)) = reload.await {
                let _ = this.update(cx, |bench, cx| {
                    bench.notify(
                        toast::error(t!("Agent manifests: {error}", error = error)).autohide(false),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    fn remember_command(&mut self, key: SharedString, cx: &mut Context<Self>) {
        self.recent_commands.retain(|known| *known != key);
        self.recent_commands.insert(0, key);
        self.recent_commands.truncate(REMEMBERED);
        self.keep_rails(cx);
    }

    /// Starts the project's checked-in preview in a terminal of its own; the core asks the
    /// person to trust `sikemux.json` first when they have not.
    fn start_preview(&self, project: &str, cx: &mut Context<Self>) {
        let started = self
            .target()
            .request_typed::<call::StartPreview>(ProjectParams {
                project: project.to_owned(),
            });
        self.report(
            started,
            t!("The project preview did not start"),
            |_, _, _| {},
            cx,
        );
    }

    /// Runs one of the person's commands for the strip in front, telling them
    /// what one in the background printed.
    fn run_custom_command(&self, id: &str, cx: &mut Context<Self>) {
        let title = self
            .preferences()
            .custom_commands
            .clone()
            .into_iter()
            .find(|command| command.id == id)
            .map_or_else(|| id.to_owned(), |command| command.title);
        self.run_command(Which::Command(id.to_owned()), title, cx);
    }

    /// Runs `which`, a custom command's id or a `sikemux.json` action's, for the strip in
    /// front, telling the person what one in the background printed.
    pub(crate) fn run_command(&self, which: Which, title: String, cx: &mut Context<Self>) {
        let Some(strip) = self.strip() else {
            return;
        };
        if matches!(which, Which::Action(_)) && !matches!(strip, Strip::Project(_)) {
            return;
        }
        let (id, action) = match which {
            Which::Command(id) => (Some(id), None),
            Which::Action(id) => (None, Some(id)),
        };
        let ran = self
            .target()
            .request_typed::<call::RunCustomCommand>(RunCustomParams {
                at: strip.at(),
                id,
                action,
                tab: self.tab().map(|tab| tab.id.clone()),
                pane: self.focused_view(),
            });
        cx.spawn(async move |this, cx| {
            let ran = ran.await;
            let _ = this.update(cx, |bench, cx| match ran {
                Ok(ran) => {
                    if let Some(popup) = ran.popup {
                        return bench.show_command_popup(title, popup, cx);
                    }
                    let Some(result) = ran.result else {
                        return;
                    };
                    let output = result.output.trim();
                    let code = result.code;
                    let said = if output.is_empty() {
                        t!("exit {code}", code = code)
                    } else {
                        output.to_owned()
                    };
                    let said = t!("{title}: {said}", title = title, said = said);
                    let note = if code == 0 {
                        toast::success(said)
                    } else {
                        toast::error(said).autohide(false)
                    };
                    bench.notify(note, cx);
                }
                Err(error) => bench.notify(
                    toast::error(t!("{title}: {error}", title = title, error = error))
                        .autohide(false),
                    cx,
                ),
            });
        })
        .detach();
    }
}
