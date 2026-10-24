//! A project's code host as the Git tab shows it in place of the local workbench: its
//! pull requests, CI runs, issues and releases, as the core lists them.

mod compose;
mod diffs;
pub mod inbox;
mod issues;
mod logs;
mod pictures;
mod pull;
mod pulls;
mod releases;
mod run_actions;
mod runs;
pub(crate) mod shell;
mod thread;

use std::future::Future;
use std::rc::Rc;

use gpui_kit::{
    div, px, AnyElement, App, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sikemux_wire::codehost::{CheckTally, Label, Outcome, PullState};
use sikemux_wire::protocol::ProjectCodeHost;

use crate::core::{Target, Watch};
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

/// What the Git tab shows: the local workbench, or one of the code host's areas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Area {
    #[default]
    Local,
    Pulls,
    Actions,
    Issues,
    Releases,
}

impl Area {
    /// The area a code host's section names, as the core lists them.
    pub fn of(section: &str) -> Option<Self> {
        Some(match section {
            "pulls" => Self::Pulls,
            "actions" => Self::Actions,
            "issues" => Self::Issues,
            "releases" => Self::Releases,
            _ => return None,
        })
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Local => Icon::Git,
            Self::Pulls => Icon::PullRequest,
            Self::Actions => Icon::Actions,
            Self::Issues => Icon::Issues,
            Self::Releases => Icon::Releases,
        }
    }

    pub fn label(self, host: &ProjectCodeHost) -> SharedString {
        match self {
            Self::Local => "Git".into(),
            Self::Pulls => "Pull requests".into(),
            Self::Actions => host.ci_name.clone().into(),
            Self::Issues => "Issues".into(),
            Self::Releases => "Releases".into(),
        }
    }
}

/// Brings the project's Git tab to the front showing `area`.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(name = "git.openArea", no_json)]
pub struct OpenArea {
    pub area: Area,
}

/// What a code-host list answered: its value, or why the host could not give it.
type Read<T> = Option<Result<T, SharedString>>;

/// What the person agreeing does.
type Then = Rc<dyn Fn(&mut HostArea, &mut Context<HostArea>)>;

/// What the person is asked before a change the host cannot take back.
pub struct Asking {
    title: SharedString,
    body: SharedString,
    yes: SharedString,
    destructive: bool,
    then: Then,
}

impl Asking {
    pub fn new(
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        destructive: bool,
        then: impl Fn(&mut HostArea, &mut Context<HostArea>) + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            yes: yes.into(),
            destructive,
            then: Rc::new(then),
        }
    }
}

/// How something the person did went.
pub enum Said {
    Done(SharedString),
    Failed(SharedString),
}

/// The code host's areas for one project.
pub struct HostArea {
    core: Target,
    /// The host the areas ask, while it is out of reach: they show what it last said and
    /// take no changes.
    out_of_reach: Option<SharedString>,
    bench: WeakEntity<Workbench>,
    /// The project's folder, which the core's lists are asked by.
    pub project: String,
    host: ProjectCodeHost,
    /// The account the project picked on its host, else the host's default.
    account: Option<String>,
    area: Area,
    pulls: pulls::Pulls,
    pull: Option<pull::Opened>,
    runs: runs::Runs,
    releases: releases::Releases,
    issues: issues::Issues,
    /// A new pull request being written, while one is.
    composing: Option<compose::NewPull>,
    /// The branch of the new pull request being picked, while one is.
    picking: Option<compose::End>,
    asking: Option<Asking>,
    /// Another repository on the host, while a pull request or issue of it is open.
    elsewhere: Option<String>,
    /// Who is signed in to the host, once it said.
    status: Read<shell::Status>,
    asked_status: bool,
    /// The sign-in form, while it shows.
    signing: Option<shell::SignIn>,
    /// The account menu is open.
    menu: bool,
    /// The host's accounts, read as the menu opens.
    accounts: Vec<shell::Listed>,
    budget: Option<shell::Budget>,
    /// How long to wait before reading a rate limit again that says it is spent but past its reset.
    budget_again: crate::retry::Backoff,
    pictures: crate::avatars::Avatars,
}

