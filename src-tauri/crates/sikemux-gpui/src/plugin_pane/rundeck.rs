//! Rundeck's pane: the projects and their folders beside a folder's jobs, a job with its runs,
//! a project's activity, the form that starts a job with its deploy plan, and a run as it
//! goes, step by step with its output.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, AppContext, ClickEvent, Context, Div, Entity,
    FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, UniformListScrollHandle, Window,
};
use serde_json::{json, Value};
use sikemux_wire::rundeck::{
    ActivityPage, DeployForm, DeployTarget, EnvTone, ExecutionPage, GroupNode, JobCatalog,
    JobOption, JobPage, JobRow, Jobs, LiveRun, OptionChoice, OptionKind, OutputLine, OutputTick,
    Plan, Run, RunCheck, RunNotice, RunStep, Section, StatusKind, StepState, TermColor, Tone, Tree,
};

use super::{after, listen, Failure, Host, Panel, PluginPane, Streamed};
use crate::toast;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui::{self, Button, IconButton, Segmented, Toggle, Variant};

const PITCH: &str = "Your jobs and deploys, beside the code.";
const TREE_WIDTH: f32 = 240.;
const ACTIVITY_PAGE: u32 = 50;
const ACTIVITY_MOST: u32 = 200;
/// The most output lines a run's page keeps; older ones are counted, not shown.
const LOG_LINES_KEPT: usize = 10_000;
const LOG_ROW: f32 = 18.;

/// A job as the plugin's views address it.
#[derive(Clone, PartialEq)]
struct JobRef {
    project: String,
    job_id: String,
    name: String,
    group: Option<String>,
}

/// What the main area shows, the last of the stack in front.
#[derive(Clone, PartialEq)]
enum Level {
    Activity {
        project: String,
    },
    Job(JobRef),
    Deploy {
        job: JobRef,
        branch: Option<String>,
        given: BTreeMap<String, String>,
    },
    Execution {
        job: JobRef,
        id: u64,
    },
}

#[derive(Clone, Copy, Default, PartialEq)]
enum JobsShown {
    #[default]
    All,
    Running,
    Failed,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum RunsShown {
    #[default]
    All,
    Running,
    Failed,
    Mine,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum LinesShown {
    #[default]
    All,
    NoDebug,
    Problems,
}

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.rundeck",
    surface: "deploy",
    pane: super::boxed::<Rundeck>,
    top_bar: Some(super::boxed::<DeployChip>),
    palettes: PALETTES,
};

#[derive(Default)]
pub struct Rundeck {
    /// What the main area shows over the folder's jobs, newest last.
    stack: Vec<Level>,
    project: String,
    group: Option<String>,
    /// The tree's open projects and folders, as `project` and `project/path`.
    open: HashSet<String>,
    jobs_shown: JobsShown,
    compare: bool,
    filter: Option<Entity<InputState>>,
    runs_shown: RunsShown,
    activity_max: u32,
    form: Option<Form>,
    run: Option<Output>,
    /// Runs the person asked to hear about when they end.
    watching: HashMap<u64, Task<()>>,
    sign_in: Option<SignIn>,
    /// The run an abort was asked for, until Rundeck answers.
    aborting: Option<u64>,
    typing: Vec<Subscription>,
}

/// The run form being filled in, for the job and branch it was opened with.
struct Form {
    of: Level,
    options: Vec<JobOption>,
    values: BTreeMap<String, String>,
    inputs: BTreeMap<String, Entity<InputState>>,
    /// Each option's values read from its values URL; none where reading failed.
    remote: BTreeMap<String, Option<Vec<String>>>,
    check: RunCheck,
    checking: Option<Task<()>>,
    advanced: bool,
    debug: bool,
    node_filter: Entity<InputState>,
    run_at: Entity<InputState>,
    as_user: Entity<InputState>,
    attempted: bool,
    busy: bool,
    error: Option<String>,
}

/// A run's output as it streams in.
struct Output {
    id: u64,
    lines: Rc<Vec<OutputLine>>,
    dropped: usize,
    offset: Option<String>,
    completed: bool,
    failed: bool,
    error: Option<String>,
    step: Option<String>,
    shown: LinesShown,
    follow: bool,
    scroll: UniformListScrollHandle,
    _stream: Option<Task<()>>,
}

struct SignIn {
    with_token: bool,
    url: Entity<InputState>,
    token: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    insecure: bool,
    busy: bool,
}

impl Panel for Rundeck {
    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        if host.signed_in(cx) == Some(false) {
            return self.sign_in(host, window, cx);
        }
        let tree = host.view("treeView", json!({}), cx).read::<Tree>();
        if let Err(failure) = &tree {
            if failure.signed_out() {
                return self.sign_in(host, window, cx);
            }
        }
        if self.project.is_empty() {
            if let Ok(Some(tree)) = &tree {
                if let Some(first) = tree.projects.first() {
                    self.project = first.name.clone();
                    self.open.insert(first.name.clone());
                }
            }
        }
        let jobs = (!self.project.is_empty()).then(|| {
            host.view(
                "jobsView",
                json!({ "project": self.project, "group": self.group }),
                cx,
            )
            .read::<Jobs>()
        });
        let live: HashSet<String> = match &jobs {
            Some(Ok(Some(jobs))) => jobs.live_paths.iter().cloned().collect(),
            _ => HashSet::new(),
        };
        let side = self.tree(host, tree, &live, cx);
        let main = match self.stack.last().cloned() {
            None => self.jobs(jobs, window, cx),
            Some(Level::Activity { project }) => self.activity(host, &project, cx),
            Some(Level::Job(job)) => self.job(host, &job, cx),
            Some(level @ Level::Deploy { .. }) => self.deploy(host, level, window, cx),
            Some(Level::Execution { job, id }) => self.execution(host, &job, id, cx),
        };
        div()
            .size_full()
            .flex()
            .child(side)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(main),
            )
            .into_any_element()
    }

    fn open_at(&mut self, at: &str, _: &mut Host, _: &mut Context<PluginPane>) {
        let Some((job, deploy)) = place_of(at) else {
            return;
        };
        self.stack.clear();
        self.open.remove(&job.project);
        self.pick(&job.project, job.group.clone());
        self.form = None;
        self.stack = vec![match deploy {
            Some(branch) => Level::Deploy {
                job,
                branch,
                given: BTreeMap::new(),
            },
            None => Level::Job(job),
        }];
    }
}

/// Where a palette row or the top bar opens the pane: a job, or its run form with a branch.
fn place_key(job: &JobRef, deploy: Option<Option<&str>>) -> String {
    json!({
        "project": job.project,
        "jobId": job.job_id,
        "name": job.name,
        "group": job.group,
        "deploy": deploy.map(|branch| json!({ "branch": branch })),
    })
    .to_string()
}

