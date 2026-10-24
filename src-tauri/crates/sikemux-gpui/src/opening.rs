//! What `sikemux open` and an agent's `ui_open` ask the window to open. The core finds each
//! path's project and opens its tab; the window comes forward to show it, and tells the core
//! when a waiting `open`'s tabs have all closed.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};

use gpui_kit::{AppContext, Context};
use serde_json::{json, Value};
use sikemux_core::cli::protocol::{
    CliOpenFailure, CliOpenOutcome, CliOpenRequest, CliOpenTarget, CliTargetKind, HarnessRequest,
};
use sikemux_wire::protocol::CallId;

use crate::core::Ask;
use crate::editor::OpenAt;
use crate::workbench::Workbench;

pub type Answer = Box<dyn FnOnce(Result<Value, String>) + Send>;

impl Workbench {
    /// The CLI's `open`: answers what opened, and with `--wait` tells the core once every
    /// file tab it opened has closed.
    pub(crate) fn open_from_cli(
        &mut self,
        call_id: CallId,
        request: Value,
        answer: Answer,
        cx: &mut Context<Self>,
    ) {
        let request: CliOpenRequest = match serde_json::from_value(request) {
            Ok(request) => request,
            Err(error) => {
                return answer(Err(format!(
                    "Sikemux could not read the open request: {error}"
                )))
            }
        };
        self.bring_window_forward(cx);
        let wait = request.wait;
        let opening = self.open_targets(request.targets, false);
        cx.spawn(async move |bench, cx| {
            let (outcome, tabs) = opening.await;
            answer(serde_json::to_value(outcome).map_err(|error| error.to_string()));
            if wait {
                let _ = bench.update(cx, |bench, _| {
                    let tabs = tabs.into_iter().map(|tab| (tab, false)).collect();
                    bench.open_waits.push((call_id, tabs));
                    bench.settle_open_waits();
                });
            }
        })
        .detach();
    }

    /// Tells the core of each waiting `open` whose tabs have all closed. A tab counts as closed
    /// once the layout has shown it and then lost it, since the layout that shows a new tab
    /// arrives after the core says it opened.
    pub(crate) fn settle_open_waits(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let shown: HashSet<&str> = view
            .layout
            .projects
            .values()
            .flat_map(|strip| strip.tabs.iter().map(|tab| tab.id.as_str()))
            .collect();
        let asks = &self.asks;
        self.open_waits.retain_mut(|(call, tabs)| {
            tabs.retain(|tab, seen| {
                let open = shown.contains(tab.as_str());
                *seen |= open;
                open || !*seen
            });
            let closed = tabs.is_empty();
            if closed {
                let _ = asks.send(Ask::WindowOpenClosed(*call));
            }
            !closed
        });
    }

    /// An agent's `ui_open`: a file in the editor, the project's changes, a task's terminal,
    /// or the project's preview on the agent's desk; the window comes forward with `focus`.
    pub(crate) fn ui_open(
        &mut self,
        request: HarnessRequest,
        answer: Answer,
        cx: &mut Context<Self>,
    ) {
        let params = &request.params;
        let focus = match params.get("focus") {
            None | Some(Value::Null) => false,
            Some(focus) => match focus.as_bool() {
                Some(focus) => focus,
                None => return answer(Err("focus must be a boolean".into())),
            },
        };
        if focus {
            self.bring_window_forward(cx);
        }
        let kind = params["kind"].as_str().unwrap_or_default().to_owned();
        match kind.as_str() {
            "file" if request.agent_id.is_some() => {
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                let line = params["line"].as_u64().map(|line| line as u32);
                let agent = request.agent_id.clone().unwrap_or_default();
                self.open_file_on_desk(&agent, PathBuf::from(&path), line, None);
                self.parts.redraw(&self.parts.stage, cx);
                answer(Ok(json!({ "kind": kind, "agentId": agent, "path": path })));
            }
            "file" => {
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                let target = CliOpenTarget {
                    id: request.id.clone(),
                    kind: CliTargetKind::File,
                    path: path.clone(),
                    project_root: request.project.clone(),
                    line: params["line"].as_u64().map(|line| line as u32),
                    column: None,
                };
                let opening = self.open_targets(vec![target], !focus);
                cx.background_spawn(async move {
                    let (outcome, _) = opening.await;
                    answer(match outcome.failed.into_iter().next() {
                        Some(failed) => Err(failed.message),
                        None => Ok(json!({ "kind": kind, "path": path })),
                    });
                })
                .detach();
            }
            "terminal"
                if params["session"]
                    .as_u64()
                    .is_some_and(|session| self.show_desk_terminal(session).is_some()) =>
            {
                self.parts.redraw(&self.parts.stage, cx);
                answer(Ok(json!({ "kind": kind, "agentId": request.agent_id })));
            }
            "diff" | "terminal" => {
                let Some(project) = self.project_here(&request.project) else {
                    return answer(Err("That project is not open in Sikemux".into()));
                };
                let tab = if kind == "diff" {
                    json!({ "project": project, "kind": "git", "target": request.project, "behind": !focus })
                } else {
                    let Some(session) = params["session"].as_u64() else {
                        return answer(Err("Task terminal is no longer open".into()));
                    };
                    json!({ "project": project, "kind": "terminal", "target": session.to_string(), "title": params["title"], "behind": !focus })
                };
                let opened = crate::core::request(&self.asks, "openTab", tab);
                cx.background_spawn(async move {
                    answer(opened.await.map(|_| json!({ "kind": kind })));
                })
                .detach();
            }
            "preview" => {
                let (Some(agent), Some(url)) =
                    (request.agent_id.as_deref(), params["url"].as_str())
                else {
                    return answer(Err("No preview URL is configured in sikemux.json".into()));
                };
                match self.open_on_desk(agent, url) {
                    true => answer(Ok(json!({ "kind": kind, "agentId": agent, "url": url }))),
                    false => answer(Err(
                        "This window has no browser to show the preview in".into()
                    )),
                }
            }
            _ => answer(Err("kind must be file, diff, terminal, or preview".into())),
        }
    }