impl HostArea {
    pub fn new(
        core: Target,
        bench: WeakEntity<Workbench>,
        project: String,
        host: ProjectCodeHost,
        account: Option<String>,
        branch: Option<(String, bool)>,
    ) -> Self {
        Self {
            core,
            out_of_reach: None,
            bench,
            project,
            host,
            account,
            area: Area::Local,
            pulls: pulls::Pulls::new(branch),
            pull: None,
            runs: runs::Runs::new(),
            releases: releases::Releases::default(),
            issues: issues::Issues::default(),
            composing: None,
            picking: None,
            asking: None,
            elsewhere: None,
            status: None,
            asked_status: false,
            signing: None,
            menu: false,
            accounts: Vec::new(),
            budget: None,
            budget_again: crate::retry::Backoff::new(
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(60),
            ),
            pictures: Default::default(),
        }
    }

    /// The host whose core the areas ask.
    pub fn place(&self) -> &crate::hosts::Place {
        self.core.place()
    }

    /// Hears that the areas' host went out of reach, by its name, or came back.
    pub fn set_out_of_reach(&mut self, host: Option<SharedString>, cx: &mut Context<Self>) {
        if self.out_of_reach == host {
            return;
        }
        let back = self.out_of_reach.is_some() && host.is_none();
        self.out_of_reach = host;
        if back && !self.signed_in() {
            self.status = None;
            self.read_status(cx);
        }
        cx.notify();
    }

    /// Shows `area`, asking the core for its lists the first time.
    pub fn show(&mut self, area: Area, cx: &mut Context<Self>) {
        if !self.asked_status {
            self.asked_status = true;
            self.read_status(cx);
        }
        if self.area != area {
            self.area = area;
            self.watch();
            cx.notify();
        }
    }

    /// Opens pull request or issue `number` of `repo` in `area`.
    pub fn open_item(&mut self, area: Area, number: u64, repo: String, cx: &mut Context<Self>) {
        self.elsewhere = (repo != self.host.repo).then_some(repo);
        self.show(area, cx);
        match area {
            Area::Pulls => self.open_pull(number, cx),
            Area::Issues => self.open_issue(number, cx),
            _ => {}
        }
    }

    /// Takes what the project's repository on its host now is, and where its branch stands.
    pub fn project_moved(
        &mut self,
        host: &ProjectCodeHost,
        account: Option<String>,
        branch: Option<(String, bool)>,
        cx: &mut Context<Self>,
    ) {
        let elsewhere =
            self.host.host != host.host || self.host.repo != host.repo || self.account != account;
        if &self.host == host && self.pulls.branch == branch && !elsewhere {
            return;
        }
        self.host = host.clone();
        self.pulls.branch = branch;
        if elsewhere {
            self.account = account;
            self.elsewhere = None;
            self.pull = None;
            self.composing = None;
            self.runs.open = None;
            self.issues = issues::Issues::default();
            self.pulls.list = None;
            self.runs.list = None;
            self.releases = releases::Releases::default();
            self.status = None;
            self.read_status(cx);
        }
        cx.notify();
    }

