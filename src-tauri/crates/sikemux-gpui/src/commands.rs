//! ⌘⇧P, the command deck: every command the window can do and the person's own
//! commands, the ones used last first.

use gpui_kit::{App, Context, SharedString, Window};
use serde_json::{json, Value};
use sikemux_wire::protocol::{PaneKind, SessionId};
use tokio::sync::oneshot;

use crate::core;
use crate::keymap;
use crate::palette::{Row, Spec};
use crate::toast;
use crate::workbench::{Strip, Workbench};

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "palette.commands")]
pub struct OpenCommands;

const REMEMBERED: usize = 20;
const BUILTIN: &str = "builtin:";
const CUSTOM: &str = "custom:";
const STANDALONE: &str = "standalone:";
const OPEN_WORKTREE: &str = "worktree.open:";
const REMOVE_WORKTREE: &str = "worktree.remove:";
const RUN_TASK: &str = "task.run:";
const RESTART_TASK: &str = "task.restart:";
const STOP_TASK: &str = "task.stop:";
const START_PREVIEW: &str = "project.preview.start:";
const OPEN_PREVIEW: &str = "project.preview.open:";
const CONFIG_INVALID: &str = "project.config.invalid";
const ACTION: &str = "project.action:";
const PLUGIN_PALETTE: &str = "plugin.palette:";

/// A command no shortcut does: its id, name, what it does and its category.
type Standalone = (&'static str, &'static str, &'static str, &'static str);

/// The deck's rows for the preview the project's `sikemux.json` checks in: start its command,
/// and open its address.
fn preview_rows(config: &Value, project: &str) -> Vec<Row> {
    if config["status"] != "valid" {
        return Vec::new();
    }
    let preview = &config["config"]["preview"];
    let url = preview["url"].as_str();
    let mut rows = Vec::new();
    if preview["command"]
        .as_str()
        .is_some_and(|command| !command.is_empty())
    {
        let detail = url.map_or_else(
            || "Run the checked-in preview command".to_owned(),
            |url| format!("Serve {url}"),
        );
        rows.push(
            Row::new(format!("{START_PREVIEW}{project}"), "Start project preview")
                .meta("Project · Preview")
                .also(&detail)
                .also("Project · Preview"),
        );
    }
    if let Some(url) = url {
        rows.push(
            Row::new(format!("{OPEN_PREVIEW}{url}"), "Open project preview")
                .meta("Project · Preview")
                .also(url)
                .also("Project · Preview"),
        );
    }
    rows
}

/// The deck's rows for the project's `sikemux.json` actions, each run once the person trusts
/// the file.
/// The project's sikemux.json actions that give themselves a shortcut, as `(id, label, keys)`.
pub(crate) fn action_keys(config: &Value) -> Vec<(String, String, String)> {
    if config["status"] != "valid" {
        return Vec::new();
    }
    config["config"]["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|action| {
            let id = action["id"].as_str()?;
            let keys = action["keybinding"].as_str()?;
            let label = action["label"].as_str().unwrap_or(id);
            Some((id.to_owned(), label.to_owned(), keys.to_owned()))
        })
        .collect()
}

