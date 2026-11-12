//! What `sikemux open` and an agent's `ui_open` ask the window to open. The core finds each
//! path's project and opens its tab; the window comes forward to show it, and tells the core
//! when a waiting `open`'s tabs have all closed.

use crate::core::Target;
use crate::hosts::Place;
use crate::tabs::shows;
use crate::toast;
use crate::workbench::Strip;
use gpui_kit::Window;
use sikemux_core::server::layout::HostParams;
use sikemux_core::server::workspace::SessionChoice;
use sikemux_view::device::DeviceView;
use sikemux_view::device::RecentInfo;
use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{App, AppContext, Context, Task};
use serde_json::{json, Value};
use sikemux_core::cli::protocol::{
    is_browser_method, is_sim_method, CliOpenFailure, CliOpenOutcome, CliOpenRequest,
    CliOpenTarget, CliTargetKind, HarnessRequest, MAX_CLI_TARGETS,
};
use sikemux_core::server::call;
use sikemux_core::server::calls::AgentParams;
use sikemux_core::server::ship::WebWriteParams;
use sikemux_view::layout::{At, OpenTab, PaneKind};
use sikemux_wire::protocol::{CallId, WindowAnswer, WindowCall};
use std::pin::Pin;

use crate::core::Ask;
use crate::editor::OpenAt;
use crate::workbench::Workbench;
use sikemux_core::server::harness::pages::PageParams;
use sikemux_core::server::layout::OpenPaths;

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
        let opening = self.open_targets(request.targets, false, cx);
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

    /// Opens the files and folders the system handed this app, such as from Finder's Open With,
    /// as `sikemux open` would, saying which could not open.
    pub(crate) fn open_handed_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let targets: Vec<CliOpenTarget> = paths
            .into_iter()
            .filter_map(|path| {
                let kind = match std::fs::metadata(&path) {
                    Ok(metadata) if metadata.is_dir() => CliTargetKind::Directory,
                    Ok(metadata) if metadata.is_file() => CliTargetKind::File,
                    _ => return None,
                };
                let project_root = sikemux_host::cli_client::project_root(&path, kind);
                Some((path, kind, project_root))
            })
            .take(MAX_CLI_TARGETS)
            .enumerate()
            .map(|(index, (path, kind, project_root))| CliOpenTarget {
                id: format!("handed-{index}"),
                kind,
                path: path.to_string_lossy().into_owned(),
                project_root: project_root.to_string_lossy().into_owned(),
                line: None,
                column: None,
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        self.bring_window_forward(cx);
        let opening = self.open_targets(targets, false, cx);
        cx.spawn(async move |bench, cx| {
            let (outcome, _) = opening.await;
            let _ = bench.update(cx, |bench, cx| {
                for failure in outcome.failed {
                    bench.notify(crate::toast::error(failure.message), cx);
                }
            });
        })
        .detach();
    }

    /// Tells the core of each waiting `open` whose tabs have all closed. A tab counts as closed
    /// once the layout has shown it and then lost it, since the layout that shows a new tab
    /// arrives after the core says it opened.
    pub(crate) fn settle_open_waits(&mut self) {
        let Some(view) = &self.feeds.this.view else {
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

    /// An agent's `ui_open`: a file in the editor, the project's changes, a task's terminal, a
    /// mod, or the project's preview on the agent's desk; the window comes forward with `focus`.
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
                self.open_file_on_desk(&agent, PathBuf::from(&path), line, None, cx);
                self.parts.redraw_stage(cx);
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
                let opening = self.open_targets(vec![target], !focus, cx);
                cx.background_spawn(async move {
                    let (outcome, tabs) = opening.await;
                    answer(match outcome.failed.into_iter().next() {
                        Some(failed) => Err(failed.message),
                        None => Ok(
                            json!({ "kind": kind, "path": path, "windowId": tabs.into_iter().next() }),
                        ),
                    });
                })
                .detach();
            }
            "terminal"
                if params["session"]
                    .as_u64()
                    .is_some_and(|session| self.show_desk_terminal(session, cx).is_some()) =>
            {
                self.parts.redraw_stage(cx);
                answer(Ok(json!({ "kind": kind, "agentId": request.agent_id })));
            }
            "diff" | "terminal" => {
                let Some(project) = self.project_here(&request.project) else {
                    return answer(Err("That project is not open in Sikemux".into()));
                };
                let at = At::project(&project);
                let tab = if kind == "diff" {
                    OpenTab::new(at, PaneKind::Git, request.project)
                } else {
                    let Some(session) = params["session"].as_u64() else {
                        return answer(Err("The process's terminal is no longer open".into()));
                    };
                    let tab = OpenTab::new(at, PaneKind::Terminal, session.to_string());
                    OpenTab {
                        title: params["title"].as_str().map(str::to_owned),
                        ..tab
                    }
                };
                let opened = crate::core::Target::here(&self.asks)
                    .request_typed::<call::OpenTab>(tab.behind(!focus));
                cx.background_spawn(async move {
                    answer(
                        opened
                            .await
                            .map(|tab| json!({ "kind": kind, "windowId": tab })),
                    );
                })
                .detach();
            }
            "mod" => {
                let Some(project) = self.project_here(&request.project) else {
                    return answer(Err("That project is not open in Sikemux".into()));
                };
                let tab = OpenTab::new(
                    At::project(&project),
                    PaneKind::Mod,
                    params["mod"].as_str().unwrap_or_default(),
                );
                let tab = OpenTab {
                    title: params["title"].as_str().map(str::to_owned),
                    ..tab
                };
                let opened = crate::core::Target::here(&self.asks)
                    .request_typed::<call::OpenTab>(tab.behind(!focus));
                cx.background_spawn(async move {
                    answer(
                        opened
                            .await
                            .map(|tab| json!({ "kind": kind, "windowId": tab })),
                    );
                })
                .detach();
            }
            "preview" => {
                let (Some(agent), Some(url)) =
                    (request.agent_id.as_deref(), params["url"].as_str())
                else {
                    return answer(Err("No preview URL is configured in sikemux.json".into()));
                };
                let shown = match focus {
                    true => self.open_on_desk(agent, url, cx),
                    false => self.show_on_desk(agent, url, cx),
                };
                match shown {
                    true => answer(Ok(json!({ "kind": kind, "agentId": agent, "url": url }))),
                    false => answer(Err(
                        "This window has no browser to show the preview in".into()
                    )),
                }
            }
            _ => answer(Err(
                "kind must be file, diff, terminal, preview, or mod".into()
            )),
        }
    }

    /// Has this host's core open each target, each file's cursor going to its line once it is
    /// read, and gathers what opened and the tabs the files opened in.
    pub(crate) fn open_targets(
        &mut self,
        targets: Vec<CliOpenTarget>,
        behind: bool,
        cx: &mut App,
    ) -> impl Future<Output = (CliOpenOutcome, HashSet<String>)> + Send + 'static {
        let here = crate::core::Target::new(&self.asks, crate::hosts::Place::This);
        self.open_targets_on(&here, targets, behind, cx)
    }

    /// Has the core `target` reaches open each target, as `open_targets` does on this host's.
    pub(crate) fn open_targets_on(
        &mut self,
        target: &crate::core::Target,
        targets: Vec<CliOpenTarget>,
        behind: bool,
        cx: &mut App,
    ) -> impl Future<Output = (CliOpenOutcome, HashSet<String>)> + Send + 'static {
        for target in &targets {
            if let (CliTargetKind::File, Some(line)) = (target.kind, target.line) {
                let at = OpenAt {
                    path: PathBuf::from(&target.path),
                    line: line.saturating_sub(1),
                    column: target.column.map(|column| column.saturating_sub(1)),
                };
                self.editors.update(cx, |editors, _| editors.reveal(at));
            }
        }
        let ids: Vec<String> = targets.iter().map(|target| target.id.clone()).collect();
        let asked = target.request_typed::<call::OpenPaths>(OpenPaths { targets, behind });
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
                    for answer in answers {
                        let target_id = answer.target_id;
                        match answer.error {
                            Some(message) => {
                                outcome.failed.push(CliOpenFailure { target_id, message })
                            }
                            None => {
                                tabs.extend(answer.tab);
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
        let workspace = &self.this_view()?.workspace;
        workspace
            .projects
            .iter()
            .find(|project| project.path == Path::new(path))
            .map(|project| project.id.clone())
    }

    pub(crate) fn bring_window_forward(&self, cx: &mut Context<Self>) {
        cx.activate(true);
        let _ = self
            .window
            .update(cx, |_, window, _| window.activate_window());
    }
}

impl Workbench {
    /// Answers a call the core hands this window: an agent's browser tools run on the
    /// engine, on a thread of their own, since most answers wait on a page.
    pub fn answer_window_call(
        &mut self,
        call_id: CallId,
        call: WindowCall,
        cx: &mut Context<Self>,
    ) {
        let asks = self.asks.clone();
        let answer: crate::opening::Answer = Box::new(move |answer: Result<Value, String>| {
            let _ = asks.send(Ask::AnswerWindow(call_id, WindowAnswer::from(answer)));
        });
        let request = match call {
            WindowCall::Open { request } => {
                return self.open_from_cli(call_id, request, answer, cx)
            }
            WindowCall::Harness { request } => request,
        };
        let request: HarnessRequest = match serde_json::from_value(request) {
            Ok(request) => request,
            Err(error) => {
                return answer(Err(format!(
                    "Sikemux could not read the tool call: {error}"
                )))
            }
        };
        match request.method.as_str() {
            "app.console" => return answer(crate::console::read(&request.params)),
            "page.show" => return self.show_page(request, answer, cx),
            "ui.open" => return self.ui_open(request, answer, cx),
            _ => {}
        }
        let run: Box<dyn FnOnce() -> Result<Value, String> + Send> =
            if is_sim_method(&request.method) {
                let engine = Arc::clone(self.sims.read(cx).engine());
                Box::new(move || sikemux_simulator::tools::execute(&engine, &request))
            } else if let Some(engine) = self
                .desks
                .read(cx)
                .engine
                .clone()
                .filter(|_| is_browser_method(&request.method))
            {
                let core = Target::here(&self.asks);
                Box::new(move || {
                    let agent = request.agent_id.clone().unwrap_or_default();
                    let project = request.project.clone();
                    let check = move |what: String, origin: String| {
                        let asked = core.request_typed::<call::CheckWebWrite>(WebWriteParams {
                            agent: agent.clone(),
                            project: project.clone(),
                            what,
                            origin,
                        });
                        Box::pin(async move { asked.await?.map_or(Ok(()), Err) })
                            as Pin<Box<dyn Future<Output = Result<(), String>>>>
                    };
                    sikemux_browser::tools::execute(&engine, &request, &check)
                })
            } else {
                return answer(Err(format!(
                    "this window cannot answer {} yet",
                    request.method
                )));
            };
        let spawned = std::thread::Builder::new()
            .name("sikemux-window-call".into())
            .spawn(move || answer(run()));
        if let Err(error) = spawned {
            eprintln!("sikemux: a tool call could not start: {error}");
        }
    }

    /// A page an agent kept, which an agent in a terminal, or one that asked for a tab, sees on
    /// its desk. A chat's card for the page gets a picture of the tab.
    pub(crate) fn show_page(
        &self,
        request: HarnessRequest,
        answer: impl FnOnce(Result<Value, String>) + Send + 'static,
        cx: &App,
    ) {
        let Some(agent) = request.agent_id else {
            return answer(Ok(json!({})));
        };
        let id = request.params["id"].as_str().unwrap_or_default().to_owned();
        if !sikemux_core::pages::is_page_id(&id) {
            return answer(Err("that is not a page the core keeps".into()));
        }
        let page = (request.params["desk"] == true).then(|| {
            Target::here(&self.asks).request_typed::<call::ReadPage>(PageParams { id: id.clone() })
        });
        let picture = request.params["picture"] == true;
        let files = self.desks.read(cx).page_files.clone();
        crate::desk::spawn(async move {
            let Some(page) = page else {
                return answer(Ok(json!({})));
            };
            let shown = async {
                let page = page.await?;
                let html = page.html;
                let url = files.url(&id, &html)?;
                let engine = files.engine()?;
                engine
                    .open_tab(&agent, Some(&url))
                    .await
                    .map_err(|error| error.to_string())?;
                if !picture {
                    return Ok(json!({}));
                }
                let taken = engine.front_picture(&agent).await.ok();
                let picture = taken.and_then(|taken| crate::chat::page::thumbnail(&taken));
                Ok(json!({ "picture": picture }))
            };
            answer(shown.await);
        });
    }
}

impl Workbench {
    /// Starts a chat in the project in front, or in the one picked when no project is in front,
    /// or on an SSH host's strip opens its Install Sikemux pane.
    pub fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.strip() {
            Some(Strip::Host(alias)) => self.open_install_pane(&alias),
            Some(Strip::Project(project)) => self.start_chat_in(&project, window, cx),
            _ => self.pick_project(crate::pickers::picker::ThenPicked::StartChat, window, cx),
        }
    }

    /// Starts a chat in `project` with the agent the rail starts with, or offers every agent when
    /// none is installed.
    pub fn start_chat_in(&mut self, project: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.launcher() {
            Some(launcher) => {
                let launcher = self.new_chat_launcher(launcher);
                self.target()
                    .start_chat(launcher, project.to_owned(), None, None)
            }
            None => self.toggle_agent_palette(window, cx),
        }
    }

    /// Takes up a saved chat again, in the project it ran in, and brings its tab forward.
    /// It goes by the chat's session rather than its place in the core's list, which forgets
    /// the oldest chats any device paged to.
    pub fn resume_chat(&self, recent: &RecentInfo, cx: &mut Context<Self>) {
        self.resume_chat_finding(recent, None, cx);
    }

    /// As [`Self::resume_chat`], opening the chat's find on `find` once it shows.
    pub fn resume_chat_finding(
        &self,
        recent: &RecentInfo,
        find: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let session = recent
            .id
            .strip_prefix(&format!("{}:", recent.provider))
            .unwrap_or(&recent.id);
        let taken = self.resume_session(
            &recent.project,
            recent.provider.clone(),
            session.to_owned(),
            recent.cwd.clone(),
            recent.title.clone(),
            cx,
        );
        let Some(find) = find else {
            return taken.detach();
        };
        cx.spawn(async move |bench, cx| {
            if let Some(agent) = taken.await {
                let _ = bench.update(cx, |bench, cx| {
                    bench.find_once_attached(agent, Some(find), cx)
                });
            }
        })
        .detach();
    }

    /// Takes `provider`'s session up again in the project `project`, in a tab of its own.
    /// Answers with the agent that took it up.
    pub fn resume_session(
        &self,
        project: &str,
        provider: String,
        session_id: String,
        cwd: PathBuf,
        title: String,
        cx: &mut Context<Self>,
    ) -> Task<Option<String>> {
        let Some(project) = self.projects().iter().find(|known| known.id == project) else {
            return Task::ready(None);
        };
        let asked = SessionChoice {
            provider,
            session_id,
            project: project.path.clone(),
            cwd: Some(cwd),
            title: Some(title),
        };
        let target = self.target();
        let taken = target.request_typed::<call::ResumeSession>(asked);
        cx.spawn(async move |bench, cx| match taken.await {
            Ok(taken) => {
                target.call_typed::<call::OpenTab>(OpenTab::new(
                    At::project(&taken.project),
                    PaneKind::Agent,
                    taken.agent_id.clone(),
                ));
                Some(taken.agent_id)
            }
            Err(error) => {
                let note = toast::error(t!("The chat did not open: {error}", error = error));
                let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
                None
            }
        })
    }

    /// Brings the tab of the agent `id` to the front, in its project, opening one when none
    /// shows it, with the workbench in place of the goals. A closed agent's chat opens again on
    /// this host.
    pub fn open_agent(&mut self, id: &str, cx: &mut Context<Self>) {
        self.open_agent_from(id, Target::here(&self.asks), cx);
    }

    /// As [`Self::open_agent`], for an agent named by a record of the core `from`, which opens
    /// its chat again when it has closed.
    pub fn open_agent_from(&mut self, id: &str, from: Target, cx: &mut Context<Self>) {
        self.bring_agent_from(id, from, cx).detach();
    }

    /// Brings the tab of the agent `id` to the front, in its project, opening one when none
    /// shows it. A closed agent's chat opens again on this host. Answers with the agent the tab
    /// shows.
    pub fn bring_agent(&mut self, id: &str, cx: &mut Context<Self>) -> Task<Option<String>> {
        self.bring_agent_from(id, Target::here(&self.asks), cx)
    }

    fn bring_agent_from(
        &mut self,
        id: &str,
        from: Target,
        cx: &mut Context<Self>,
    ) -> Task<Option<String>> {
        match self.bring_open_agent(id) {
            true => Task::ready(Some(id.to_owned())),
            false => Self::reopen_agent(id, from, cx),
        }
    }

    /// Asks the core `on` to open the chat of the closed agent `id` again and brings its tab
    /// forward, answering with the agent it opened as.
    fn reopen_agent(id: &str, on: Target, cx: &mut Context<Self>) -> Task<Option<String>> {
        let taken = on.request_typed::<call::ReopenAgent>(AgentParams {
            agent_id: id.to_owned(),
        });
        cx.spawn(async move |bench, cx| match taken.await {
            Ok(taken) => {
                on.call_typed::<call::OpenTab>(OpenTab::new(
                    At::project(&taken.project),
                    PaneKind::Agent,
                    taken.agent_id.clone(),
                ));
                Some(taken.agent_id)
            }
            Err(error) => {
                let note = toast::error(t!("The chat did not open: {error}", error = error));
                let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
                None
            }
        })
    }

    /// Opens the chief of the project `project` in front, making its chat the first time and
    /// bringing a closed one back as itself.
    pub fn open_chief(&self, project: &str, cx: &mut Context<Self>) {
        let target = self.target();
        let opening =
            target.request_typed::<call::OpenChief>(sikemux_core::server::chief::OpenChiefParams {
                project: project.to_owned(),
            });
        let project = project.to_owned();
        cx.spawn(async move |bench, cx| match opening.await {
            Ok(opened) => {
                target.call_typed::<call::OpenTab>(OpenTab::new(
                    At::project(&project),
                    PaneKind::Agent,
                    opened.agent_id,
                ));
            }
            Err(error) => {
                let note = toast::error(t!("@Chief did not open: {error}", error = error));
                let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
            }
        })
        .detach();
    }

    /// Brings the tab of the open agent `id` forward, and says whether any view lists it.
    fn bring_open_agent(&mut self, id: &str) -> bool {
        let project_on = |view: &DeviceView| {
            view.workspace
                .agents
                .iter()
                .find(|agent| agent.id == id)
                .map(|agent| agent.project.clone())
        };
        let tab = |project: String| OpenTab::new(At::project(&project), PaneKind::Agent, id);
        let Some(project) = self.shown().and_then(project_on) else {
            // Opening a tab on a host's layout brings that host forward there.
            if let Some(project) = self.this_view().and_then(project_on) {
                self.open_here(tab(project));
                return true;
            }
            if let Some((host, project)) = self
                .feeds
                .hosts()
                .find_map(|(host, feed)| Some((host.clone(), project_on(feed.view.as_ref()?)?)))
            {
                Target::here(&self.asks).call_typed::<call::ChooseHost>(HostParams {
                    host: format!("host:{host}"),
                });
                Target::new(&self.asks, Place::Account(host))
                    .call_typed::<call::OpenTab>(tab(project));
                return true;
            }
            return false;
        };
        let strip = Strip::Project(project.clone());
        let shown = (self.tabs(&strip).iter())
            .find(|tab| shows(tab, PaneKind::Agent).is_some_and(|shown| shown == id))
            .map(|tab| tab.id.clone());
        match shown {
            Some(shown) => {
                self.activate_project(&project);
                self.activate_tab(&strip, &shown);
            }
            None => self.open_in_front(tab(project)),
        }
        true
    }
}