    /// Takes a watched list's answer, under the key it was watched by.
    pub fn read(&mut self, key: &str, value: &Value, cx: &mut Context<Self>) {
        match key {
            pulls::KEY => self.pulls.list = Some(answer(value)),
            pull::VIEW => {
                if let Some(opened) = &mut self.pull {
                    opened.take_view(Some(answer(value)));
                }
            }
            pull::FILES => {
                if let Some(opened) = &mut self.pull {
                    opened.take_files(answer(value));
                }
            }
            runs::LIST => self.runs.list = Some(answer(value)),
            releases::KEY => self.releases.list = Some(answer(value)),
            issues::KEY => self.issues.list = Some(answer(value)),
            runs::RUN => {
                if self.take_run(value) {
                    self.watch();
                }
                self.read_extras(cx);
                let picked = self
                    .runs
                    .open
                    .as_ref()
                    .and_then(|opened| {
                        let view = opened.view.as_ref()?.as_ref().ok()?;
                        let job = opened.job.as_ref()?;
                        view.jobs.iter().find(|listed| &listed.job.id == job)
                    })
                    .map(|listed| listed.job.clone());
                if let Some(job) = picked {
                    self.read_annotations(&job, cx);
                }
            }
            logs::KEY => {
                let Some(opened) = &mut self.runs.open else {
                    return;
                };
                let job = opened
                    .view
                    .as_ref()
                    .and_then(|view| view.as_ref().ok())
                    .and_then(|view| {
                        view.jobs
                            .iter()
                            .find(|listed| Some(&listed.job.id) == opened.job.as_ref())
                    })
                    .map(|listed| listed.job.clone());
                if let (Some(log), Some(job)) = (&mut opened.log, job) {
                    log.take(value, &job);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn watch(&self) {
        if !self.signed_in() {
            return;
        }
        if self.area == Area::Actions {
            self.watch_runs();
            let shown = self
                .runs
                .open
                .as_ref()
                .filter(|opened| opened.tab == runs::Tab::Logs)
                .and_then(|opened| Some((&opened.id, opened.job.as_ref()?)));
            if let Some((run, job)) = shown {
                let params = json!({ "project": self.project, "runId": run, "jobId": job });
                self.ask_watch(logs::KEY, "codeHostJobLog", params);
            }
            return;
        }
        if self.area == Area::Issues {
            self.watch_issues();
            return;
        }
        if self.area == Area::Releases {
            self.ask_watch(
                releases::KEY,
                "codeHostReleases",
                json!({ "project": self.project }),
            );
            return;
        }
        if self.area != Area::Pulls {
            return;
        }
        match &self.pull {
            Some(opened) => {
                let mut params = self.located();
                params["number"] = json!(opened.number);
                self.ask_watch(pull::VIEW, "codeHostPull", params.clone());
                self.ask_watch(pull::FILES, "codeHostPullFiles", params);
            }
            None => self.ask_watch(
                pulls::KEY,
                "codeHostPulls",
                json!({ "project": self.project, "state": self.pulls.state.word() }),
            ),
        }
    }

    fn plugin_call(&self, method: &'static str, more: Value) -> Value {
        call_of(&self.host.host, self.repo(), &self.account, method, more)
    }

    /// The repository shown: the project's, or another an inbox item opened.
    fn repo(&self) -> &str {
        self.elsewhere.as_deref().unwrap_or(&self.host.repo)
    }

    /// Where the core finds what is shown: the project, or the repository named outright.
    fn located(&self) -> Value {
        match &self.elsewhere {
            Some(repo) => {
                let (owner, name) = repo.split_once('/').unwrap_or_default();
                json!({ "repo": { "provider": self.host.host, "owner": owner, "name": name, "account": self.account } })
            }
            None => json!({ "project": self.project }),
        }
    }

    /// Calls `method` of the host's plugin about the project's repository, with `more`.
    fn plugin(
        &self,
        method: &'static str,
        more: Value,
    ) -> impl Future<Output = Result<Value, SharedString>> + 'static {
        let away = self
            .out_of_reach
            .as_ref()
            .map(|host| SharedString::from(format!("{host} is out of reach")));
        let answered = away
            .is_none()
            .then(|| self.core.request("plugin", self.plugin_call(method, more)));
        async move {
            let Some(answered) = answered else {
                return Err(away.unwrap_or_default());
            };
            match answered.await {
                Ok(value) => answer(&value),
                Err(error) => Err(error.into()),
            }
        }
    }

    /// Downloads a file through `method` of the host's plugin, answering where it was saved.
    fn download(
        &self,
        method: &'static str,
        more: Value,
    ) -> impl Future<Output = Result<String, SharedString>> + 'static {
        let mut streaming = self
            .core
            .stream("pluginStream", self.plugin_call(method, more));
        async move {
            let mut saved = None;
            while let Some(part) = streaming.parts.recv().await {
                if let Some(error) = part.error {
                    let message = error["message"].as_str().map(str::to_owned);
                    return Err(message.unwrap_or_else(|| error.to_string()).into());
                }
                saved = part
                    .items
                    .iter()
                    .find_map(|item| item["saved"]["path"].as_str().map(str::to_owned))
                    .or(saved);
                if part.done {
                    break;
                }
            }
            saved.ok_or_else(|| "the download ended without saving anything".into())
        }
    }

    /// Tells the person how something went, in a toast.
    fn say(&self, said: Said, cx: &mut Context<Self>) {
        let note = match said {
            Said::Done(text) => toast::success(text),
            Said::Failed(text) => toast::error(text).autohide(false),
        };
        self.bench
            .update(cx, |bench, cx| bench.notify(note, cx))
            .ok();
    }

    /// Shows `area`, here and in the rail.
    fn set_area(&mut self, area: Area, cx: &mut Context<Self>) {
        self.area = area;
        self.bench
            .update(cx, |bench, cx| {
                bench.area = area;
                bench.parts.redraw(&bench.parts.rail, cx);
                cx.notify();
            })
            .ok();
    }

    fn choose(&mut self, end: compose::End, cx: &mut Context<Self>) {
        self.picking = Some(end);
        cx.notify();
    }

    fn ask(&mut self, asking: Asking, cx: &mut Context<Self>) {
        self.asking = Some(asking);
        cx.notify();
    }

    fn ask_watch(&self, key: &'static str, method: &'static str, params: Value) {
        self.core
            .watch(key, Watch::new(method, params, &["codeHost"]));
    }
}

impl Render for HostArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.area {
            _ if shell::needs_sign_in(self) => shell::sign_in(self, window, cx),
            Area::Pulls if self.composing.is_some() => compose::view(self, window, cx),
            Area::Pulls if self.pull.is_some() => pull::view(self, window, cx),
            Area::Pulls => div()
                .id("host-list")
                .size_full()
                .overflow_y_scroll()
                .child(pulls::list(self, cx))
                .into_any_element(),
            Area::Releases => releases::view(self, window, cx),
            Area::Issues => issues::view(self, window, cx),
            Area::Actions if self.runs.open.is_some() => runs::view(self, window, cx),
            Area::Actions => div()
                .id("host-runs")
                .size_full()
                .overflow_y_scroll()
                .child(runs::list(self, cx))
                .into_any_element(),
            area => notice(
                format!("{} is not here yet", area.label(&self.host)),
                None,
                cx,
            ),
        };
        let asking = self.asking.as_ref().map(|asking| {
            let (yes, no) = (cx.entity().downgrade(), cx.entity().downgrade());
            let then = asking.then.clone();
            ui::Confirm::new(
                asking.title.clone(),
                asking.body.clone(),
                asking.yes.clone(),
            )
            .marked()
            .destructive(asking.destructive)
            .on_yes(move |_, cx| {
                let then = then.clone();
                yes.update(cx, |area, cx| {
                    area.asking = None;
                    then(area, cx);
                    cx.notify();
                })
                .ok();
            })
            .on_no(move |_, cx| {
                no.update(cx, |area, cx| {
                    area.asking = None;
                    cx.notify();
                })
                .ok();
            })
        });
        let workflows = self.runs.picking.then(|| {
            let no = cx.entity().downgrade();
            ui::Choices::new(
                "Which workflow's runs to show",
                runs::workflow_choices(self, cx),
            )
            .on_no(move |_, cx| {
                no.update(cx, |area, cx| {
                    area.runs.picking = false;
                    cx.notify();
                })
                .ok();
            })
        });
        let more = self.runs.more.clone().map(|run| {
            let no = cx.entity().downgrade();
            ui::Choices::new("Run", run_actions::more_choices(self, &run, cx)).on_no(
                move |_, cx| {
                    no.update(cx, |area, cx| {
                        area.runs.more = None;
                        cx.notify();
                    })
                    .ok();
                },
            )
        });
        let dispatch = run_actions::dispatch_sheet(self, cx);
        let menu = self.account_menu(cx);
        let note = shell::budget_note(self, cx);
        let away = self
            .out_of_reach
            .clone()
            .map(|host| away_note(&host, cx).mx(px(12.)).mt(px(10.)));
        self.fetch_pictures(cx);
        let picking = self.picking.map(|end| {
            let title = match end {
                compose::End::Head => "The branch with the changes",
                compose::End::Base => "The branch the changes land in",
            };
            let no = cx.entity().downgrade();
            ui::Choices::new(title, compose::choices(self, end, cx)).on_no(move |_, cx| {
                no.update(cx, |area, cx| {
                    area.picking = None;
                    cx.notify();
                })
                .ok();
            })
        });
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .children(away)
            .children(note)
            .child(div().flex_1().min_h_0().child(body))
            .children(picking)
            .children(menu)
            .children(workflows)
            .children(more)
            .children(dispatch)
            .children(asking)
    }
}