fn action_rows(config: &Value, cx: &App) -> Vec<Row> {
    if config["status"] != "valid" {
        return Vec::new();
    }
    let path = config["path"].as_str().unwrap_or("sikemux.json");
    let actions = config["config"]["actions"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    actions
        .iter()
        .filter_map(|action| {
            let id = action["id"].as_str()?;
            let label = action["label"].as_str().unwrap_or(id);
            let detail = action["description"]
                .as_str()
                .filter(|detail| !detail.is_empty())
                .map_or_else(|| format!("Run from {path}"), str::to_owned);
            Some(
                Row::new(format!("{ACTION}{id}"), label)
                    .keys(keymap::action_hint(id, cx))
                    .meta("Project · sikemux.json")
                    .also(&detail)
                    .also("Project · sikemux.json"),
            )
        })
        .collect()
}

/// The deck's rows for each worktree of the project's repository: open it as a project, and
/// remove it unless it is the main checkout or the one in front.
fn worktree_rows(worktrees: &Value) -> Vec<Row> {
    let mut rows = Vec::new();
    for worktree in worktrees.as_array().into_iter().flatten() {
        let Some(path) = worktree["path"].as_str() else {
            continue;
        };
        if worktree["bare"].as_bool() == Some(true) {
            continue;
        }
        let current = worktree["current"].as_bool() == Some(true);
        let branch = worktree["branch"].as_str().unwrap_or("detached");
        let title = if current {
            "Open current worktree".to_owned()
        } else {
            format!("Open worktree: {branch}")
        };
        rows.push(
            Row::new(format!("{OPEN_WORKTREE}{path}"), title)
                .meta("worktrees")
                .also(path),
        );
        if !current && worktree["is_main"].as_bool() != Some(true) {
            rows.push(
                Row::new(
                    format!("{REMOVE_WORKTREE}{path}"),
                    format!("Remove worktree: {branch}"),
                )
                .meta("worktrees")
                .also(path),
            );
        }
    }
    rows
}

impl Workbench {
    /// What is wrong with the project's `sikemux.json`, each as where in the file and why.
    fn config_errors(&self) -> Vec<String> {
        self.read("tasks")
            .filter(|read| read["configStatus"] == "invalid")
            .and_then(|read| read["configErrors"].as_array())
            .into_iter()
            .flatten()
            .map(|error| {
                format!(
                    "{} {}",
                    error["path"].as_str().unwrap_or_default(),
                    error["message"].as_str().unwrap_or_default()
                )
            })
            .collect()
    }

    /// The project's tasks to run, and the one running to restart or stop.
    fn task_rows(&self) -> Vec<Row> {
        let Some(read) = self.read("tasks") else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        if read["configStatus"] == "invalid" {
            let first = read["configErrors"][0]["message"]
                .as_str()
                .unwrap_or("sikemux.json is invalid");
            rows.push(
                Row::new(CONFIG_INVALID, "Project config needs attention")
                    .meta("Project · sikemux.json")
                    .also(first)
                    .also("Project · sikemux.json"),
            );
        }
        let Some(tasks) = read["tasks"].as_array() else {
            return rows;
        };
        rows.extend(tasks.iter().filter_map(|task| {
            let (id, label) = (task["id"].as_str()?, task["label"].as_str()?);
            let cwd = task["cwd"].as_str().unwrap_or_default();
            Some(
                Row::new(format!("{RUN_TASK}{id}"), label.to_owned())
                    .meta("Tasks")
                    .also(&format!("Run task from {cwd}"))
                    .also("Tasks"),
            )
        }));
        let Some(running) = self.person_runs().0 else {
            return rows;
        };
        if let Some(task) = running["taskId"]
            .as_str()
            .filter(|id| tasks.iter().any(|task| task["id"] == *id))
        {
            rows.push(
                Row::new(format!("{RESTART_TASK}{task}"), "Restart active task")
                    .meta("Tasks")
                    .also("Stop the current project task and start its current trusted definition"),
            );
        }
        if let Some(execution) = running["executionId"].as_str() {
            rows.push(
                Row::new(format!("{STOP_TASK}{execution}"), "Stop active task")
                    .meta("Tasks")
                    .also("Stop only the exact terminal owned by the current project task"),
            );
        }
        rows
    }

    pub fn toggle_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut rows: Vec<Row> = keymap::shortcuts()
            .iter()
            .filter(|shortcut| cx.build_action(shortcut.id, None).is_ok())
            .map(|shortcut| {
                Row::new(format!("{BUILTIN}{}", shortcut.id), shortcut.label)
                    .meta(shortcut.category)
                    .keys(keymap::hint(shortcut.id, cx))
                    .also(shortcut.detail)
                    .also(shortcut.category)
                    .also(shortcut.id)
            })
            .collect();
        rows.extend(
            self.standalone()
                .into_iter()
                .map(|(id, title, detail, category)| {
                    Row::new(format!("{STANDALONE}{id}"), title)
                        .meta(category)
                        .also(detail)
                        .also(category)
                }),
        );
        rows.extend(self.task_rows());
        rows.extend(self.plugin_palette_rows());
        if let Some(Strip::Project(_)) = self.strip() {
            let custom = self
                .preferences()
                .custom_commands
                .into_iter()
                .filter(|command| {
                    command.contexts.is_empty()
                        || command.contexts.iter().any(|kind| kind == "project")
                });
            rows.extend(custom.map(|command| {
                Row::new(format!("{CUSTOM}{}", command.id), command.title)
                    .meta("custom")
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
            "type a command…",
            rows,
            |row, bench, window, cx| {
                bench.remember_command(row.key.clone(), cx);
                if let Some(id) = row.key.strip_prefix(BUILTIN) {
                    keymap::dispatch(id, window, cx);
                } else if let Some(id) = row.key.strip_prefix(CUSTOM) {
                    bench.run_custom_command(id, cx);
                } else if let Some(id) = row.key.strip_prefix(ACTION) {
                    bench.run_command(json!({ "action": id }), row.title.to_string(), cx);
                } else if let Some(id) = row.key.strip_prefix(STANDALONE) {
                    bench.run_standalone(id, window, cx);
                } else if let Some(path) = row.key.strip_prefix(OPEN_WORKTREE) {
                    bench.open_place(
                        &format!("folder:{path}"),
                        crate::picker::ThenPicked::Open,
                        window,
                        cx,
                    );
                } else if let Some(task) = row
                    .key
                    .strip_prefix(RUN_TASK)
                    .or_else(|| row.key.strip_prefix(RESTART_TASK))
                {
                    bench.start_task(task, cx);
                } else if let Some(palette) = row
                    .key
                    .strip_prefix(PLUGIN_PALETTE)
                    .and_then(plugin_palette)
                {
                    bench.open_plugin_palette(palette, window, cx);
                } else if let Some(execution) = row.key.strip_prefix(STOP_TASK) {
                    bench.stop_run(execution);
                } else if let Some(project) = row.key.strip_prefix(START_PREVIEW) {
                    bench.start_preview(project, cx);
                } else if let Some(url) = row.key.strip_prefix(OPEN_PREVIEW) {
                    cx.open_url(url);
                } else if row.key.as_ref() == CONFIG_INVALID {
                    let errors = bench.config_errors().join(" · ");
                    bench.notify(
                        toast::error(format!("sikemux.json: {errors}")).autohide(false),
                        cx,
                    );
                } else if let Some(path) = row.key.strip_prefix(REMOVE_WORKTREE) {
                    bench.remove_worktree_at(path.to_owned(), row.title.to_string(), cx);
                }
            },
        )
        .empty("no commands match")
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
        let config = target.request("projectConfig", json!({ "project": folder }));
        let listed = self
            .git_repo()
            .map(|repo| target.request("git.worktrees", json!({ "repo": repo })));
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
    fn remove_worktree_at(&mut self, path: String, title: String, cx: &mut Context<Self>) {
        let Some(repo) = self.git_repo() else {
            return;
        };
        let branch = title.trim_start_matches("Remove worktree: ").to_owned();
        self.confirm(
            format!("Remove worktree {branch}?"),
            "Dirty worktrees will be refused.",
            "Remove",
            true,
            move |bench, yes, _, cx| {
                if !yes {
                    return;
                }
                let params = json!({ "repo": repo, "path": path, "force": false });
                let removed = bench.target().request("git.worktreeRemove", params);
                let branch = branch.clone();
                cx.spawn(async move |this, cx| {
                    let removed = removed.await;
                    let _ = this.update(cx, |bench, cx| {
                        let note = match removed {
                            Ok(_) => toast::success(format!("Removed worktree {branch}")),
                            Err(error) => {
                                toast::error(format!("Could not remove the worktree: {error}"))
                                    .autohide(false)
                            }
                        };
                        bench.notify(note, cx);
                    });
                })
                .detach();
            },
            cx,
        );
    }

    fn standalone(&self) -> Vec<Standalone> {
        let mut commands = Vec::new();
        if let Some(project) = self.project() {
            let path = project.path.to_string_lossy();
            commands.push((
                "project.settings",
                "Project settings",
                "Tasks, the default git repository, worktrees and language servers",
                "Project",
            ));
            commands.push(match crate::editor::language_servers_allowed(self, &path) {
                true => (
                    "project.language-servers.stop",
                    "Stop language servers for this project",
                    "Stop them now and do not start them again",
                    "Project · Language servers",
                ),
                false => (
                    "project.language-servers.allow",
                    "Allow language servers for this project",
                    "Start them when you open a file here",
                    "Project · Language servers",
                ),
            });
        }
        commands.extend([
            (
                "support.diagnostics",
                "Open runtime diagnostics",
                "Inspect redacted runtime and agent-detection health",
                "Support",
            ),
            (
                "support.whats-new",
                "Open What’s New",
                "Review the latest Sikemux release notes",
                "Support",
            ),
            (
                "support.onboarding",
                "Show welcome",
                "Open the first-run screen",
                "Support",
            ),
        ]);
        if self.terminal_in_front().is_some() {
            commands.push((
                "window.duplicate",
                "Duplicate active terminal",
                "Clone the active window into a new terminal tab",
                "Window",
            ));
        }
        commands.extend([
            (
                "app.quit-and-stop-everything",
                "Quit and Stop Everything",
                "Stop every terminal, terminal agent and task, then quit. Plain Quit leaves them running.",
                "Application",
            ),
            (
                "agents.reload-manifests",
                "Reload agent manifests",
                "Reload agent-state detection rules from disk",
                "Agents",
            ),
        ]);
        commands
    }

    fn run_standalone(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match id {
            "project.language-servers.allow" | "project.language-servers.stop" => {
                if let Some(project) = self.project() {
                    let path = project.path.to_string_lossy().into_owned();
                    crate::editor::allow_language_servers(self, path, id.ends_with("allow"));
                }
            }
            "project.settings" => {
                if let Some(project) = self.project().map(|project| project.id.clone()) {
                    self.open_project_settings(&project);
                }
            }
            "support.diagnostics" => crate::experience::open_diagnostics(self, cx),
            "support.whats-new" => crate::experience::open_whats_new(self, cx),
            "support.onboarding" => crate::experience::open_welcome(self, cx),
            "window.duplicate" => {
                if let Some((strip, tab)) = self.terminal_in_front() {
                    self.duplicate_tab(&strip, &tab);
                }
            }
            "app.quit-and-stop-everything" => {
                keymap::dispatch("app.quitAndStopEverything", window, cx)
            }
            "agents.reload-manifests" => self.reload_manifests(cx),
            _ => {}
        }
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
                        toast::error(format!("Agent manifests: {error}")).autohide(false),
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
            .request("startPreview", json!({ "project": project }));
        cx.spawn(async move |this, cx| {
            if let Err(error) = started.await {
                let _ = this.update(cx, |bench, cx| {
                    bench.notify(
                        toast::error(format!("The project preview did not start: {error}"))
                            .autohide(false),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// Runs one of the person's commands in the project in front, telling them
    /// what one in the background printed.
    fn run_custom_command(&self, id: &str, cx: &mut Context<Self>) {
        let title = self
            .preferences()
            .custom_commands
            .into_iter()
            .find(|command| command.id == id)
            .map_or_else(|| id.to_owned(), |command| command.title);
        self.run_command(json!({ "id": id }), title, cx);
    }

    /// Runs `which`, a custom command's id or a `sikemux.json` action's, in the project in
    /// front, telling the person what one in the background printed.
    pub(crate) fn run_command(&self, which: Value, title: String, cx: &mut Context<Self>) {
        let Some(Strip::Project(project)) = self.strip() else {
            return;
        };
        let tab = self.tab().map(|tab| tab.id.clone());
        let mut params = json!({ "project": project, "tab": tab, "pane": self.focused_view() });
        if let (Some(params), Some(which)) = (params.as_object_mut(), which.as_object()) {
            params.extend(which.clone());
        }
        let ran = core::request(&self.asks, "runCustomCommand", params);
        cx.spawn(async move |this, cx| {
            let ran = ran.await;
            let _ = this.update(cx, |bench, cx| match ran {
                Ok(ran) => {
                    if let Ok(popup) = serde_json::from_value::<SessionId>(ran["popup"].clone()) {
                        return bench.show_command_popup(title, popup, cx);
                    }
                    let Some(result) = ran.get("result") else {
                        return;
                    };
                    let output = result["output"].as_str().unwrap_or_default().trim();
                    let code = result["code"].as_i64().unwrap_or(-1);
                    let said = if output.is_empty() {
                        format!("exit {code}")
                    } else {
                        output.to_owned()
                    };
                    let said = format!("{title}: {said}");
                    let note = if code == 0 {
                        toast::success(said)
                    } else {
                        toast::error(said).autohide(false)
                    };
                    bench.notify(note, cx);
                }
                Err(error) => bench.notify(
                    toast::error(format!("{title}: {error}")).autohide(false),
                    cx,
                ),
            });
        })
        .detach();
    }
}

impl Workbench {
    /// A command for each palette an enabled plugin offers.
    fn plugin_palette_rows(&self) -> Vec<Row> {
        self.read("plugins")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|plugin| plugin["enabled"].as_bool() == Some(true))
            .filter_map(|plugin| {
                let name = plugin["name"].as_str()?.to_owned();
                let palettes = crate::plugin_pane::palettes(plugin["id"].as_str()?);
                Some(palettes.iter().map(move |palette| {
                    Row::new(
                        format!("{PLUGIN_PALETTE}{}/{}", palette.plugin, palette.id),
                        palette.title,
                    )
                    .meta(name.clone())
                    .also(&name)
                }))
            })
            .flatten()
            .collect()
    }
}

/// The plugin palette a command's key names, as `<plugin>/<palette>`.
fn plugin_palette(key: &str) -> Option<&'static crate::plugin_pane::PluginPalette> {
    let (plugin, id) = key.split_once('/')?;
    crate::plugin_pane::palettes(plugin)
        .iter()
        .find(|palette| palette.id == id)
}