#[allow(clippy::type_complexity)]
fn place_of(key: &str) -> Option<(JobRef, Option<Option<String>>)> {
    let place: Value = serde_json::from_str(key).ok()?;
    let job = JobRef {
        project: place["project"].as_str()?.to_owned(),
        job_id: place["jobId"].as_str()?.to_owned(),
        name: place["name"].as_str()?.to_owned(),
        group: place["group"].as_str().map(str::to_owned),
    };
    let deploy = place["deploy"].as_object().map(|deploy| {
        deploy
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    Some((job, deploy))
}

/// The palettes Rundeck offers: every job in every project, to open one.
pub const PALETTES: &[super::PluginPalette] = &[super::PluginPalette {
    plugin: "sikemux.rundeck",
    id: "rundeck.jobs",
    title: "Find a Rundeck job",
    placeholder: "search Rundeck jobs…",
    view: "catalogView",
    params: None,
    rows: job_rows,
    typed: None,
}];

fn job_rows(catalog: &Value) -> Vec<crate::palette::Row> {
    let Ok(catalog) = serde_json::from_value::<JobCatalog>(catalog.clone()) else {
        return Vec::new();
    };
    catalog
        .jobs
        .into_iter()
        .map(|place| {
            let job = JobRef {
                project: place.project.clone(),
                job_id: place.job_id.clone(),
                name: place.name.clone(),
                group: place.group.clone(),
            };
            let folder = place.group.clone().unwrap_or_default();
            crate::palette::Row::new(place_key(&job, None), place.name.clone())
                .meta(match folder.is_empty() {
                    true => place.project.clone(),
                    false => format!("{} · {folder}", place.project),
                })
                .also(&place.place)
        })
        .collect()
}

/// The top bar's word on where the project in front deploys, and a menu to pick the place,
/// deploy there or check out what runs there.
#[derive(Default)]
pub struct DeployChip {
    /// The pointer has been over the top bar, so asking the deploy server is worth it.
    looked: bool,
    /// The job picked from the menu, for each project.
    picked: HashMap<String, (String, String)>,
    /// A checkout of the deployed branch is under way.
    checking_out: bool,
}

impl Panel for DeployChip {
    fn render(
        &mut self,
        host: &mut Host,
        _: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let nothing = || div().into_any_element();
        if host.signed_in(cx) != Some(true) {
            return nothing();
        }
        let Some(project) = host.project(cx) else {
            return nothing();
        };
        self.looked |= host.hovered(cx);
        if !self.looked {
            return nothing();
        }
        let folder = project
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        let picked = self
            .picked
            .get(&project)
            .map(|(project, job_id)| json!({ "project": project, "jobId": job_id }));
        let target = host
            .view(
                "deployTargetView",
                json!({ "folder": folder, "picked": picked }),
                cx,
            )
            .read::<DeployTarget>();
        let Ok(Some(target)) = target else {
            return nothing();
        };
        let Some(active) = target.active.clone() else {
            return nothing();
        };
        let t = Tokens::get(cx).clone();
        let dot = |tone: EnvTone| match tone {
            EnvTone::Prod => t.danger,
            EnvTone::Staging => t.warn,
            EnvTone::Preprod => t.cmd,
            EnvTone::Dev => t.live,
            EnvTone::Other => t.text_tertiary,
        };
        let pane = cx.weak_entity();
        let menu_target = target.clone();
        let repo = project.clone();
        let read = repo_read(host, cx);
        let checkout = read.clone().flatten();
        let checking_out = self.checking_out;
        KitButton::new("rundeck-deploy-chip")
            .ghost()
            .compact()
            .h(px(24.))
            .px(px(6.))
            .gap(px(6.))
            .rounded(theme::RADIUS_1)
            .tooltip(target.summary.clone())
            .child(ui::icon(Icon::Rundeck, 12.).text_color(t.text_tertiary))
            .child(div().size(px(6.)).rounded_full().bg(dot(active.tone)))
            .child(
                ui::text(active.place.clone(), 11.5, FontWeight::NORMAL)
                    .text_color(t.text_primary)
                    .max_w(px(260.))
                    .truncate(),
            )
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |popup, _, _| {
                let target = &menu_target;
                let mut popup = popup.min_w(px(280.));
                if target.jobs.len() > 1 {
                    popup = popup.label("Deploy to");
                    for job in &target.jobs {
                        let (pane, project, picked) = (
                            pane.clone(),
                            repo.clone(),
                            (job.project.clone(), job.job_id.clone()),
                        );
                        popup = popup.item(
                            PopupMenuItem::new(job.place.clone())
                                .checked(
                                    Some(&job.job_id)
                                        == target.active.as_ref().map(|active| &active.job_id),
                                )
                                .on_click(move |_, _, cx| {
                                    let _ = pane.update(cx, |pane, cx| {
                                        if let Some((chip, _)) = pane.parts::<DeployChip>() {
                                            chip.picked.insert(project.clone(), picked.clone());
                                        }
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    popup = popup.separator();
                }
                let Some(active) = &target.active else {
                    return popup;
                };
                let job = JobRef {
                    project: active.project.clone(),
                    job_id: active.job_id.clone(),
                    name: active.name.clone(),
                    group: active.group.clone(),
                };
                let deploy_label = match (target.runs_branch, &read) {
                    (false, _) => format!("Run on {}", active.place),
                    (true, None) => "Deploy (reading the branch…)".to_owned(),
                    (true, Some(None)) => "Deploy (detached HEAD)".to_owned(),
                    (true, Some(Some(branch))) => format!("Deploy {branch}"),
                };
                let reading = target.runs_branch && read.is_none();
                let at = place_key(
                    &job,
                    Some(checkout.as_deref().filter(|_| target.runs_branch)),
                );
                let opener = pane.clone();
                popup = popup.item(
                    PopupMenuItem::new(deploy_label)
                        .icon(Icon::Rundeck)
                        .disabled(reading)
                        .on_click(move |_, _, cx| {
                            let _ = opener
                                .update(cx, |pane, cx| pane.host.open_pane_at(at.clone(), cx));
                        }),
                );
                if let Some(deployed) = target.deployed_branch.clone() {
                    let (pane, repo) = (pane.clone(), repo.clone());
                    let here = checkout.as_deref() == Some(deployed.as_str());
                    let label = match checking_out {
                        true => format!("Checking out… {deployed}"),
                        false => format!("Check out {deployed}"),
                    };
                    popup = popup.item(
                        PopupMenuItem::new(label)
                            .icon(Icon::Checkout)
                            .disabled(here || checking_out)
                            .on_click(move |_, _, cx| {
                                let _ = pane.update(cx, |pane, cx| {
                                    if let Some((chip, _)) = pane.parts::<DeployChip>() {
                                        chip.checking_out = true;
                                    }
                                    let checked = pane.host.core(
                                        "git.checkoutSmart",
                                        json!({ "repo": repo, "branch": deployed }),
                                    );
                                    super::after::<DeployChip, _>(
                                        checked,
                                        |chip, host, done, cx| {
                                            chip.checking_out = false;
                                            if let Err(failure) = done {
                                                host.said(&failure.titled("Check out"), cx);
                                            }
                                        },
                                        cx,
                                    );
                                });
                            }),
                    );
                }
                if let Some(last) = &target.last_run {
                    popup = popup.separator().label(last.text.clone());
                }
                popup
            })
            .into_any_element()
    }
}

/// The branch checked out in the project in front, as the git pane last read it: none until it
/// has, and a read without a branch while HEAD is detached.
fn repo_read(host: &Host, cx: &App) -> Option<Option<String>> {
    let bench = host.bench().upgrade()?;
    let overview = bench.read(cx).read("git-overview")?;
    Some(
        overview["status"]["branch"]
            .as_str()
            .filter(|branch| !branch.is_empty())
            .map(str::to_owned),
    )
}

// ---- the projects' tree ------------------------------------------------------

impl Rundeck {
    fn tree(
        &self,
        host: &mut Host,
        tree: Result<Option<Rc<Tree>>, Failure>,
        live: &HashSet<String>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let body = match tree {
            Ok(None) => super::loading(cx),
            Err(failure) => super::failure(&failure, cx),
            Ok(Some(tree)) if tree.projects.is_empty() => {
                super::empty("This sign-in sees no projects.", cx)
            }
            Ok(Some(tree)) => {
                let mut rows = Vec::new();
                for project in &tree.projects {
                    let open = self.open.contains(&project.name);
                    let chosen = self.stack.is_empty()
                        && self.project == project.name
                        && self.group.is_none();
                    let lit = self.project == project.name && live.contains("");
                    let name = project.name.clone();
                    rows.push(
                        tree_row(
                            ("rundeck-project", rows.len()),
                            &project.name,
                            0,
                            (project.total > 0).then_some(open),
                            chosen,
                            lit,
                            project.error.is_none().then_some(project.total),
                            cx,
                        )
                        .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                            rd.pick(&name, None);
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                    if !open {
                        continue;
                    }
                    let project_name = project.name.clone();
                    let in_activity = matches!(
                        self.stack.last(),
                        Some(Level::Activity { project }) if *project == project_name
                    );
                    rows.push(
                        tree_row(
                            ("rundeck-activity", rows.len()),
                            "Activity",
                            1,
                            None,
                            in_activity,
                            false,
                            None,
                            cx,
                        )
                        .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                            rd.stack = vec![Level::Activity {
                                project: project_name.clone(),
                            }];
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                    if let Some(error) = &project.error {
                        rows.push(
                            div()
                                .pl(px(28.))
                                .py(px(4.))
                                .child(ui::meta(error.clone(), cx).text_color(t.danger))
                                .into_any_element(),
                        );
                    }
                    self.folders(&project.name, &project.groups, 1, live, &mut rows, cx);
                }
                div()
                    .id("rundeck-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(6.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .children(rows)
                    .into_any_element()
            }
        };
        div()
            .w(px(TREE_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.border)
            .child(
                div()
                    .px(px(12.))
                    .pt(px(12.))
                    .pb(px(4.))
                    .child(ui::caps("Projects", cx)),
            )
            .child(body)
            .child(connection(host, cx))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn folders(
        &self,
        project: &str,
        nodes: &[GroupNode],
        depth: usize,
        live: &HashSet<String>,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<PluginPane>,
    ) {
        for node in nodes {
            let key = format!("{project}/{}", node.path);
            let open = self.open.contains(&key);
            let chosen = self.stack.is_empty()
                && self.project == project
                && self.group.as_deref() == Some(node.path.as_str());
            let lit = self.project == project && live.contains(&node.path);
            let (project_name, path) = (project.to_owned(), node.path.clone());
            rows.push(
                tree_row(
                    ("rundeck-folder", rows.len()),
                    &node.name,
                    depth,
                    (!node.children.is_empty()).then_some(open),
                    chosen,
                    lit,
                    Some(node.count),
                    cx,
                )
                .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                    rd.pick(&project_name, Some(path.clone()));
                    cx.notify();
                }))
                .into_any_element(),
            );
            if open {
                self.folders(project, &node.children, depth + 1, live, rows, cx);
            }
        }
    }

    /// Shows a project's or a folder's jobs, opening it and the folders above it in the tree.
    fn pick(&mut self, project: &str, group: Option<String>) {
        let same = self.stack.is_empty() && self.project == project && self.group == group;
        let key = match &group {
            Some(path) => format!("{project}/{path}"),
            None => project.to_owned(),
        };
        if same && self.open.contains(&key) {
            self.open.remove(&key);
        } else {
            self.open.insert(project.to_owned());
            if let Some(path) = &group {
                let mut above = String::new();
                for segment in path.split('/') {
                    if !above.is_empty() {
                        above.push('/');
                    }
                    above.push_str(segment);
                    self.open.insert(format!("{project}/{above}"));
                }
            }
        }
        self.project = project.to_owned();
        self.group = group;
        self.stack.clear();
    }

    fn push(&mut self, level: Level) {
        self.stack.push(level);
    }
}

#[allow(clippy::too_many_arguments)]
fn tree_row(
    id: (&'static str, usize),
    name: &str,
    depth: usize,
    open: Option<bool>,
    chosen: bool,
    live: bool,
    count: Option<u32>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx).clone();
    let hover = t.surface_hover;
    let chevron = match open {
        Some(open) => ui::icon(Icon::Chevron, 10.)
            .text_color(t.text_tertiary)
            .when(open, |icon| {
                icon.with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(
                    std::f32::consts::FRAC_PI_2,
                )))
            })
            .into_any_element(),
        None => div().size(px(10.)).into_any_element(),
    };
    let glyph = if name == "Activity" && depth == 1 {
        Icon::Activity
    } else {
        Icon::Folder
    };
    div()
        .id(id)
        .pl(px(6. + depth as f32 * 14.))
        .pr(px(8.))
        .py(px(4.))
        .rounded(theme::RADIUS_2)
        .flex()
        .items_center()
        .gap(px(6.))
        .cursor_pointer()
        .when(chosen, |row| row.bg(t.surface_chosen))
        .when(!chosen, |row| row.hover(move |row| row.bg(hover)))
        .child(chevron)
        .child(ui::icon(glyph, 13.).text_color(t.text_tertiary))
        .child(
            ui::text(name.to_owned(), 12.5, FontWeight::NORMAL)
                .text_color(t.text_primary)
                .flex_1()
                .min_w_0()
                .truncate(),
        )
        .when(live, |row| {
            row.child(div().size(px(6.)).rounded_full().bg(t.warn))
        })
        .children(count.map(|count| ui::meta(count.to_string(), cx)))
}

/// Where the pane is signed in, and signing out.
fn connection(host: &mut Host, cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let status = match host.status(cx).read::<Value>() {
        Ok(Some(status)) => status,
        _ => Rc::new(Value::Null),
    };
    let place = status["url"]
        .as_str()
        .map(|url| {
            url.trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/')
                .to_owned()
        })
        .unwrap_or_default();
    let user = status["user"].as_str().unwrap_or_default().to_owned();
    let ok = status["ok"].as_bool() == Some(true);
    div()
        .border_t_1()
        .border_color(t.border)
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(ui::dot(if ok { ui::Tone::Ok } else { ui::Tone::Idle }, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    ui::text(place, 12., FontWeight::MEDIUM)
                        .text_color(t.text_primary)
                        .truncate(),
                )
                .child(ui::meta(user, cx)),
        )
        .child(
            IconButton::new("rundeck-sign-out", Icon::LogOut, "Sign out")
                .small()
                .on_click(listen::<Rundeck, ClickEvent>(cx, |_, host, _, _, cx| {
                    let out = host.call("logout", Value::Null);
                    after::<Rundeck, _>(
                        out,
                        |_, host, done, cx| {
                            if let Err(failure) = done {
                                host.said(&failure.titled("Sign out of Rundeck"), cx);
                            }
                        },
                        cx,
                    );
                })),
        )
        .into_any_element()
}

// ---- a folder's jobs ---------------------------------------------------------

impl Rundeck {
    fn jobs(
        &mut self,
        jobs: Option<Result<Option<Rc<Jobs>>, Failure>>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let Some(jobs) = jobs else {
            return super::empty("Pick a Rundeck project from the list on the left.", cx);
        };
        let jobs = match jobs {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
            Ok(Some(jobs)) => jobs,
        };
        let t = Tokens::get(cx).clone();
        let filter = self.filter_input(window, cx);
        let query = filter.read(cx).value().trim().to_lowercase();
        let compare = self.compare && !jobs.compare.is_empty();
        let by_id: HashMap<&str, &JobRow> = jobs
            .jobs
            .iter()
            .map(|job| (job.job_id.as_str(), job))
            .collect();
        let shown = |job: &JobRow| {
            (query.is_empty() || job.name.to_lowercase().contains(&query))
                && match self.jobs_shown {
                    JobsShown::All => true,
                    JobsShown::Running => job.latest.as_ref().is_some_and(|run| run.live),
                    JobsShown::Failed => job
                        .latest
                        .as_ref()
                        .is_some_and(|run| run.status_kind == StatusKind::Failed),
                }
        };
        let sections: Vec<(&Section, Vec<&JobRow>)> =
            (if compare { &jobs.compare } else { &jobs.list })
                .iter()
                .map(|section| {
                    let rows = section
                        .jobs
                        .iter()
                        .filter_map(|id| by_id.get(id.as_str()).copied())
                        .filter(|job| shown(job))
                        .collect::<Vec<_>>();
                    (section, rows)
                })
                .filter(|(_, rows)| !rows.is_empty())
                .collect();
        let total = jobs.jobs.len() as u64;
        let picker = if compare {
            ui::meta(
                format!(
                    "{} {} in more than one folder",
                    jobs.shared,
                    if jobs.shared == 1 { "job" } else { "jobs" }
                ),
                cx,
            )
            .into_any_element()
        } else {
            let chosen = [JobsShown::All, JobsShown::Running, JobsShown::Failed]
                .iter()
                .position(|shown| *shown == self.jobs_shown)
                .unwrap_or_default();
            Segmented::words(
                "rundeck-jobs-shown",
                vec![
                    ("All".into(), Some(total)),
                    ("Running".into(), Some(jobs.running as u64)),
                    ("Failed".into(), Some(jobs.failed as u64)),
                ],
                chosen,
            )
            .on_select(listen::<Rundeck, usize>(cx, |rd, _, index, _, cx| {
                rd.jobs_shown = [JobsShown::All, JobsShown::Running, JobsShown::Failed][*index];
                cx.notify();
            }))
            .into_any_element()
        };
        let layout = (!jobs.compare.is_empty()).then(|| {
            Segmented::words(
                "rundeck-layout",
                vec![("List".into(), None), ("Compare folders".into(), None)],
                usize::from(compare),
            )
            .on_select(listen::<Rundeck, usize>(cx, |rd, _, index, _, cx| {
                rd.compare = *index == 1;
                cx.notify();
            }))
        });
        let head = header(
            crumbs(&jobs.project, jobs.group.as_deref(), cx),
            jobs.title.clone(),
            Some(picker),
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .children(layout)
                .child(
                    div()
                        .w(px(200.))
                        .child(ui::Field::new(&filter).icon(Icon::Search)),
                )
                .child(refresh("rundeck-jobs-refresh", "jobsView", cx))
                .into_any_element(),
            cx,
        );
        let mut banners = Vec::new();
        if let Some(error) = &jobs.error {
            banners.push(banner(error.clone(), t.warn, cx));
        }
        if jobs.partial {
            let timed_out = match jobs.timed_out {
                0 => String::new(),
                1 => ": 1 job timed out".into(),
                count => format!(": {count} jobs timed out"),
            };
            banners.push(banner(
                format!("Rundeck was slow to answer{timed_out}. Those rows show no recent runs; refresh to try again."),
                t.warn,
                cx,
            ));
        }
        let body = if jobs.jobs.is_empty() {
            super::empty(
                match &jobs.group {
                    Some(group) => format!("No jobs in {group}."),
                    None => format!("No jobs in {}.", jobs.project),
                },
                cx,
            )
        } else if sections.is_empty() {
            super::empty("No jobs match.", cx)
        } else if compare {
            compare_grid(&sections, jobs.loose, cx)
        } else {
            let many = sections.len() > 1;
            let mut rows = Vec::new();
            for (section, jobs_in) in &sections {
                if many || section.segment.is_some() {
                    let (project, path) = (self.project.clone(), section.path.clone());
                    let link = section.segment.is_some();
                    rows.push(
                        div()
                            .id(SharedString::from(format!(
                                "rundeck-section-{}",
                                section.path
                            )))
                            .px(px(12.))
                            .pt(px(12.))
                            .pb(px(4.))
                            .when(link, |head| {
                                head.cursor_pointer()
                                    .on_click(listen::<Rundeck, ClickEvent>(
                                        cx,
                                        move |rd, _, _, _, cx| {
                                            rd.pick(&project, Some(path.clone()));
                                            cx.notify();
                                        },
                                    ))
                            })
                            .child(
                                ui::caps(&section.label, cx)
                                    .when(section.prod, |caps| caps.text_color(t.warn)),
                            )
                            .into_any_element(),
                    );
                }
                for job in jobs_in {
                    rows.push(self.job_row(job, &jobs.project, cx));
                }
            }
            div()
                .id("rundeck-jobs")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(6.))
                .pb(px(12.))
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .children(banners)
            .child(body)
            .into_any_element()
    }

    fn filter_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> Entity<InputState> {
        if let Some(filter) = &self.filter {
            return filter.clone();
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter jobs"));
        self.typing
            .push(cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()));
        self.filter = Some(filter.clone());
        filter
    }

    fn job_row(&self, job: &JobRow, project: &str, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let hover = t.surface_hover;
        let job_ref = JobRef {
            project: project.to_owned(),
            job_id: job.job_id.clone(),
            name: job.name.clone(),
            group: job.group.clone(),
        };
        let (open, deploy) = (job_ref.clone(), job_ref);
        let branch = job
            .deployed
            .as_ref()
            .and_then(|run| run.branch.clone())
            .or_else(|| job.latest.as_ref().and_then(|run| run.branch.clone()));
        let when = job
            .latest
            .as_ref()
            .and_then(|run| run.ended_at.as_deref().or(run.started_at.as_deref()))
            .map(crate::codehost::ago)
            .unwrap_or_else(|| "—".into());
        let took_for = job
            .latest
            .as_ref()
            .and_then(|run| run_secs(run.started_at.as_deref(), run.ended_at.as_deref()))
            .map(took)
            .unwrap_or_else(|| "—".into());
        let tags = [
            job.scheduled.then(|| tag("scheduled", t.text_tertiary, cx)),
            (!job.enabled).then(|| tag("disabled", t.text_disabled, cx)),
            job.error.as_ref().map(|error| {
                tag(
                    if error == "timed out" {
                        "timed out"
                    } else {
                        "error"
                    },
                    t.warn,
                    cx,
                )
            }),
        ];
        div()
            .id(SharedString::from(format!("rundeck-job-{}", job.job_id)))
            .px(px(8.))
            .py(px(6.))
            .rounded(theme::RADIUS_2)
            .flex()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .when(!job.enabled, |row| row.opacity(0.6))
            .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                rd.push(Level::Job(open.clone()));
                cx.notify();
            }))
            .child(status_dot(
                job.latest.as_ref().map(|run| run.status_kind),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                ui::text(job.name.clone(), 12.5, FontWeight::MEDIUM)
                                    .text_color(t.text_primary)
                                    .min_w_0()
                                    .truncate(),
                            )
                            .children(tags.into_iter().flatten()),
                    )
                    .child(
                        ui::meta(
                            match &job.latest {
                                Some(run) => format!(
                                    "{} · {when} · took {took_for}",
                                    run.user.clone().unwrap_or_else(|| "—".into())
                                ),
                                None => "Never run".into(),
                            },
                            cx,
                        )
                        .truncate(),
                    ),
            )
            .child(
                div()
                    .w(px(150.))
                    .flex_none()
                    .overflow_hidden()
                    .child(branch_chip(job.branch.as_deref(), cx)),
            )
            .child(
                div()
                    .w(px(84.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .when(job.enabled, |slot| {
                        slot.child(
                            Button::new(
                                SharedString::from(format!("rundeck-start-{}", job.job_id)),
                                job.run_word.clone(),
                                Variant::Outline,
                            )
                            .icon(Icon::Run)
                            .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                                rd.push(Level::Deploy {
                                    job: deploy.clone(),
                                    branch: branch.clone(),
                                    given: BTreeMap::new(),
                                });
                                cx.stop_propagation();
                                cx.notify();
                            })),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// The same jobs across the folders right below this one, side by side.
fn compare_grid(
    sections: &[(&Section, Vec<&JobRow>)],
    loose: u32,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let mut names: Vec<&str> = sections
        .iter()
        .flat_map(|(_, jobs)| jobs.iter().map(|job| job.name.as_str()))
        .collect();
    names.sort_unstable();
    names.dedup();
    let column = |section: &Section| {
        div().flex_1().min_w_0().px(px(8.)).child(
            ui::caps(section.segment.as_deref().unwrap_or_default(), cx)
                .when(section.prod, |caps| caps.text_color(t.warn)),
        )
    };
    let head = div()
        .flex()
        .px(px(12.))
        .py(px(8.))
        .child(div().w(px(160.)).flex_none().child(ui::caps("Job", cx)))
        .children(sections.iter().map(|(section, _)| column(section)));
    let rows = names.into_iter().map(|name| {
        div()
            .flex()
            .px(px(12.))
            .py(px(6.))
            .border_t_1()
            .border_color(t.line_soft)
            .child(
                ui::text(name.to_owned(), 12.5, FontWeight::MEDIUM)
                    .text_color(t.text_primary)
                    .w(px(160.))
                    .flex_none()
                    .truncate(),
            )
            .children(sections.iter().map(|(_, jobs)| {
                let cell = div()
                    .flex_1()
                    .min_w_0()
                    .px(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(2.));
                match jobs.iter().find(|job| job.name == name) {
                    None => cell,
                    Some(job) => cell
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(status_dot(
                                    job.latest.as_ref().map(|run| run.status_kind),
                                    cx,
                                ))
                                .child(branch_chip(job.branch.as_deref(), cx)),
                        )
                        .children(job.latest.as_ref().map(|run| {
                            let when = run
                                .ended_at
                                .as_deref()
                                .or(run.started_at.as_deref())
                                .map(crate::codehost::ago)
                                .unwrap_or_default();
                            ui::meta(
                                format!("{} · {when}", run.user.clone().unwrap_or_default()),
                                cx,
                            )
                        }))
                        .children(job.live_branch.as_ref().map(|live| {
                            ui::meta(format!("live is {live}"), cx).text_color(t.warn)
                        })),
                }
            }))
    });
    div()
        .id("rundeck-compare")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .child(head)
        .children(rows)
        .when(loose > 0, |grid| {
            grid.child(div().px(px(12.)).py(px(10.)).child(ui::meta(
                format!(
                    "{loose} {} directly in this folder; switch to List to see {}.",
                    if loose == 1 { "job sits" } else { "jobs sit" },
                    if loose == 1 { "it" } else { "them" }
                ),
                cx,
            )))
        })
        .into_any_element()
}

// ---- a job and a project's runs ----------------------------------------------

impl Rundeck {
    fn job(&mut self, host: &mut Host, job: &JobRef, cx: &mut Context<PluginPane>) -> AnyElement {
        let page = host
            .view(
                "jobView",
                json!({ "jobId": job.job_id, "project": job.project }),
                cx,
            )
            .read::<JobPage>();
        let page = match page {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure.titled("Couldn't load runs"), cx),
            Ok(Some(page)) => page,
        };
        let t = Tokens::get(cx).clone();
        let repeat = page.last_good.as_ref().map(|good| {
            (
                good.branch.clone(),
                page.repeat_options.clone(),
                good.execution_id,
            )
        });
        let (form_job, again_job) = (job.clone(), job.clone());
        let again_label = if page.runs_branch {
            match repeat.as_ref().and_then(|(branch, _, _)| branch.clone()) {
                Some(branch) => format!("Redeploy {branch}"),
                None => "Redeploy".into(),
            }
        } else {
            "Run again".into()
        };
        let tools = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .children(page.permalink.clone().map(|url| {
                IconButton::new("rundeck-job-open", Icon::Link, "Open in Rundeck")
                    .on_click(move |_, _, cx| cx.open_url(&url))
            }))
            .child(
                Button::new("rundeck-again", again_label, Variant::Outline)
                    .icon(Icon::Fetch)
                    .disabled(repeat.is_none() || !page.execution_enabled)
                    .tooltip(match &repeat {
                        Some((_, _, id)) => format!("Prefill from #{id}"),
                        None => format!(
                            "No successful {} to repeat yet",
                            page.run_word.to_lowercase()
                        ),
                    })
                    .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                        if let Some((branch, given, _)) = &repeat {
                            rd.push(Level::Deploy {
                                job: again_job.clone(),
                                branch: branch.clone(),
                                given: given.clone(),
                            });
                            cx.notify();
                        }
                    })),
            )
            .child(
                Button::new("rundeck-start", page.run_word.clone(), Variant::Primary)
                    .icon(Icon::Run)
                    .disabled(!page.execution_enabled)
                    .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                        rd.push(Level::Deploy {
                            job: form_job.clone(),
                            branch: None,
                            given: BTreeMap::new(),
                        });
                        cx.notify();
                    })),
            )
            .child(refresh("rundeck-job-refresh", "jobView", cx))
            .into_any_element();
        let head = header(
            self.back(&job.project, job.group.as_deref(), cx),
            page.name.clone(),
            Some(folder_chip(
                &page.project,
                page.group.as_deref(),
                page.prod,
                cx,
            )),
            tools,
            cx,
        );
        let fact = |label: &'static str, value: String, under: Option<String>| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(ui::caps(label, cx))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(4.))
                        .child(ui::text(value, 13., FontWeight::MEDIUM).text_color(t.text_primary))
                        .children(under.map(|under| ui::meta(under, cx))),
                )
        };
        let good = page.last_good.as_ref();
        let mut facts = div().flex().gap(px(28.)).flex_wrap();
        if page.runs_branch {
            facts = facts.child(fact(
                "Live",
                good.and_then(|good| good.branch.clone())
                    .unwrap_or_else(|| "—".into()),
                None,
            ));
        }
        facts = facts
            .child(fact(
                if page.runs_branch {
                    "Deployed"
                } else {
                    "Last success"
                },
                good.and_then(|good| good.ended_at.as_deref())
                    .map(crate::codehost::ago)
                    .unwrap_or_else(|| "—".into()),
                good.and_then(|good| good.user.clone())
                    .map(|user| format!("by {user}")),
            ))
            .child(fact(
                "Success",
                match page.finished {
                    0 => "—".into(),
                    finished => format!("{}%", page.succeeded * 100 / finished),
                },
                (page.finished > 0).then(|| format!("of {}", page.finished)),
            ))
            .child(fact(
                "Usual time",
                page.usual_secs
                    .map(|secs| took(secs as f64))
                    .unwrap_or_else(|| "—".into()),
                None,
            ))
            .children(
                page.fastest_secs
                    .zip(page.slowest_secs)
                    .map(|(fast, slow)| {
                        fact(
                            "Fastest · slowest",
                            format!("{} · {}", took(fast as f64), took(slow as f64)),
                            None,
                        )
                    }),
            );
        let tags = [
            (!page.execution_enabled).then(|| tag("executions disabled", t.warn, cx)),
            page.node_filter
                .as_ref()
                .map(|nodes| tag(&format!("nodes: {nodes}"), t.text_tertiary, cx)),
        ];
        let live = page.live.as_ref().map(|run| {
            let open = job.clone();
            let id = run.execution_id;
            live_card(run, page.usual_secs, cx).on_click(listen::<Rundeck, ClickEvent>(
                cx,
                move |rd, _, _, _, cx| {
                    rd.push(Level::Execution {
                        job: open.clone(),
                        id,
                    });
                    cx.notify();
                },
            ))
        });
        let history = if page.runs.is_empty() {
            super::empty(format!("{} hasn't run yet.", page.name), cx)
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(card(
                    if page.runs_branch {
                        "Deploy times"
                    } else {
                        "Run times"
                    },
                    format!("last {} runs", page.runs.len()),
                    times(&page.runs, page.usual_secs, job, cx),
                    cx,
                ))
                .child(card(
                    "History",
                    page.runs.len().to_string(),
                    history(&page.runs, false, job, cx),
                    cx,
                ))
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("rundeck-job")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(20.))
                    .py(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .children(page.description.clone().map(|description| {
                        ui::text(description, 12.5, FontWeight::NORMAL)
                            .text_color(t.text_secondary)
                            .whitespace_normal()
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .children(tags.into_iter().flatten()),
                    )
                    .child(facts)
                    .children(live)
                    .child(history),
            )
            .into_any_element()
    }

    fn activity(
        &mut self,
        host: &mut Host,
        project: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let max = self.activity_max.max(ACTIVITY_PAGE);
        let page = host
            .view(
                "activityView",
                json!({ "project": project, "max": max }),
                cx,
            )
            .read::<ActivityPage>();
        let page = match page {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure.titled("Couldn't load runs"), cx),
            Ok(Some(page)) => page,
        };
        let mut words = vec![
            ("All".into(), Some(page.runs.len() as u64)),
            ("Running".into(), Some(page.running as u64)),
            ("Failed".into(), Some(page.failed as u64)),
        ];
        let mut kinds = vec![RunsShown::All, RunsShown::Running, RunsShown::Failed];
        if page.me.is_some() {
            words.push(("Mine".into(), Some(page.mine as u64)));
            kinds.push(RunsShown::Mine);
        }
        let chosen = kinds
            .iter()
            .position(|kind| *kind == self.runs_shown)
            .unwrap_or_default();
        let picker = Segmented::words("rundeck-runs-shown", words, chosen)
            .on_select(listen::<Rundeck, usize>(cx, move |rd, _, index, _, cx| {
                rd.runs_shown = kinds.get(*index).copied().unwrap_or_default();
                cx.notify();
            }))
            .into_any_element();
        let shown: Vec<Run> = page
            .runs
            .iter()
            .filter(|run| match self.runs_shown {
                RunsShown::All => true,
                RunsShown::Running => run.live,
                RunsShown::Failed => run.status_kind == StatusKind::Failed,
                RunsShown::Mine => page.me.is_some() && run.user == page.me,
            })
            .cloned()
            .collect();
        let head = header(
            self.back(project, None, cx),
            "Activity",
            Some(picker),
            refresh("rundeck-activity-refresh", "activityView", cx),
            cx,
        );
        let more = page
            .total
            .is_some_and(|total| (page.runs.len() as u64) < total && max < ACTIVITY_MOST);
        let job = JobRef {
            project: project.to_owned(),
            job_id: String::new(),
            name: String::new(),
            group: None,
        };
        let body = if shown.is_empty() {
            super::empty(
                if page.runs.is_empty() {
                    format!("Nothing has run in {project} yet.")
                } else {
                    "No runs match this filter.".into()
                },
                cx,
            )
        } else {
            div()
                .id("rundeck-activity")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(20.))
                .py(px(14.))
                .child(history(&shown, true, &job, cx))
                .when(more, |list| {
                    list.child(div().pt(px(10.)).flex().justify_center().child(
                        Button::new("rundeck-older", "Show older runs", Variant::Outline).on_click(
                            listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                                rd.activity_max = (max + ACTIVITY_PAGE).min(ACTIVITY_MOST);
                                cx.notify();
                            }),
                        ),
                    ))
                })
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(body)
            .into_any_element()
    }

    /// A way back to the folder's jobs, or up the stack.
    fn back(&self, project: &str, group: Option<&str>, cx: &mut Context<PluginPane>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                IconButton::new("rundeck-back", Icon::Chevron, "Back")
                    .small()
                    .turned(std::f32::consts::PI)
                    .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, _, _, _, cx| {
                        rd.stack.pop();
                        rd.form = None;
                        cx.notify();
                    })),
            )
            .child(crumbs(project, group, cx))
            .into_any_element()
    }
}