/// The `plugin` call of `method` of `host`'s plugin about `repo`, as `account`, with `more`.
fn call_of(
    host: &str,
    repo: &str,
    account: &Option<String>,
    method: &'static str,
    more: Value,
) -> Value {
    let (owner, name) = repo.split_once('/').unwrap_or_default();
    let mut params = json!({
        "provider": host,
        "owner": owner,
        "name": name,
        "account": account,
    });
    if let (Some(params), Value::Object(more)) = (params.as_object_mut(), more) {
        params.extend(more);
    }
    json!({ "plugin": format!("sikemux.{host}"), "method": method, "params": params })
}

/// Says that `host` is out of reach and what shows is what it last said.
pub fn away_note(host: &str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .px(px(12.))
        .py(px(8.))
        .rounded(crate::theme::RADIUS_2)
        .border_1()
        .border_color(t.warn.opacity(0.4))
        .bg(t.warn.opacity(0.08))
        .text_size(crate::theme::ui_text(12.))
        .text_color(t.text_primary)
        .child(format!(
            "{host} is out of reach. Showing what it last said; changes wait until it is back."
        ))
}

/// The value inside a code-host answer, or what its failure said.
fn answer<T: DeserializeOwned>(value: &Value) -> Result<T, SharedString> {
    if let Some(failed) = value.get("failed") {
        return Err(failed["message"]
            .as_str()
            .unwrap_or("the code host did not answer")
            .to_owned()
            .into());
    }
    serde_json::from_value(value["value"].clone()).map_err(|error| error.to_string().into())
}