    /// Has this host's core open each target, each file's cursor going to its line once it is
    /// read, and gathers what opened and the tabs the files opened in.
    pub(crate) fn open_targets(
        &mut self,
        targets: Vec<CliOpenTarget>,
        behind: bool,
    ) -> impl Future<Output = (CliOpenOutcome, HashSet<String>)> + Send + 'static {
        let here = crate::core::Target::new(&self.asks, crate::hosts::Place::This);
        self.open_targets_on(&here, targets, behind)
    }

    /// Has the core `target` reaches open each target, as `open_targets` does on this host's.
    pub(crate) fn open_targets_on(
        &mut self,
        target: &crate::core::Target,
        targets: Vec<CliOpenTarget>,
        behind: bool,
    ) -> impl Future<Output = (CliOpenOutcome, HashSet<String>)> + Send + 'static {
        for target in &targets {
            if let (CliTargetKind::File, Some(line)) = (target.kind, target.line) {
                self.editors.reveal(OpenAt {
                    path: PathBuf::from(&target.path),
                    line: line.saturating_sub(1),
                    column: target.column.map(|column| column.saturating_sub(1)),
                });
            }
        }
        let ids: Vec<String> = targets.iter().map(|target| target.id.clone()).collect();
        let asked = target.request("openPaths", json!({ "targets": targets, "behind": behind }));
        async move {
            let mut outcome = CliOpenOutcome::default();
            let mut tabs = HashSet::new();
            match asked.await {
                Err(message) => {
                    outcome.failed = ids
                        .into_iter()
                        .map(|target_id| CliOpenFailure {
                            target_id,
                            message: message.clone(),
                        })
                        .collect();
                }
                Ok(answers) => {
                    for answer in answers.as_array().into_iter().flatten() {
                        let target_id = answer["targetId"].as_str().unwrap_or_default().to_owned();
                        match answer["error"].as_str() {
                            Some(message) => outcome.failed.push(CliOpenFailure {
                                target_id,
                                message: message.to_owned(),
                            }),
                            None => {
                                if let Some(tab) = answer["tab"].as_str() {
                                    tabs.insert(tab.to_owned());
                                }
                                outcome.opened.push(target_id);
                            }
                        }
                    }
                }
            }
            (outcome, tabs)
        }
    }

    /// The project at `path` on this host, where the core that asked keeps it, by id.
    fn project_here(&self, path: &str) -> Option<String> {
        let workspace = &self.view.as_ref()?.workspace;
        workspace
            .projects
            .iter()
            .find(|project| project.path == Path::new(path))
            .map(|project| project.id.clone())
    }

    fn bring_window_forward(&self, cx: &mut Context<Self>) {
        cx.activate(true);
        let _ = self
            .window
            .update(cx, |_, window, _| window.activate_window());
    }
}