/// Runs by day, newest first: each run's state, branch, who, when and how long against usual.
fn history(runs: &[Run], with_job: bool, job: &JobRef, cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let hover = t.surface_hover;
    let mut days: Vec<(String, Vec<&Run>)> = Vec::new();
    for run in runs {
        let day = run
            .started_at
            .as_deref()
            .map(day_of)
            .unwrap_or_else(|| "Unknown date".into());
        match days.last_mut() {
            Some((last, runs)) if *last == day => runs.push(run),
            _ => days.push((day, vec![run])),
        }
    }
    div()
        .flex()
        .flex_col()
        .children(days.into_iter().map(|(day, runs)| {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .px(px(12.))
                        .pt(px(10.))
                        .pb(px(4.))
                        .child(ui::caps(&day, cx))
                        .child(ui::meta(runs.len().to_string(), cx)),
                )
                .children(runs.into_iter().map(|run| {
                    let secs =
                        run_secs(run.started_at.as_deref(), run.ended_at.as_deref()).unwrap_or(0.);
                    let usual = run.usual_secs.map(|usual| usual as f64);
                    let scale = (usual.unwrap_or(secs) * 1.7).max(secs).max(1.);
                    let open = JobRef {
                        project: job.project.clone(),
                        job_id: run.job_id.clone().unwrap_or_else(|| job.job_id.clone()),
                        name: run.job_name.clone().unwrap_or_else(|| job.name.clone()),
                        group: if with_job {
                            run.job_group.clone()
                        } else {
                            job.group.clone()
                        },
                    };
                    let id = run.execution_id;
                    let clickable = !open.job_id.is_empty();
                    div()
                        .id(SharedString::from(format!("rundeck-run-{id}")))
                        .px(px(12.))
                        .py(px(5.))
                        .rounded(theme::RADIUS_2)
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when(clickable, |row| {
                            row.cursor_pointer()
                                .hover(move |row| row.bg(hover))
                                .on_click(listen::<Rundeck, ClickEvent>(
                                    cx,
                                    move |rd, _, _, _, cx| {
                                        rd.push(Level::Execution {
                                            job: open.clone(),
                                            id,
                                        });
                                        cx.notify();
                                    },
                                ))
                        })
                        .child(status_dot(Some(run.status_kind), cx))
                        .child(ui::mono(format!("#{id}"), 11.5, cx).w(px(56.)).flex_none())
                        .when(with_job, |row| {
                            row.child(
                                ui::text(
                                    run.job_name.clone().unwrap_or_else(|| "—".into()),
                                    12.5,
                                    FontWeight::MEDIUM,
                                )
                                .text_color(t.text_primary)
                                .w(px(160.))
                                .flex_shrink(1.)
                                .min_w(px(60.))
                                .truncate(),
                            )
                        })
                        .child(
                            div()
                                .w(px(130.))
                                .flex_shrink(1.)
                                .min_w(px(40.))
                                .overflow_hidden()
                                .child(branch_chip(run.branch.as_deref(), cx)),
                        )
                        .child(
                            ui::meta(run.user.clone().unwrap_or_else(|| "—".into()), cx)
                                .w(px(84.))
                                .flex_shrink(1.)
                                .min_w(px(30.))
                                .truncate(),
                        )
                        .child(
                            ui::meta(
                                run.started_at.as_deref().map(clock_of).unwrap_or_default(),
                                cx,
                            )
                            .w(px(40.))
                            .flex_none(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(48.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(t.line_soft)
                                .relative()
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .h_full()
                                        .rounded_full()
                                        .bg(status_ink(run.status_kind, &t))
                                        .w(gpui_kit::relative(((secs / scale).min(1.)) as f32)),
                                )
                                .children(usual.map(|usual| {
                                    div()
                                        .absolute()
                                        .top(px(-3.))
                                        .left(gpui_kit::relative((usual / scale) as f32))
                                        .w(px(1.5))
                                        .h(px(10.))
                                        .bg(t.text_tertiary)
                                })),
                        )
                        .child(
                            ui::meta(
                                if run.live {
                                    "running".into()
                                } else {
                                    took(secs)
                                },
                                cx,
                            )
                            .when(run.live, |meta| meta.text_color(t.warn))
                            .w(px(52.))
                            .flex_none()
                            .flex()
                            .justify_end(),
                        )
                }))
        }))
        .into_any_element()
}