/// What a list says while it has nothing to show: still loading, empty, or failed.
fn notice(title: impl Into<SharedString>, detail: Option<SharedString>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .size_full()
        .min_h(px(240.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(24.))
        .child(ui::text(title, 12.5, FontWeight::MEDIUM).text_color(t.text_secondary))
        .children(detail.map(|detail| ui::meta(detail, cx).text_center()))
        .into_any_element()
}

/// A list's head: its area's name with its icon, then what it holds.
fn list_head(area: Area, host: &ProjectCodeHost, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .pt(px(12.))
        .pb(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .mr(px(8.))
                .child(ui::icon(area.icon(), 14.).text_color(t.text_tertiary))
                .child(
                    ui::text(area.label(host), 12., FontWeight::MEDIUM).text_color(t.text_primary),
                ),
        )
}

/// A pull request's state as a mark in its colour: green while open, red once closed,
/// purple once merged, grey as a draft.
fn pull_mark(state: PullState, size: f32, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let (glyph, ink, label) = match state {
        PullState::Open => (Icon::PullRequest, t.live, "Open"),
        PullState::Draft => (Icon::PullRequest, t.text_tertiary, "Draft"),
        PullState::Closed => (Icon::PullRequest, t.danger, "Closed"),
        PullState::Merged => (Icon::Merge, t.accent, "Merged"),
    };
    div()
        .id(SharedString::from(format!("pull-mark-{label}")))
        .flex_none()
        .tooltip(ui::tooltip(label))
        .child(ui::icon(glyph, size).text_color(ink))
        .into_any_element()
}

fn outcome_word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "running",
        Outcome::Queued => "queued",
        Outcome::Success => "success",
        Outcome::Failure => "failure",
        Outcome::Cancelled => "cancelled",
        Outcome::Skipped => "skipped",
        Outcome::Blocked => "blocked",
        Outcome::Unknown => "unknown",
    }
}

/// How many of a commit's checks stand each way, as one line.
fn tally_summary(tally: &CheckTally) -> String {
    if tally.passed == tally.total {
        return "all passed".into();
    }
    [
        (tally.failing, "failing"),
        (tally.running + tally.queued, "running"),
        (tally.waiting, "waiting"),
        (tally.cancelled, "cancelled"),
        (tally.unknown, "unknown"),
        (tally.passed, "passed"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, word)| format!("{count} {word}"))
    .collect::<Vec<_>>()
    .join(", ")
}

