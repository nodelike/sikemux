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
use sikemux_plugin_api::SignInStatus;
use sikemux_plugin_rundeck_views::{
    ActivityPage, ActivityRequest, CheckRequest, Connection, DeployForm, DeployRequest,
    DeployTarget, EnvTone, ExecutionPage, ExecutionRequest, GroupNode, JobCatalog, JobOption,
    JobPage, JobRequest, JobRow, Jobs, JobsRequest, LiveRun, OptionChoice, OptionKind, OutputLine,
    OutputTick, Picked, Plan, PlanRequest, Run, RunCheck, RunNotice, RunStep, Section, StatusKind,
    StepState, TargetRequest, TermColor, Tone, Tree,
};

use super::{after, listen, Failure, Host, Panel, PluginPane, Streamed};
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::toast;
use crate::ui::{self, sign_in, Button, IconButton, Segmented, Toggle, Variant};

mod chip;
mod compare;
mod execution;
mod form;
mod jobs;
mod parts;
mod runs;
mod signing_in;
mod time;
mod tree;

use self::chip::DeployChip;
use self::compare::compare_grid;
use self::parts::{
    banner, branch_chip, capitalised, card, crumbs, folder_chip, header, meta_chip, refresh,
    status_dot, status_ink, step_ink, tag,
};
use self::time::{clock_of, day_of, local_to_iso, run_secs, since, stamp_of, took};

const PITCH: &str = "Your jobs and deploys, beside the code.";
const TREE_WIDTH: f32 = 240.;
const ACTIVITY_PAGE: u32 = 50;
const ACTIVITY_MOST: u32 = 200;
/// The most output lines a run's page keeps; older ones are counted, not shown.
const LOG_LINES_KEPT: usize = 10_000;
const LOG_ROW: f32 = 18.;

/// Which Rundeck the pane is signed in to, as the plugin's `status` says.
type Status = SignInStatus<Connection>;

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
    stream: Stream,
    step: Option<String>,
    shown: LinesShown,
    follow: bool,
    scroll: UniformListScrollHandle,
    _stream: Option<Task<()>>,
}

/// How a run's output stream stands.
enum Stream {
    Flowing,
    /// Rundeck said something went wrong; the stream carries on.
    Erred(String),
    /// The stream gave up, with why when it said.
    GaveUp(Option<String>),
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
                JobsRequest {
                    project: self.project.clone(),
                    group: self.group.clone(),
                },
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
    let catalog = match serde_json::from_value::<JobCatalog>(catalog.clone()) {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("sikemux: Rundeck's jobs did not read for the palette: {error}");
            return Vec::new();
        }
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