/// One bar per run, oldest on the left, against the usual time.
fn times(
    runs: &[Run],
    usual: Option<u64>,
    job: &JobRef,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    const SLOTS: usize = 30;
    const HEIGHT: f32 = 96.;
    let t = Tokens::get(cx).clone();
    let shown: Vec<&Run> = runs.iter().take(SLOTS).rev().collect();
    let secs: Vec<f64> = shown
        .iter()
        .map(|run| run_secs(run.started_at.as_deref(), run.ended_at.as_deref()).unwrap_or(0.))
        .collect();
    let longest = secs
        .iter()
        .copied()
        .fold(60_f64.max(usual.unwrap_or(0) as f64 * 1.2), f64::max);
    let bars = shown.iter().zip(&secs).map(|(run, secs)| {
        let id = run.execution_id;
        let open = job.clone();
        let label = format!(
            "#{id}{} · {} · {}",
            run.branch
                .as_deref()
                .map(|branch| format!(" · {branch}"))
                .unwrap_or_default(),
            took(*secs),
            run.user.clone().unwrap_or_default()
        );
        div()
            .id(SharedString::from(format!("rundeck-bar-{id}")))
            .flex_1()
            .h_full()
            .flex()
            .items_end()
            .justify_center()
            .cursor_pointer()
            .tooltip(ui::tooltip(label))
            .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                rd.push(Level::Execution {
                    job: open.clone(),
                    id,
                });
                cx.notify();
            }))
            .child(
                div()
                    .w(px(6.))
                    .rounded_t(px(2.))
                    .bg(status_ink(run.status_kind, &t))
                    .h(px((HEIGHT * (*secs / longest) as f32).max(3.))),
            )
    });
    let empty = (shown.len()..SLOTS).map(|_| div().flex_1());
    div()
        .relative()
        .h(px(HEIGHT))
        .flex()
        .gap(px(2.))
        .children(empty.map(IntoElement::into_any_element))
        .children(bars.map(IntoElement::into_any_element))
        .children(usual.map(|usual| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(HEIGHT * (usual as f64 / longest) as f32))
                .border_t_1()
                .border_color(t.text_tertiary)
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .top(px(-14.))
                        .child(ui::meta(format!("usual {}", took(usual as f64)), cx)),
                )
        }))
        .into_any_element()
}