pub(crate) fn host_label(host: &str) -> &str {
    match host {
        "github" => "GitHub",
        "gitlab" => "GitLab",
        "bitbucket" => "Bitbucket",
        other => other,
    }
}

fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "Running",
        Outcome::Queued => "Queued",
        Outcome::Success => "Passed",
        Outcome::Failure => "Failed",
        Outcome::Cancelled => "Cancelled",
        Outcome::Skipped => "Skipped",
        Outcome::Blocked => "Waiting for approval",
        Outcome::Unknown => "Unknown",
    }
}

/// What started a run, in words.
fn event_label(event: &str) -> String {
    match event {
        "push" => "Push".into(),
        "pull_request" | "pull_request_target" => "Pull request".into(),
        "workflow_dispatch" => "Manual".into(),
        "schedule" => "Scheduled".into(),
        "release" => "Release".into(),
        "workflow_run" => "Workflow run".into(),
        "merge_group" => "Merge queue".into(),
        other => {
            let words = other.replace('_', " ");
            let mut letters = words.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect())
                .unwrap_or_default()
        }
    }
}

/// How long something took from `started`, to `finished` or until now, in words.
fn elapsed(started: &str, finished: Option<&str>) -> String {
    let parse = |at: &str| chrono::DateTime::parse_from_rfc3339(at).ok();
    let Some(from) = parse(started) else {
        return "—".into();
    };
    let to = finished
        .and_then(parse)
        .map_or_else(chrono::Utc::now, |to| to.with_timezone(&chrono::Utc));
    duration((to.timestamp() - from.timestamp()).max(0) as u64)
}

/// A length of time in words, from its seconds.
fn duration(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 if seconds.is_multiple_of(60) => format!("{}m", seconds / 60),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ if (seconds / 60).is_multiple_of(60) => format!("{}h", seconds / 3600),
        _ => format!("{}h {}m", seconds / 3600, seconds / 60 % 60),
    }
}

/// A label as the host colours it.
fn label_chip(label: &Label, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let ink = u32::from_str_radix(&label.color, 16)
        .map(|rgb| Hsla::from(gpui_kit::rgb(rgb)))
        .unwrap_or(t.text_tertiary);
    div()
        .px(px(7.))
        .rounded_full()
        .border_1()
        .border_color(ink.opacity(0.5))
        .bg(ink.opacity(0.14))
        .text_color(t.text_primary)
        .child(label.name.clone())
}

/// When something happened, as how long ago, from the host's RFC 3339 time.
pub(crate) fn ago(at: &str) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(at) else {
        return String::new();
    };
    match crate::agents::age(at.timestamp_millis().max(0) as u64) {
        now if now == "now" => now,
        age => format!("{age} ago"),
    }
}

/// A branch's name in a quiet chip.
fn branch_chip(name: &str, ink: Hsla, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex_shrink(1.)
        .min_w_0()
        .max_w(px(220.))
        .px(px(6.))
        .py(px(1.))
        .rounded(crate::theme::RADIUS_1)
        .bg(t.surface_active)
        .font_family(crate::theme::MONO)
        .text_color(ink)
        .truncate()
        .child(name.to_owned())
}

/// A comment count with its icon, or nothing when there are none.
fn comments(count: u64, cx: &App) -> Option<Div> {
    let t = Tokens::get(cx);
    (count > 0).then(|| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(crate::theme::ui_text(11.))
            .text_color(t.text_tertiary)
            .font_features(crate::theme::tabular())
            .child(ui::icon(Icon::Message, 12.).text_color(t.text_tertiary))
            .child(count.to_string())
    })
}

/// Whether `key` is one this module watches, so the workbench hands its answers here.
pub fn watches(key: &str) -> bool {
    key.starts_with("host-")
}

/// The project's branch, and whether the core says it needs a pull request.
pub fn branch_of(project: &sikemux_wire::protocol::ProjectInfo) -> Option<(String, bool)> {
    project
        .git
        .as_ref()
        .map(|git| (git.branch.clone(), git.needs_pull))
}