/// The run of a job going on now, step by step; it opens the run.
fn live_card(
    run: &LiveRun,
    usual: Option<u64>,
    cx: &mut Context<PluginPane>,
) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx).clone();
    let elapsed = since(run.started_at.as_deref()).unwrap_or(0.);
    let step = (run.total > 0).then(|| {
        format!(
            "step {} of {} · ",
            (run.completed + 1).min(run.total),
            run.total
        )
    });
    div()
        .id("rundeck-live-run")
        .p(px(12.))
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .cursor_pointer()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(7.)).rounded_full().bg(t.warn))
                .child(
                    ui::text(
                        match &run.branch {
                            Some(branch) => format!("Deploying {branch}"),
                            None => "Running".into(),
                        },
                        12.5,
                        FontWeight::SEMIBOLD,
                    )
                    .text_color(t.text_primary),
                )
                .child(ui::meta(
                    format!(
                        "#{} · {}",
                        run.execution_id,
                        run.user.clone().unwrap_or_default()
                    ),
                    cx,
                ))
                .child(div().flex_1())
                .child(ui::meta(
                    format!(
                        "{}{}{}",
                        step.unwrap_or_default(),
                        took(elapsed),
                        usual
                            .map(|usual| format!(" of about {}", took(usual as f64)))
                            .unwrap_or_default()
                    ),
                    cx,
                )),
        )
        .when(!run.steps.is_empty(), |card| {
            card.child(
                div()
                    .flex()
                    .gap(px(2.))
                    .children(run.steps.iter().map(|step| {
                        div()
                            .flex_1()
                            .h(px(4.))
                            .rounded_full()
                            .bg(step_ink(step.state, &t))
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .children(run.steps.iter().map(|step| {
                        ui::meta(step.name.clone(), cx)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(step.state == StepState::Running, |meta| {
                                meta.text_color(t.text_primary)
                            })
                    })),
            )
        })
}

// ---- the run form ------------------------------------------------------------

impl Rundeck {
    fn deploy(
        &mut self,
        host: &mut Host,
        level: Level,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let Level::Deploy { job, branch, given } = &level else {
            return super::empty("", cx);
        };
        let read = host
            .view(
                "deployView",
                json!({
                    "jobId": job.job_id,
                    "project": job.project,
                    "name": job.name,
                    "group": job.group,
                    "branch": branch,
                    "given": given,
                }),
                cx,
            )
            .read::<DeployForm>();
        let form_data = match read {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure.titled("Couldn't load the job"), cx),
            Ok(Some(form)) => form,
        };
        if self.form.as_ref().is_none_or(|form| form.of != level) {
            self.form = Some(Form::new(level.clone(), &form_data, window, cx));
            self.recheck(host, cx);
        }
        let urls = self
            .form
            .as_ref()
            .map(|form| form.check.values_urls.clone())
            .unwrap_or_default();
        self.read_remote(host, urls, cx);
        let t = Tokens::get(cx).clone();
        let form = self.form.as_ref().expect("made just now");
        let target = form_data
            .branch_key
            .as_ref()
            .map(|key| {
                form.values
                    .get(key)
                    .map(|value| value.trim().to_owned())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let repo = repo_of(host, &form_data.name, cx);
        let plan = (form_data.runs_branch && !target.is_empty()).then(|| {
            host.view(
                "planView",
                json!({
                    "jobId": form_data.job_id,
                    "project": form_data.project,
                    "service": form_data.service,
                    "targetBranch": target,
                    "repoPath": repo,
                }),
                cx,
            )
            .read::<Plan>()
        });
        let plan_ready = !form_data.runs_branch
            || matches!(&plan, Some(Ok(Some(plan))) if plan.target_branch == target);
        let verb = match (form_data.runs_branch, target.is_empty()) {
            (true, false) => format!("Deploy {target}"),
            (true, true) => "Deploy".into(),
            (false, _) => "Run".into(),
        };
        let label = if form.busy {
            "Starting…".into()
        } else if form_data.prod {
            format!("{verb} to production")
        } else {
            verb
        };
        let run_word = form_data.run_word.to_lowercase();
        let errors = if form.attempted {
            form.check.errors.clone()
        } else {
            BTreeMap::new()
        };
        let plan_view = form_data
            .runs_branch
            .then(|| plan_card(plan.clone(), &target, cx));
        let mut picks = branch_picks(&form_data, plan.as_ref(), &target, cx);
        let options = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .when(form_data.options.is_empty(), |list| {
                list.child(ui::meta("This job takes no options.", cx))
            })
            .children(form_data.options.iter().map(|option| {
                let hint = match form_data.branch_key.as_deref() == Some(option.name.as_str()) {
                    true => picks.take(),
                    false => None,
                };
                self.option_field(
                    option,
                    errors.get(&option.name).cloned(),
                    hint,
                    &form_data,
                    cx,
                )
            }))
            .child(self.advanced(form_data.node_filter.as_deref(), cx));
        let banners = [
            form_data.options_error.as_ref().map(|error| {
                banner(
                    format!("Couldn't read job options: {error}. Only the branch is sent."),
                    t.text_tertiary,
                    cx,
                )
            }),
            (!form_data.execution_enabled).then(|| {
                banner(
                    "Executions are disabled for this job in Rundeck.".into(),
                    t.warn,
                    cx,
                )
            }),
            form.error.clone().map(|error| banner(error, t.danger, cx)),
        ];
        let start_data = form_data.clone();
        let can_start = !form.busy && form_data.execution_enabled && plan_ready;
        let head = header(
            self.back(&job.project, job.group.as_deref(), cx),
            format!(
                "{} {}",
                if form_data.runs_branch {
                    "Deploy"
                } else {
                    "Run"
                },
                job.name
            ),
            Some(folder_chip(
                &form_data.project,
                form_data.group.as_deref(),
                form_data.prod,
                cx,
            )),
            div().into_any_element(),
            cx,
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("rundeck-form")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(20.))
                    .py(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .children(banners.into_iter().flatten())
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(14.))
                            .items_start()
                            .child(div().flex_1().min_w(px(320.)).child(card(
                                "Options",
                                form_data.description.clone().unwrap_or_default(),
                                options.into_any_element(),
                                cx,
                            )))
                            .children(
                                plan_view.map(|plan| div().flex_1().min_w(px(320.)).child(plan)),
                            ),
                    )
                    .when(form_data.prod, |page| {
                        page.child(banner(
                            format!(
                                "This {run_word}s to production ({}). You'll be asked to confirm.",
                                form_data.env
                            ),
                            t.warn,
                            cx,
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .children(form_data.usual_secs.map(|usual| {
                                ui::meta(format!("Usually takes {}", took(usual as f64)), cx)
                            }))
                            .child(div().flex_1())
                            .child(
                                Button::new("rundeck-cancel", "Cancel", Variant::Outline)
                                    .disabled(form.busy)
                                    .on_click(listen::<Rundeck, ClickEvent>(
                                        cx,
                                        |rd, _, _, _, cx| {
                                            rd.stack.pop();
                                            rd.form = None;
                                            cx.notify();
                                        },
                                    )),
                            )
                            .child(
                                Button::new("rundeck-submit", label, Variant::Primary)
                                    .icon(Icon::Run)
                                    .disabled(!can_start)
                                    .on_click(listen::<Rundeck, ClickEvent>(
                                        cx,
                                        move |rd, host, _, _, cx| {
                                            rd.submit(host, &start_data, cx);
                                        },
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn option_field(
        &self,
        option: &JobOption,
        error: Option<String>,
        hint: Option<AnyElement>,
        form_data: &DeployForm,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let form = self.form.as_ref().expect("the form is open");
        let value = form.values.get(&option.name).cloned().unwrap_or_default();
        let remote_url = form.check.values_urls.get(&option.name).cloned();
        let label = option.label.clone().unwrap_or_else(|| option.name.clone());
        let choices: Option<Vec<OptionChoice>> = match (&option.values_url, &option.values) {
            (Some(_), _) => remote_url.as_ref().and_then(|_| {
                form.remote
                    .get(&option.name)
                    .cloned()
                    .flatten()
                    .map(|values| {
                        values
                            .into_iter()
                            .map(|value| OptionChoice {
                                name: value.clone(),
                                value,
                            })
                            .collect()
                    })
            }),
            (None, Some(values)) if !values.is_empty() => Some(
                values
                    .iter()
                    .map(|value| OptionChoice {
                        name: value.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            ),
            _ => None,
        };
        let name = option.name.clone();
        let input: AnyElement = if option.kind == OptionKind::File {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ui::meta("file options must be run from Rundeck", cx))
                .children(form_data.permalink.clone().map(|url| {
                    Button::new("rundeck-file-open", "Open in Rundeck", Variant::Outline)
                        .on_click(move |_, _, cx| cx.open_url(&url))
                }))
                .into_any_element()
        } else if option.enforced && option.multivalued && choices.is_some() {
            let picked: HashSet<String> = value
                .split(option.delimiter.as_str())
                .map(|part| part.trim().to_owned())
                .filter(|part| !part.is_empty())
                .collect();
            let choices = choices.clone().unwrap_or_default();
            let delimiter = option.delimiter.clone();
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(choices.iter().enumerate().map(|(at, choice)| {
                    let on = picked.contains(&choice.value);
                    let (name, value, all, delimiter, picked) = (
                        name.clone(),
                        choice.value.clone(),
                        choices
                            .iter()
                            .map(|choice| choice.value.clone())
                            .collect::<Vec<_>>(),
                        delimiter.clone(),
                        picked.clone(),
                    );
                    Toggle::new(
                        SharedString::from(format!("rundeck-multi-{name}-{at}")),
                        choice.name.clone(),
                        on,
                    )
                    .on_click(listen::<Rundeck, ClickEvent>(
                        cx,
                        move |rd, host, _, _, cx| {
                            let next: Vec<String> = all
                                .iter()
                                .filter(|candidate| {
                                    if **candidate == value {
                                        !on
                                    } else {
                                        picked.contains(*candidate)
                                    }
                                })
                                .cloned()
                                .collect();
                            rd.set_value(host, &name, next.join(&delimiter), cx);
                        },
                    ))
                }))
                .into_any_element()
        } else if option.enforced && choices.is_some() {
            let choices = choices.clone().unwrap_or_default();
            let known = choices.iter().any(|choice| choice.value == value);
            let mut picks = Vec::new();
            if !(option.required && known) {
                picks.push(ui::Pick::new("", "—"));
            }
            if !value.is_empty() && !known {
                picks.push(ui::Pick::new(value.clone(), value.clone()).hint("not allowed"));
            }
            picks.extend(choices.iter().map(|choice| {
                let pick = ui::Pick::new(choice.value.clone(), choice.name.clone());
                if choice.name != choice.value {
                    pick.hint(choice.value.clone())
                } else {
                    pick
                }
            }));
            ui::select(
                SharedString::from(format!("rundeck-option-{name}")),
                if value.is_empty() {
                    "—".to_owned()
                } else {
                    value.clone()
                },
                picks,
                Some(value.clone().into()),
                false,
                listen::<Rundeck, SharedString>(cx, move |rd, host, picked, _, cx| {
                    rd.set_value(host, &name, picked.to_string(), cx);
                }),
                cx,
            )
        } else {
            match form.inputs.get(&option.name) {
                Some(input) => ui::Field::new(input).into_any_element(),
                None => div().into_any_element(),
            }
        };
        let remote_failed = option.values_url.is_some()
            && form.remote.get(&option.name).is_some_and(Option::is_none);
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        ui::text(label.clone(), 12., FontWeight::MEDIUM).text_color(t.text_primary),
                    )
                    .when(option.required, |head| {
                        head.child(ui::text("*", 12., FontWeight::MEDIUM).text_color(t.danger))
                    })
                    .when(
                        option
                            .label
                            .as_deref()
                            .is_some_and(|label| label != option.name),
                        |head| head.child(ui::mono(option.name.clone(), 10.5, cx)),
                    ),
            )
            .child(input)
            .children(hint)
            .children(
                option
                    .description
                    .clone()
                    .map(|description| ui::meta(description, cx).whitespace_normal()),
            )
            .when(remote_failed, |field| {
                field.child(
                    ui::meta(
                        "Couldn't load the allowed values. Type a value instead.",
                        cx,
                    )
                    .text_color(t.warn),
                )
            })
            .children(error.map(|error| ui::meta(error, cx).text_color(t.danger)))
            .into_any_element()
    }

    fn advanced(&self, default_filter: Option<&str>, cx: &mut Context<PluginPane>) -> AnyElement {
        let form = self.form.as_ref().expect("the form is open");
        let toggle = Button::new("rundeck-advanced", "Advanced", Variant::Outline)
            .pressed(form.advanced)
            .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, _, _, _, cx| {
                if let Some(form) = &mut rd.form {
                    form.advanced = !form.advanced;
                }
                cx.notify();
            }));
        let labelled = |label: &'static str, field: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(ui::meta(label, cx))
                .child(field)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(toggle)
                    .when(!form.advanced, |row| {
                        row.child(ui::meta("debug output, nodes, schedule, run as", cx))
                    }),
            )
            .when(form.advanced, |page| {
                page.child(
                    Toggle::new("rundeck-debug", "Debug output", form.debug).on_click(listen::<
                        Rundeck,
                        ClickEvent,
                    >(
                        cx,
                        |rd, _, _, _, cx| {
                            if let Some(form) = &mut rd.form {
                                form.debug = !form.debug;
                            }
                            cx.notify();
                        },
                    )),
                )
                .child(labelled(
                    "Node filter",
                    ui::Field::new(&form.node_filter).mono().into_any_element(),
                ))
                .child(labelled(
                    "Run later (YYYY-MM-DD HH:MM, your time)",
                    ui::Field::new(&form.run_at).into_any_element(),
                ))
                .child(labelled(
                    "Run as user",
                    ui::Field::new(&form.as_user).into_any_element(),
                ))
                .children(
                    default_filter
                        .map(|filter| ui::meta(format!("The job's own nodes: {filter}"), cx)),
                )
            })
            .into_any_element()
    }

    fn set_value(
        &mut self,
        host: &mut Host,
        name: &str,
        value: String,
        cx: &mut Context<PluginPane>,
    ) {
        if let Some(form) = &mut self.form {
            form.values.insert(name.to_owned(), value);
        }
        self.recheck(host, cx);
        cx.notify();
    }

    /// Has the plugin check the form again, as it now stands.
    fn recheck(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let call = host.call(
            "checkRun",
            json!({ "options": form.options, "values": form.values, "remote": form.remote }),
        );
        form.checking = Some(cx.spawn(async move |pane, cx| {
            let checked = call.await;
            let _ = pane.update(cx, |pane, cx| {
                let Some((rd, _)) = pane.parts::<Rundeck>() else {
                    return;
                };
                let check = checked
                    .ok()
                    .and_then(|value| serde_json::from_value::<RunCheck>(value).ok());
                if let (Some(form), Some(check)) = (&mut rd.form, check) {
                    if check.errors.is_empty() && form.error.as_deref() == Some(FIX_OPTIONS) {
                        form.error = None;
                    }
                    form.check = check;
                }
                cx.notify();
            });
        }));
    }

    /// Reads each option's values from its URL, as the plugin filled the URL in.
    fn read_remote(
        &mut self,
        host: &mut Host,
        urls: BTreeMap<String, String>,
        cx: &mut Context<PluginPane>,
    ) {
        let mut moved = false;
        for (name, url) in urls {
            let read = host
                .view("optionValues", json!({ "url": url }), cx)
                .read::<Vec<OptionChoice>>();
            let values = match read {
                Ok(None) => continue,
                Ok(Some(choices)) => {
                    Some(choices.iter().map(|choice| choice.value.clone()).collect())
                }
                Err(_) => None,
            };
            if let Some(form) = &mut self.form {
                if form.remote.get(&name) != Some(&values) {
                    form.remote.insert(name, values);
                    moved = true;
                }
            }
        }
        if moved {
            self.recheck(host, cx);
        }
    }

    fn submit(
        &mut self,
        host: &mut Host,
        form_data: &Rc<DeployForm>,
        cx: &mut Context<PluginPane>,
    ) {
        let Some(form) = &mut self.form else {
            return;
        };
        form.attempted = true;
        form.error = None;
        let run_at = form.run_at.read(cx).value().trim().to_owned();
        let run_at_time = match run_at.is_empty() {
            true => None,
            false => match local_to_iso(&run_at) {
                Some(iso) => Some(iso),
                None => {
                    form.error = Some("Run later needs a full date and time.".into());
                    cx.notify();
                    return;
                }
            },
        };
        let filter = form.node_filter.read(cx).value().trim().to_owned();
        let as_user = form.as_user.read(cx).value().trim().to_owned();
        let own_nodes =
            filter.is_empty() || Some(filter.as_str()) == form_data.node_filter.as_deref();
        let filter = (!own_nodes).then_some(filter);
        let request = json!({
            "loglevel": if form.debug { Some("DEBUG") } else { None },
            "filter": filter,
            "runAtTime": run_at_time,
            "asUser": (!as_user.is_empty()).then_some(as_user),
        });
        form.busy = true;
        cx.notify();
        let ready = host.call(
            "checkRun",
            json!({
                "options": form_data.options,
                "values": form.values,
                "remote": form.remote,
                "jobId": form_data.job_id,
                "project": form_data.project,
            }),
        );
        let form_data = form_data.clone();
        after::<Rundeck, _>(
            ready,
            move |rd, host, ready, cx| {
                let ready = match ready.map(serde_json::from_value::<RunCheck>) {
                    Ok(Ok(ready)) => ready,
                    Ok(Err(error)) => return rd.failed(error.to_string()),
                    Err(failure) => return rd.failed(failure.message.to_string()),
                };
                if !ready.errors.is_empty() {
                    if let Some(form) = &mut rd.form {
                        form.check.errors = ready.errors.clone();
                    }
                    return rd.failed(FIX_OPTIONS.into());
                }
                rd.confirm_and_start(host, form_data, ready, request, cx);
            },
            cx,
        );
    }

    fn failed(&mut self, error: String) {
        if let Some(form) = &mut self.form {
            form.busy = false;
            form.error = Some(error);
        }
    }

    /// Asks before production and before a second run, pushes the checkout when the plan says,
    /// then starts the job and opens the run.
    fn confirm_and_start(
        &mut self,
        host: &mut Host,
        form_data: Rc<DeployForm>,
        ready: RunCheck,
        request: Value,
        cx: &mut Context<PluginPane>,
    ) {
        let target = form_data
            .branch_key
            .as_ref()
            .and_then(|key| ready.send.get(key).cloned())
            .unwrap_or_default();
        let run_word = form_data.run_word.to_lowercase();
        let mut questions = Vec::new();
        if form_data.prod {
            questions.push((
                if form_data.runs_branch {
                    format!("Deploy {target} to production?")
                } else {
                    format!("Run {} in production?", form_data.name)
                },
                format!(
                    "Project: {}\nJob: {}\nEnvironment: {}",
                    form_data.project, form_data.service, form_data.env
                ),
                if form_data.runs_branch {
                    "deploy to production"
                } else {
                    "run in production"
                }
                .to_owned(),
                true,
            ));
        }
        if ready.running == Some(true) {
            questions.push((
                "A run is already in progress".to_owned(),
                format!(
                    "{} is already running. Start another {run_word} anyway?",
                    form_data.name
                ),
                format!("{run_word} anyway"),
                false,
            ));
        }
        let repo = repo_of(host, &form_data.name, cx);
        let plan = form_data
            .runs_branch
            .then(|| {
                host.view(
                    "planView",
                    json!({
                        "jobId": form_data.job_id,
                        "project": form_data.project,
                        "service": form_data.service,
                        "targetBranch": target,
                        "repoPath": repo,
                    }),
                    cx,
                )
                .read::<Plan>()
                .ok()
                .flatten()
            })
            .flatten();
        let push = plan
            .filter(|plan| plan.pushes && plan.current_branch.as_deref() == Some(target.as_str()))
            .and(repo);
        let mut options = request;
        options["jobId"] = json!(form_data.job_id);
        options["options"] = json!(ready.send);
        self.ask_then_start(host, questions, push, options, form_data, target, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn ask_then_start(
        &mut self,
        host: &mut Host,
        mut questions: Vec<(String, String, String, bool)>,
        push: Option<String>,
        request: Value,
        form_data: Rc<DeployForm>,
        target: String,
        cx: &mut Context<PluginPane>,
    ) {
        if questions.is_empty() {
            return self.start(host, push, request, form_data, target, cx);
        }
        let (title, body, yes, destructive) = questions.remove(0);
        let Some(bench) = host.bench().upgrade() else {
            return;
        };
        let pane = cx.weak_entity();
        bench.update(cx, |bench, cx| {
            bench.confirm(
                title,
                body,
                yes,
                destructive,
                move |_, yes, _, cx| {
                    let (questions, push, request, form_data, target) = (
                        questions.clone(),
                        push.clone(),
                        request.clone(),
                        form_data.clone(),
                        target.clone(),
                    );
                    let _ = pane.update(cx, |pane, cx| {
                        if let Some((rd, host)) = pane.parts::<Rundeck>() {
                            if yes {
                                rd.ask_then_start(
                                    host, questions, push, request, form_data, target, cx,
                                );
                            } else if let Some(form) = &mut rd.form {
                                form.busy = false;
                            }
                            cx.notify();
                        }
                    });
                },
                cx,
            );
        });
    }

    fn start(
        &mut self,
        host: &mut Host,
        push: Option<String>,
        request: Value,
        form_data: Rc<DeployForm>,
        target: String,
        cx: &mut Context<PluginPane>,
    ) {
        let pushed = push
            .as_ref()
            .map(|repo| host.core("git.push", json!({ "repo": repo })));
        let run = {
            let started = host.call("run", request);
            async move {
                if let Some(pushed) = pushed {
                    pushed
                        .await
                        .map_err(|failure| failure.titled("Nothing was run: the push failed"))?;
                }
                started.await
            }
        };
        let pushing = push.is_some();
        after::<Rundeck, _>(
            run,
            move |rd, host, started, cx| {
                let id = match started {
                    Ok(result) => result["id"].as_u64(),
                    Err(failure) => {
                        let message = if pushing {
                            format!(
                                "{target} was pushed, but Rundeck rejected the run: {}",
                                failure.message
                            )
                        } else {
                            failure.message.to_string()
                        };
                        return rd.failed(message);
                    }
                };
                let Some(id) = id else {
                    return rd
                        .failed("Rundeck started the run but did not say which it is.".into());
                };
                let job = JobRef {
                    project: form_data.project.clone(),
                    job_id: form_data.job_id.clone(),
                    name: form_data.name.clone(),
                    group: form_data.group.clone(),
                };
                rd.form = None;
                rd.stack.pop();
                rd.push(Level::Execution {
                    job: job.clone(),
                    id,
                });
                rd.watch(host, &job, id, cx);
            },
            cx,
        );
    }
}

const FIX_OPTIONS: &str = "Fix the highlighted options first.";

impl Form {
    fn new(
        of: Level,
        data: &DeployForm,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> Self {
        let mut inputs = BTreeMap::new();
        for option in &data.options {
            if option.kind == OptionKind::File {
                continue;
            }
            let value = data.values.get(&option.name).cloned().unwrap_or_default();
            let placeholder = if option.is_date {
                option.date_format.clone().unwrap_or_else(|| "date".into())
            } else if option.multivalued {
                format!("values separated by {}", option.delimiter)
            } else {
                String::new()
            };
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(option.secure)
                    .default_value(value)
            });
            let name = option.name.clone();
            cx.subscribe(&input, move |pane, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let value = input.read(cx).value().to_string();
                if let Some((rd, host)) = pane.parts::<Rundeck>() {
                    rd.set_value(host, &name, value, cx);
                }
            })
            .detach();
            inputs.insert(option.name.clone(), input);
        }
        let mut field = |placeholder: &str, value: String, cx: &mut Context<PluginPane>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        Self {
            of,
            options: data.options.clone(),
            values: data.values.clone(),
            inputs,
            remote: BTreeMap::new(),
            check: RunCheck {
                errors: BTreeMap::new(),
                send: BTreeMap::new(),
                values_urls: BTreeMap::new(),
                running: None,
            },
            checking: None,
            advanced: false,
            debug: false,
            node_filter: field(
                data.node_filter.as_deref().unwrap_or("job's own nodes"),
                data.node_filter.clone().unwrap_or_default(),
                cx,
            ),
            run_at: field("2026-10-23 18:30", String::new(), cx),
            as_user: field("you", String::new(), cx),
            attempted: false,
            busy: false,
            error: None,
        }
    }
}

/// Branches worth one click: the checkout's, the live one, and whatever ran lately.
fn branch_picks(
    form: &DeployForm,
    plan: Option<&Result<Option<Rc<Plan>>, Failure>>,
    target: &str,
    cx: &mut Context<PluginPane>,
) -> Option<AnyElement> {
    let key = form.branch_key.clone()?;
    let plan = plan.and_then(|plan| plan.as_ref().ok().cloned().flatten());
    let mut picks: Vec<(String, Option<&'static str>)> = Vec::new();
    let mut add = |name: Option<String>, note: Option<&'static str>| {
        let Some(name) = name.filter(|name| !name.is_empty()) else {
            return;
        };
        match picks.iter_mut().find(|(known, _)| *known == name) {
            Some((_, known_note)) => {
                if known_note.is_none() {
                    *known_note = note;
                }
            }
            None => picks.push((name, note)),
        }
    };
    add(
        plan.as_ref().and_then(|plan| plan.current_branch.clone()),
        Some("checkout"),
    );
    add(
        plan.as_ref().and_then(|plan| plan.deployed_branch.clone()),
        Some("live"),
    );
    for branch in &form.recent_branches {
        add(Some(branch.clone()), None);
    }
    picks.truncate(6);
    if picks.is_empty() {
        return None;
    }
    Some(
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .children(picks.into_iter().enumerate().map(|(at, (name, note))| {
                let (key, picked) = (key.clone(), name.clone());
                Toggle::new(
                    SharedString::from(format!("rundeck-pick-{at}")),
                    match note {
                        Some(note) => format!("{name} · {note}"),
                        None => name.clone(),
                    },
                    name == target,
                )
                .on_click(listen::<Rundeck, ClickEvent>(
                    cx,
                    move |rd, host, _, window, cx| {
                        if let Some(input) = rd
                            .form
                            .as_ref()
                            .and_then(|form| form.inputs.get(&key))
                            .cloned()
                        {
                            let picked = picked.clone();
                            input.update(cx, |input, cx| input.set_value(picked, window, cx));
                        }
                        rd.set_value(host, &key, picked.clone(), cx);
                    },
                ))
            }))
            .into_any_element(),
    )
}

/// What deploying the branch would do, as the plugin worked it out.
fn plan_card(
    plan: Option<Result<Option<Rc<Plan>>, Failure>>,
    target: &str,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let body = match plan {
        None => ui::meta("Pick a branch to see what the deploy will do.", cx)
            .whitespace_normal()
            .into_any_element(),
        Some(Ok(None)) => ui::meta("Working out the plan…", cx).into_any_element(),
        Some(Err(failure)) => ui::meta(failure.message.to_string(), cx)
            .text_color(t.danger)
            .into_any_element(),
        Some(Ok(Some(plan))) if plan.target_branch != target => {
            ui::meta("Checking…", cx).into_any_element()
        }
        Some(Ok(Some(plan))) => div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(ui::meta("Live now", cx))
                            .child(match &plan.deployed_branch {
                                Some(branch) => branch_chip(Some(branch), cx),
                                None => ui::meta("Nothing yet", cx).into_any_element(),
                            }),
                    )
                    .child(ui::meta("→", cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(ui::meta("Will deploy", cx))
                            .child(branch_chip(Some(&plan.target_branch), cx)),
                    ),
            )
            .children(plan.checks.iter().map(|check| {
                let (icon, ink) = match (check.tone, check.pushes) {
                    (_, true) => (Icon::ArrowUp, t.warn),
                    (Tone::Ok, _) => (Icon::Check, t.live),
                    (Tone::Info, _) => (Icon::InfoCircle, t.text_tertiary),
                    (Tone::Warn, _) => (Icon::Warning, t.warn),
                    (Tone::Danger, _) => (Icon::Warning, t.danger),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(ui::icon(icon, 13.).text_color(ink))
                    .child(
                        ui::text(check.text.clone(), 12., FontWeight::NORMAL)
                            .text_color(t.text_secondary)
                            .flex_1()
                            .min_w_0()
                            .truncate(),
                    )
                    .children(check.value.clone().map(|value| ui::mono(value, 11., cx)))
            }))
            .children(
                plan.detail
                    .clone()
                    .map(|detail| ui::meta(detail, cx).whitespace_normal()),
            )
            .into_any_element(),
    };
    card("Plan", String::new(), body, cx)
}

/// The front project, when the job is named after its folder: the checkout a deploy pushes.
fn repo_of(host: &Host, job: &str, cx: &App) -> Option<String> {
    let bench = host.bench().upgrade()?;
    let project = bench.read(cx).watched_project()?.to_owned();
    let folder = project.trim_end_matches('/').rsplit('/').next()?;
    (folder == job).then_some(project)
}

// ---- a run -------------------------------------------------------------------

impl Rundeck {
    fn execution(
        &mut self,
        host: &mut Host,
        job: &JobRef,
        id: u64,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let page = host
            .view(
                "executionView",
                json!({ "executionId": id, "jobId": job.job_id, "project": job.project }),
                cx,
            )
            .read::<ExecutionPage>();
        if self.run.as_ref().is_none_or(|run| run.id != id) {
            self.run = Some(Output::new(id));
            self.follow_output(host, cx);
        }
        let page = match page {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
            Ok(Some(page)) => page,
        };
        let t = Tokens::get(cx).clone();
        let watching = self.watching.contains_key(&id);
        let (abort_job, watch_job, again_job) = (job.clone(), job.clone(), job.clone());
        let again = page
            .can_run_again
            .then(|| (page.branch.clone(), page.repeat_options.clone()));
        let status = match (page.live, page.total) {
            (true, total) if total > 0 => format!(
                "{} · step {} of {total}",
                capitalised(&page.status),
                (page.completed + 1).min(total)
            ),
            _ => capitalised(&page.status),
        };
        let tools =
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new(
                        "rundeck-run-again",
                        format!("{} again", page.run_word),
                        Variant::Outline,
                    )
                    .icon(Icon::Fetch)
                    .disabled(again.is_none())
                    .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                        if let Some((branch, given)) = &again {
                            rd.push(Level::Deploy {
                                job: again_job.clone(),
                                branch: branch.clone(),
                                given: given.clone(),
                            });
                            cx.notify();
                        }
                    })),
                )
                .when(page.live, |tools| {
                    tools
                        .child(
                            Button::new(
                                "rundeck-notify",
                                if watching { "Will notify" } else { "Notify me" },
                                Variant::Outline,
                            )
                            .icon(Icon::Clock)
                            .disabled(watching)
                            .on_click(listen::<Rundeck, ClickEvent>(
                                cx,
                                move |rd, host, _, _, cx| {
                                    rd.watch(host, &watch_job, id, cx);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            Button::new(
                                "rundeck-abort",
                                if self.aborting == Some(id) {
                                    "Aborting…"
                                } else {
                                    "Abort"
                                },
                                Variant::Outline,
                            )
                            .icon(Icon::Stop)
                            .disabled(self.aborting == Some(id))
                                .on_click(listen::<Rundeck, ClickEvent>(
                                    cx,
                                    move |_, host, _, _, cx| {
                                        abort(host, &abort_job, id, cx);
                                    },
                                )),
                        )
                })
                .children(page.permalink.clone().map(|url| {
                    IconButton::new("rundeck-run-open", Icon::Link, "Open in Rundeck")
                        .on_click(move |_, _, cx| cx.open_url(&url))
                }))
                .into_any_element();
        let head = header(
            self.back(&job.project, job.group.as_deref(), cx),
            format!("#{id}"),
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(folder_chip(
                        &job.project,
                        job.group.as_deref(),
                        page.prod,
                        cx,
                    ))
                    .child(status_dot(Some(page.status_kind), cx))
                    .child(
                        ui::text(status, 12., FontWeight::MEDIUM)
                            .text_color(status_ink(page.status_kind, &t)),
                    )
                    .into_any_element(),
            ),
            tools,
            cx,
        );
        let elapsed = run_secs(page.started_at.as_deref(), page.ended_at.as_deref());
        let left = match (page.live, page.usual_secs, elapsed) {
            (true, Some(usual), Some(elapsed)) => Some(usual as f64 - elapsed),
            _ => None,
        };
        let progress = (!page.steps.is_empty()).then(|| {
            div().flex().flex_col().gap(px(6.)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        ui::text(
                            page.percent
                                .map(|percent| format!("{percent}%"))
                                .unwrap_or_else(|| "—".into()),
                            20.,
                            FontWeight::SEMIBOLD,
                        )
                        .text_color(status_ink(page.status_kind, &t)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .gap(px(2.))
                            .children(page.steps.iter().map(|step| {
                                div()
                                    .flex_1()
                                    .h(px(5.))
                                    .rounded_full()
                                    .bg(step_ink(step.state, &t))
                            })),
                    )
                    .child(ui::meta(
                        format!(
                            "{} of {} steps{}",
                            page.completed,
                            page.total,
                            match left {
                                Some(left) if left > 0. => format!(" · about {} left", took(left)),
                                Some(_) => " · taking longer than usual".into(),
                                None => String::new(),
                            }
                        ),
                        cx,
                    )),
            )
        });
        let metas = div()
            .flex()
            .gap(px(8.))
            .flex_wrap()
            .children(
                page.branch
                    .clone()
                    .map(|branch| meta_chip(Icon::Git, branch, cx)),
            )
            .child(meta_chip(
                Icon::User,
                page.user.clone().unwrap_or_else(|| "—".into()),
                cx,
            ))
            .child(meta_chip(
                Icon::Clock,
                page.started_at
                    .as_deref()
                    .map(stamp_of)
                    .unwrap_or_else(|| "—".into()),
                cx,
            ))
            .child(meta_chip(
                Icon::Timer,
                elapsed.map(took).unwrap_or_else(|| "—".into()),
                cx,
            ));
        let steps = self.steps(&page.steps, cx);
        let output = self.output(&page, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .px(px(20.))
                    .py(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .border_b_1()
                    .border_color(t.border)
                    .children(progress)
                    .child(metas),
            )
            .child(div().flex_1().min_h_0().flex().child(steps).child(output))
            .into_any_element()
    }

    fn steps(&self, steps: &[RunStep], cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let run = self.run.as_ref().expect("the run's output is kept");
        let lines = run.lines.len();
        let all = ui::Row::new("rundeck-step-all", "All output")
            .mark(
                22.,
                ui::icon(Icon::LogLines, 13.).text_color(t.text_tertiary),
            )
            .chosen(run.step.is_none())
            .status(Some(ui::meta(lines.to_string(), cx)))
            .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, _, _, _, cx| {
                if let Some(run) = &mut rd.run {
                    run.step = None;
                }
                cx.notify();
            }));
        div()
            .id("rundeck-steps")
            .w(px(200.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(t.border)
            .px(px(6.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(div().px(px(8.)).pb(px(4.)).child(ui::caps("Steps", cx)))
            .child(all)
            .when(steps.is_empty(), |list| {
                list.child(div().px(px(8.)).child(ui::meta("Waiting for steps…", cx)))
            })
            .children(steps.iter().enumerate().map(|(at, step)| {
                let key = step.key.clone();
                let chosen = run.step.as_deref() == Some(step.key.as_str());
                let mark = match step.state {
                    StepState::Ok => ui::icon(Icon::Check, 11.)
                        .text_color(t.live)
                        .into_any_element(),
                    StepState::Fail => ui::icon(Icon::Close, 11.)
                        .text_color(t.danger)
                        .into_any_element(),
                    state => div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(step_ink(state, &t))
                        .into_any_element(),
                };
                let dur = run_secs(step.started_at.as_deref(), step.ended_at.as_deref())
                    .filter(|_| step.started_at.is_some())
                    .map(took)
                    .unwrap_or_default();
                ui::Row::new(("rundeck-step", at), step.name.clone())
                    .mark(22., mark)
                    .chosen(chosen)
                    .status(Some(ui::meta(dur, cx)))
                    .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                        if let Some(run) = &mut rd.run {
                            run.step = if run.step.as_deref() == Some(key.as_str()) {
                                None
                            } else {
                                Some(key.clone())
                            };
                        }
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn output(&self, page: &ExecutionPage, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let run = self.run.as_ref().expect("the run's output is kept");
        let shown: Rc<Vec<OutputLine>> = Rc::new(
            run.lines
                .iter()
                .filter(|line| {
                    run.step
                        .as_deref()
                        .is_none_or(|key| step_matches(line.step_ctx.as_deref(), key))
                        && match run.shown {
                            LinesShown::All => true,
                            LinesShown::NoDebug => {
                                !matches!(level_of(line).as_str(), "DEBUG" | "VERBOSE")
                            }
                            LinesShown::Problems => {
                                matches!(level_of(line).as_str(), "ERROR" | "WARN")
                            }
                        }
                })
                .cloned()
                .collect(),
        );
        let errors = run
            .lines
            .iter()
            .filter(|line| level_of(line) == "ERROR")
            .count();
        if run.follow && !shown.is_empty() {
            run.scroll
                .scroll_to_item(shown.len() - 1, ScrollStrategy::Bottom);
        }
        let chosen = [LinesShown::All, LinesShown::NoDebug, LinesShown::Problems]
            .iter()
            .position(|shown| *shown == run.shown)
            .unwrap_or_default();
        let step_label = run.step.as_ref().and_then(|key| {
            page.steps
                .iter()
                .position(|step| step.key == *key)
                .map(|at| format!("Step {} · {}", at + 1, page.steps[at].name))
        });
        let streaming = if run.completed || !page.live {
            "Done"
        } else {
            "Live"
        };
        let count = shown.len();
        let (ink, dim, warn, danger) = (t.text_secondary, t.text_tertiary, t.warn, t.danger);
        let list = if shown.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::meta(
                    if run.step.is_some() {
                        "No output for this step yet."
                    } else {
                        "No output yet."
                    },
                    cx,
                ))
                .into_any_element()
        } else {
            let lines = shown.clone();
            uniform_list("rundeck-output", count, move |range, _, cx| {
                range
                    .map(|at| {
                        let line = &lines[at];
                        let level = level_of(line);
                        let text_ink = match level.as_str() {
                            "ERROR" => danger,
                            "WARN" => warn,
                            "DEBUG" | "VERBOSE" => dim,
                            _ => ink,
                        };
                        div()
                            .h(px(LOG_ROW))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                ui::mono(line.time.clone().unwrap_or_default(), 11., cx)
                                    .w(px(64.))
                                    .flex_none(),
                            )
                            .child(
                                ui::mono(
                                    line.step_ctx
                                        .as_deref()
                                        .and_then(|ctx| ctx.split(['/', '@']).next())
                                        .unwrap_or_default()
                                        .to_owned(),
                                    11.,
                                    cx,
                                )
                                .w(px(24.))
                                .flex_none(),
                            )
                            .child(output_text(line, text_ink, cx))
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&run.scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        let banners = [
            run.failed.then(|| {
                banner(
                    format!(
                        "The log stream stopped: {}",
                        run.error
                            .clone()
                            .unwrap_or_else(|| "too many errors".into())
                    ),
                    t.danger,
                    cx,
                )
            }),
            (!run.failed)
                .then(|| run.error.clone())
                .flatten()
                .map(|error| banner(format!("Log stream: {error}"), t.warn, cx)),
            (run.dropped > 0).then(|| {
                banner(
                    format!(
                        "{} earlier lines aren't shown; Rundeck has them all.",
                        run.dropped
                    ),
                    t.text_tertiary,
                    cx,
                )
            }),
        ];
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.))
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        ui::text(
                            step_label.unwrap_or_else(|| "All steps".into()),
                            12.,
                            FontWeight::MEDIUM,
                        )
                        .text_color(t.text_primary),
                    )
                    .child(div().flex_1())
                    .child(
                        Segmented::words(
                            "rundeck-lines-shown",
                            vec![
                                ("All".into(), None),
                                ("Hide debug".into(), None),
                                ("Problems".into(), (errors > 0).then_some(errors as u64)),
                            ],
                            chosen,
                        )
                        .on_select(listen::<Rundeck, usize>(
                            cx,
                            |rd, _, index, _, cx| {
                                if let Some(run) = &mut rd.run {
                                    run.shown = [
                                        LinesShown::All,
                                        LinesShown::NoDebug,
                                        LinesShown::Problems,
                                    ][*index];
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        Toggle::new("rundeck-follow", "Follow", run.follow).on_click(listen::<
                            Rundeck,
                            ClickEvent,
                        >(
                            cx,
                            |rd, _, _, _, cx| {
                                if let Some(run) = &mut rd.run {
                                    run.follow = !run.follow;
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(ui::meta(streaming, cx).text_color(if streaming == "Live" {
                        t.live
                    } else {
                        t.text_tertiary
                    })),
            )
            .children(banners.into_iter().flatten())
            .child(list)
            .child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .flex()
                    .border_t_1()
                    .border_color(t.border)
                    .child(ui::meta(
                        format!(
                            "{} · {errors} {}",
                            if count == run.lines.len() {
                                format!("{count} lines")
                            } else {
                                format!("{count} of {} lines", run.lines.len())
                            },
                            if errors == 1 { "error" } else { "errors" }
                        ),
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(ui::meta(
                        if run.follow {
                            "Following the newest output"
                        } else {
                            "Paused"
                        },
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// Streams the open run's output, picking up after the lines already read.
    fn follow_output(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(run) = &mut self.run else {
            return;
        };
        let stream = host.stream::<Rundeck>(
            "logs",
            json!({ "executionId": run.id, "offset": run.offset }),
            cx,
            |rd, _, streamed, _| {
                let Some(run) = &mut rd.run else {
                    return;
                };
                match streamed {
                    Streamed::Item(item) => {
                        let Ok(tick) = serde_json::from_value::<OutputTick>(item) else {
                            return;
                        };
                        let lines = Rc::make_mut(&mut run.lines);
                        lines.extend(tick.entries);
                        if lines.len() > LOG_LINES_KEPT {
                            let over = lines.len() - LOG_LINES_KEPT;
                            lines.drain(..over);
                            run.dropped += over;
                        }
                        run.offset = Some(tick.offset);
                        run.completed |= tick.completed;
                        run.failed = tick.failed;
                        run.error = tick.error;
                    }
                    Streamed::Ended(failure) => {
                        if let Some(failure) = failure {
                            run.error = Some(failure.message.to_string());
                        }
                    }
                }
            },
        );
        run._stream = Some(stream);
    }

    /// Tells the person when the run ends, with what the plugin says of how it went.
    fn watch(&mut self, host: &Host, job: &JobRef, id: u64, cx: &mut Context<PluginPane>) {
        if self.watching.contains_key(&id) {
            return;
        }
        let task = host.stream::<Rundeck>(
            "watch",
            json!({
                "executionId": id,
                "job": { "jobId": job.job_id, "name": job.name, "group": job.group, "project": job.project },
            }),
            cx,
            move |rd, host, streamed, cx| match streamed {
                Streamed::Item(item) => {
                    if item["terminal"] != Value::Bool(true) {
                        return;
                    }
                    if let Some(task) = rd.watching.remove(&id) {
                        task.detach();
                    }
                    host.read_again(None);
                    if let Ok(notice) = serde_json::from_value::<RunNotice>(item["notice"].clone()) {
                        tell(host, &notice, cx);
                    }
                }
                Streamed::Ended(_) => {
                    if let Some(task) = rd.watching.remove(&id) {
                        task.detach();
                    }
                }
            },
        );
        self.watching.insert(id, task);
    }
}

impl Output {
    fn new(id: u64) -> Self {
        Self {
            id,
            lines: Rc::new(Vec::new()),
            dropped: 0,
            offset: None,
            completed: false,
            failed: false,
            error: None,
            step: None,
            shown: LinesShown::All,
            follow: true,
            scroll: UniformListScrollHandle::new(),
            _stream: None,
        }
    }
}

fn abort(host: &Host, job: &JobRef, id: u64, cx: &mut Context<PluginPane>) {
    let Some(bench) = host.bench().upgrade() else {
        return;
    };
    let pane = cx.weak_entity();
    let name = job.name.clone();
    bench.update(cx, |bench, cx| {
        bench.confirm(
            format!("Abort execution #{id}?"),
            format!("{name} will be stopped where it is."),
            "abort",
            true,
            move |_, yes, _, cx| {
                if !yes {
                    return;
                }
                let _ = pane.update(cx, |pane, cx| {
                    let stopped = pane.host.call("abort", json!({ "executionId": id }));
                    if let Some((rd, _)) = pane.parts::<Rundeck>() {
                        rd.aborting = Some(id);
                    }
                    cx.notify();
                    after::<Rundeck, _>(
                        stopped,
                        |rd, host, done, cx| {
                            rd.aborting = None;
                            match done {
                            Ok(result) if result["abort"]["status"] == "failed" => {
                                let reason = result["abort"]["reason"]
                                    .as_str()
                                    .unwrap_or("no reason given");
                                let failure = Failure {
                                    category: "rundeck".into(),
                                    message: format!("Rundeck refused to abort: {reason}").into(),
                                    status: None,
                                };
                                host.said(&failure, cx);
                            }
                            Ok(_) => {}
                            Err(failure) => host.said(&failure.titled("Abort"), cx),
                            }
                        },
                        cx,
                    );
                });
            },
            cx,
        );
    });
}

/// Says how a run ended over the window.
fn tell(host: &Host, notice: &RunNotice, cx: &mut Context<PluginPane>) {
    let text = match notice.body.is_empty() {
        true => notice.title.clone(),
        false => format!("{} — {}", notice.title, notice.body),
    };
    let note = if notice.ok {
        toast::success(text)
    } else {
        toast::error(text)
    };
    if let Some(bench) = host.bench().upgrade() {
        bench.update(cx, |bench, cx| bench.notify(note, cx));
    }
}

// ---- signing in --------------------------------------------------------------

impl Rundeck {
    fn sign_in(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        if self.sign_in.is_none() {
            let mut make = |placeholder: &str, masked: bool, cx: &mut Context<PluginPane>| {
                let placeholder = placeholder.to_owned();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(masked)
                });
                self.typing
                    .push(cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()));
                input
            };
            let url = make("https://rundeck.example.com", false, cx);
            let token = make("API token", true, cx);
            let user = make("Username", false, cx);
            let password = make("Password", true, cx);
            self.sign_in = Some(SignIn {
                with_token: true,
                url,
                token,
                user,
                password,
                insecure: false,
                busy: false,
            });
        }
        let message = host
            .status(cx)
            .read::<Value>()
            .ok()
            .flatten()
            .and_then(|status| status["message"].as_str().map(str::to_owned));
        let form = self.sign_in.as_ref().expect("made just now");
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let filled = !value(&form.url).is_empty()
            && if form.with_token {
                !value(&form.token).is_empty()
            } else {
                !value(&form.user).is_empty() && !value(&form.password).is_empty()
            };
        let labelled = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(ui::meta(label, cx))
                .child(ui::Field::new(input))
        };
        let mode = Segmented::words(
            "rundeck-sign-in-mode",
            vec![
                ("API token".into(), None),
                ("Username and password".into(), None),
            ],
            usize::from(!form.with_token),
        )
        .on_select(listen::<Rundeck, usize>(cx, |rd, _, index, _, cx| {
            if let Some(form) = &mut rd.sign_in {
                form.with_token = *index == 0;
            }
            cx.notify();
        }));
        div()
            .id("rundeck-sign-in")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                div()
                    .w(px(380.))
                    .py(px(40.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(ui::icon(Icon::Rundeck, 26.).text_color(t.text_primary))
                    .child(
                        ui::text("Sign in to Rundeck", 15., FontWeight::SEMIBOLD)
                            .text_color(t.text_primary),
                    )
                    .child(ui::text(PITCH, 12.5, FontWeight::NORMAL).text_color(t.text_secondary))
                    .children(message.map(|message| {
                        ui::text(message, 12., FontWeight::NORMAL)
                            .text_color(t.danger)
                            .whitespace_normal()
                    }))
                    .child(labelled("Rundeck address", &form.url))
                    .child(mode)
                    .when(form.with_token, |page| {
                        page.child(labelled("API token", &form.token))
                    })
                    .when(!form.with_token, |page| {
                        page.child(labelled("Username", &form.user))
                            .child(labelled("Password", &form.password))
                    })
                    .child(
                        div().flex().child(Toggle::new(
                            "rundeck-insecure",
                            "Allow plain HTTP on a private network",
                            form.insecure,
                        )
                        .on_click(listen::<Rundeck, ClickEvent>(
                            cx,
                            |rd, _, _, _, cx| {
                                if let Some(form) = &mut rd.sign_in {
                                    form.insecure = !form.insecure;
                                }
                                cx.notify();
                            },
                        ))),
                    )
                    .child(
                        Button::new(
                            "rundeck-sign-in-go",
                            if form.busy {
                                "Signing in…"
                            } else {
                                "Sign in"
                            },
                            Variant::Primary,
                        )
                        .icon(Icon::LogIn)
                        .disabled(!filled || form.busy)
                        .on_click(listen::<Rundeck, ClickEvent>(cx, |rd, host, _, _, cx| {
                            rd.sign_in_now(host, cx)
                        })),
                    ),
            )
            .into_any_element()
    }

    fn sign_in_now(&mut self, host: &Host, cx: &mut Context<PluginPane>) {
        let Some(form) = &mut self.sign_in else {
            return;
        };
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let (method, params) = if form.with_token {
            (
                "loginWithToken",
                json!({ "url": value(&form.url), "token": value(&form.token), "allow_insecure_private_http": form.insecure }),
            )
        } else {
            (
                "login",
                json!({
                    "url": value(&form.url),
                    "user": value(&form.user),
                    "password": value(&form.password),
                    "allow_insecure_private_http": form.insecure,
                }),
            )
        };
        form.busy = true;
        let call = host.call(method, params);
        after::<Rundeck, _>(
            call,
            |rd, host, done, cx| {
                match done {
                    Ok(_) => rd.sign_in = None,
                    Err(failure) => {
                        if let Some(form) = &mut rd.sign_in {
                            form.busy = false;
                        }
                        host.said(&failure.titled("Sign in to Rundeck"), cx);
                    }
                }
                host.read_again(None);
            },
            cx,
        );
        cx.notify();
    }
}

// ---- small parts -------------------------------------------------------------

/// A page's head: where it is, its title with what sits beside it, and its tools.
fn header(
    crumbs: AnyElement,
    title: impl Into<SharedString>,
    aside: Option<AnyElement>,
    tools: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(px(20.))
        .pt(px(12.))
        .pb(px(10.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .border_b_1()
        .border_color(t.border)
        .child(crumbs)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .child(ui::text(title, 17., FontWeight::SEMIBOLD).text_color(t.text_primary))
                .children(aside)
                .child(div().flex_1())
                .child(tools),
        )
        .into_any_element()
}

fn crumbs(project: &str, group: Option<&str>, cx: &App) -> AnyElement {
    let mut path = vec![project.to_owned()];
    path.extend(
        group
            .unwrap_or_default()
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned),
    );
    ui::meta(path.join(" › "), cx).into_any_element()
}

fn refresh(id: &'static str, method: &'static str, cx: &mut Context<PluginPane>) -> AnyElement {
    IconButton::new(id, Icon::Refresh, "Refresh")
        .on_click(listen::<Rundeck, ClickEvent>(
            cx,
            move |_, host, _, _, _| host.read_again(Some(method)),
        ))
        .into_any_element()
}

fn card(
    title: &'static str,
    note: impl Into<SharedString>,
    body: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(12.))
                .py(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .border_b_1()
                .border_color(t.line_soft)
                .child(ui::text(title, 12.5, FontWeight::SEMIBOLD).text_color(t.text_primary))
                .child(div().flex_1())
                .child(ui::meta(note.into(), cx).truncate()),
        )
        .child(div().p(px(12.)).child(body))
        .into_any_element()
}

fn banner(text: String, ink: Hsla, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .mx(px(20.))
        .my(px(6.))
        .px(px(10.))
        .py(px(6.))
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(ink.opacity(0.4))
        .bg(ink.opacity(0.08))
        .child(
            ui::text(text, 12., FontWeight::NORMAL)
                .text_color(t.text_primary)
                .whitespace_normal(),
        )
        .into_any_element()
}

fn tag(text: &str, ink: Hsla, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(px(5.))
        .rounded(theme::RADIUS_1)
        .border_1()
        .border_color(t.border)
        .child(ui::text(text.to_owned(), 10.5, FontWeight::MEDIUM).text_color(ink))
        .into_any_element()
}

fn meta_chip(icon: Icon, text: String, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(px(8.))
        .py(px(3.))
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .flex()
        .items_center()
        .gap(px(6.))
        .child(ui::icon(icon, 12.).text_color(t.text_tertiary))
        .child(ui::text(text, 12., FontWeight::NORMAL).text_color(t.text_secondary))
        .into_any_element()
}

/// A job's folder as a path, amber when it deploys to production.
fn folder_chip(project: &str, group: Option<&str>, prod: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let path = group
        .filter(|group| !group.is_empty())
        .unwrap_or(project)
        .to_owned();
    tag(&path, if prod { t.warn } else { t.text_secondary }, cx)
}

fn branch_chip(branch: Option<&str>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    match branch.filter(|branch| !branch.is_empty()) {
        None => ui::meta("—", cx).into_any_element(),
        Some(branch) => div()
            .flex()
            .items_center()
            .gap(px(4.))
            .min_w_0()
            .child(ui::icon(Icon::Git, 11.).text_color(t.text_tertiary))
            .child(
                ui::text(branch.to_owned(), 12., FontWeight::NORMAL)
                    .text_color(t.accent)
                    .truncate(),
            )
            .into_any_element(),
    }
}

fn status_ink(kind: StatusKind, t: &Tokens) -> Hsla {
    match kind {
        StatusKind::Succeeded => t.live,
        StatusKind::Failed => t.danger,
        StatusKind::Running => t.warn,
        StatusKind::Aborted => t.text_tertiary,
        StatusKind::Unknown => t.text_disabled,
    }
}

fn step_ink(state: StepState, t: &Tokens) -> Hsla {
    match state {
        StepState::Ok => t.live,
        StepState::Fail => t.danger,
        StepState::Running => t.warn,
        StepState::Skip => t.text_disabled,
        StepState::Pending => t.line_soft,
    }
}

fn status_dot(kind: Option<StatusKind>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let ink = kind.map_or(t.line_soft, |kind| status_ink(kind, t));
    div()
        .size(px(7.))
        .flex_none()
        .rounded_full()
        .bg(ink)
        .into_any_element()
}

fn capitalised(word: &str) -> String {
    let mut letters = word.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

fn level_of(line: &OutputLine) -> String {
    line.level.as_deref().unwrap_or_default().to_uppercase()
}

/// Whether a line's step context is the step `key`, or one of its parts or nodes.
fn step_matches(ctx: Option<&str>, key: &str) -> bool {
    let ctx = ctx.unwrap_or_default();
    ctx == key || ctx.starts_with(&format!("{key}/")) || ctx.starts_with(&format!("{key}@"))
}

/// A line of output in the styles its terminal codes set, as the plugin read them.
fn output_text(line: &OutputLine, ink: Hsla, cx: &App) -> Div {
    let text = div().flex_1().min_w_0().flex().overflow_hidden();
    if line.runs.is_empty() {
        return text.child(
            ui::mono(line.log.clone().unwrap_or_default(), 11.5, cx)
                .text_color(ink)
                .truncate(),
        );
    }
    let t = Tokens::get(cx);
    text.children(line.runs.iter().map(|run| {
        let mut part = ui::mono(run.text.clone(), 11.5, cx)
            .flex_none()
            .text_color(run.fg.map_or(ink, |fg| term_ink(fg, t)))
            .when(run.bold, |part| part.font_weight(FontWeight::BOLD))
            .when(run.dim, |part| part.opacity(0.7))
            .when(run.italic, |part| part.italic())
            .when(run.underline, |part| part.underline());
        if let Some(bg) = run.bg {
            part = part.bg(term_ink(bg, t));
        }
        part
    }))
}

/// A terminal colour in the theme's terminal palette, or the xterm colour it numbers.
fn term_ink(colour: TermColor, t: &Tokens) -> Hsla {
    let [r, g, b] = match colour {
        TermColor::Rgb { r, g, b } => [r, g, b],
        TermColor::Palette { index } if index < 16 => t.terminal.ansi[index as usize],
        TermColor::Palette { index } if index < 232 => {
            let step = |level: u8| if level == 0 { 0 } else { 55 + level * 40 };
            let at = index - 16;
            [step(at / 36), step(at / 6 % 6), step(at % 6)]
        }
        TermColor::Palette { index } => {
            let grey = 8 + (index - 232) * 10;
            [grey, grey, grey]
        }
    };
    gpui_kit::rgb((r as u32) << 16 | (g as u32) << 8 | b as u32).into()
}

fn parse(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// How long a run took, or has taken so far.
fn run_secs(started: Option<&str>, ended: Option<&str>) -> Option<f64> {
    let started = parse(started?)?;
    let ended = ended.and_then(parse).unwrap_or_else(Utc::now);
    Some(((ended - started).num_milliseconds().max(0) as f64) / 1000.)
}

fn since(started: Option<&str>) -> Option<f64> {
    run_secs(started, None)
}

/// A length of time as the pane writes it: `4.2s`, `4m 12s`, `1h 5m`.
fn took(secs: f64) -> String {
    if secs <= 0. {
        return "0s".into();
    }
    if secs < 10. {
        let tenths = (secs * 10.).round() / 10.;
        return if tenths.fract() == 0. {
            format!("{tenths:.0}s")
        } else {
            format!("{tenths:.1}s")
        };
    }
    let seconds = secs.round() as u64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return match seconds % 60 {
            0 => format!("{minutes}m"),
            rest => format!("{minutes}m {rest}s"),
        };
    }
    match minutes % 60 {
        0 => format!("{}h", minutes / 60),
        rest => format!("{}h {rest}m", minutes / 60),
    }
}

fn day_of(at: &str) -> String {
    parse(at)
        .map(|at| at.with_timezone(&Local).format("%A %-d %B").to_string())
        .unwrap_or_else(|| "Unknown date".into())
}

fn clock_of(at: &str) -> String {
    parse(at)
        .map(|at| at.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

fn stamp_of(at: &str) -> String {
    parse(at)
        .map(|at| {
            at.with_timezone(&Local)
                .format("%-d %b, %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| at.to_owned())
}

/// A time typed in the person's own zone, as ISO 8601 with its offset, the form Rundeck wants.
fn local_to_iso(typed: &str) -> Option<String> {
    let typed = typed.trim().replace('T', " ");
    let naive = NaiveDateTime::parse_from_str(&typed, "%Y-%m-%d %H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(&typed, "%Y-%m-%d %H:%M:%S"))
        .ok()?;
    let local = Local.from_local_datetime(&naive).single()?;
    Some(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
}
