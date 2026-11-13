//! The first-run setup over the whole window: an agent that works, then the projects to open.
//! On a real first run nothing leaves it but finishing, and only finishing counts it done. When
//! an agent is already signed in and its history names projects, it finishes by itself.

use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

use gpui_kit::base::{ScrollbarMode, ScrollbarThumbStyle};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::Role;
use gpui_kit::{
    actions, div, px, relative, transparent_black, AnyElement, App, AppContext, Context, Div,
    Entity, FocusHandle, FontFeatures, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, PathPromptOptions, Rgba,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    WindowControlArea,
};
use sikemux_agents::agent_accounts::status::AgentStatus;
use sikemux_agents::agent_accounts::status::FREE_MODELS;
use sikemux_agents::sessions::found_projects::FoundProject;
use sikemux_agents::sessions::recent::MAX_PROJECTS;
use sikemux_calls::call;
use sikemux_calls::params::NoParams;
use sikemux_calls::project_roots::ResolveParams;
use sikemux_calls::project_roots::Resolved;
use sikemux_calls::usage::Step;
use sikemux_view::layout::{At, OpenTab, PaneKind};
use sikemux_view::prefs::PreferenceChange;
use tokio::sync::oneshot;

use crate::agents::setup::{self, Doing, Found, Sheet};
use crate::core::Target;
use crate::icons::Icon;
use crate::screens::experience::{self, Experience};
use crate::theme::{self, Tokens};
use crate::ui;
use crate::ui::path::{this_home, tilde};
use crate::workbench::Workbench;

const TERMINAL_HEIGHT: f32 = 190.;
const TERMINAL_HEIGHT_COMPACT: f32 = 104.;
/// How often to look again for a starting terminal that can take the keys.
const TERMINAL_WAIT: Duration = Duration::from_millis(50);
const COMPACT_HEIGHT: f32 = 700.;
const COMPACT_WIDTH: f32 = 1100.;
const ROW_HEIGHT: f32 = 46.;
const ROW_HEIGHT_COMPACT: f32 = 32.;
const TILES_ACROSS: usize = 2;
const STEPS: usize = 2;
const LABEL_LINE: f32 = 1.4;
/// How much dither a tile gets for its state, in a roomy window and a small one.
#[derive(Clone, Copy)]
struct Density {
    roomy: f32,
    compact: f32,
}

const DENSITY_READY: Density = Density {
    roomy: 0.62,
    compact: 0.62,
};
const DENSITY_INSTALLED: Density = Density {
    roomy: 0.42,
    compact: 0.42,
};
const DENSITY_SIGNED_OUT: Density = Density {
    roomy: 0.34,
    compact: 0.,
};
const DENSITY_MISSING: Density = Density {
    roomy: 0.26,
    compact: 0.,
};
const DENSITY_FAILED: Density = Density {
    roomy: 0.,
    compact: 0.,
};
const DENSITY_RUNNING: Density = Density {
    roomy: 0.42,
    compact: 0.42,
};
/// A bright colour is dimmed by its opacity; thinning its dots as far would leave only specks.
const LEAST_DENSITY_FIT: f32 = 0.85;
const STEP_MILLIS: u64 = 200;
const RESOLVE_MILLIS: u64 = 700;
const APPEAR_MILLIS: u64 = 260;
/// The window sizes the poster's measures run between.
const SMALLEST_WINDOW: (f32, f32) = (860., 540.);
const ROOMIEST_WINDOW: (f32, f32) = (1768., 1099.);
/// Below this much room for text, the poster's quiet actions get a line of their own.
const EXTRAS_INLINE_WIDTH: f32 = 440.;
/// The progress dots and the gap after them.
const DOTS_WIDTH: f32 = 60.;
/// A character's average width, as a share of the text's size.
const SAID_CHAR: f32 = 0.5;
const MONO_CHAR: f32 = 0.6;
const BACK_LINE: f32 = 1.2;
/// The most of a row a project's name takes before its path.
const NAME_SHARE: f32 = 0.62;
/// An agent's mark sits in a box this much taller than it, and at least this far above its name.
const MARK_BOX: f32 = 1.36;
const MARK_TO_NAME: f32 = 16.;

/// Whether a first run finishes by itself, once what it needs is known.
#[derive(Clone, Copy, PartialEq)]
enum Shortcut {
    Undecided,
    Taken,
    Declined,
}

/// Whether the mark is still resolving in, the first thing the setup does.
#[derive(Clone, Copy)]
enum Beat {
    Resolving(Instant),
    Done,
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Agents,
    Projects,
}

/// A place the keyboard can stand on.
#[derive(Clone, Copy, PartialEq)]
enum Stop {
    Row(usize),
    Anyway,
    Field,
    Browse,
    Back,
    Terminal,
    SheetClose,
    Recheck,
}

struct Candidate {
    name: String,
    path: String,
    shown: String,
    ago: Option<String>,
    chats: u32,
    ticked: bool,
}

impl Candidate {
    fn found(project: FoundProject, home: Option<&str>) -> Self {
        Self {
            shown: tilde(&project.path, home),
            ago: Some(ui::time::ago(project.active_ms)),
            chats: project.chats,
            name: project.name,
            path: project.path,
            ticked: project.suggested,
        }
    }

    fn chosen_by_hand(path: String, home: Option<&str>) -> Self {
        Self {
            name: Path::new(&path)
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
            shown: tilde(&path, home),
            ago: None,
            chats: 0,
            path,
            ticked: true,
        }
    }
}

pub struct Welcome {
    /// Holds the keys whenever the path field or a terminal does not.
    root: FocusHandle,
    /// A real first run, which has no way out but finishing.
    required: bool,
    shortcut: Shortcut,
    stage: Stage,
    at: Option<Stop>,
    git_missing: bool,
    rows: Vec<Candidate>,
    looking: bool,
    partial: bool,
    path: Option<Entity<InputState>>,
    /// What is wrong with the typed path.
    error: Option<String>,
    /// Why Sikemux couldn't be opened on what was ticked.
    open_error: Option<String>,
    /// The agent whose install or sign-in was run last, whose failure the poster explains.
    agent: Option<String>,
    opening: bool,
    /// The terminal of a command just started takes the keys once it has been drawn.
    terminal_due: bool,
    /// A command was running as the last frame drew, so its ending can take the keys back.
    was_busy: bool,
    scan_failed: bool,
    /// What is typed in the field, which narrows the list unless it is a path.
    filter: String,
    beat: Beat,
    stage_at: Instant,
    /// How far the step has faded in. Dither paints apart from element opacity, so it is told.
    fade: Cell<f32>,
    scroll: ScrollHandle,
    _typing: Option<Subscription>,
    _activation: Option<Subscription>,
}

impl Welcome {
    pub fn new(required: bool, cx: &mut Context<Workbench>) -> Self {
        Self {
            root: cx.focus_handle(),
            required,
            shortcut: match required {
                true => Shortcut::Undecided,
                false => Shortcut::Declined,
            },
            stage: Stage::Agents,
            at: None,
            git_missing: false,
            rows: Vec::new(),
            looking: true,
            partial: false,
            path: None,
            error: None,
            open_error: None,
            agent: None,
            opening: false,
            terminal_due: false,
            was_busy: false,
            scan_failed: false,
            filter: String::new(),
            beat: Beat::Resolving(Instant::now()),
            stage_at: Instant::now(),
            fade: Cell::new(1.),
            scroll: ScrollHandle::new(),
            _typing: None,
            _activation: None,
        }
    }

    fn ticked(&self) -> impl Iterator<Item = &Candidate> {
        self.rows.iter().filter(|row| row.ticked)
    }

    /// Ticks the folder chosen by hand, adding it to the top when it was not listed.
    fn take_folder(&mut self, path: String) {
        let path = match path.trim_end_matches('/') {
            "" => path,
            trimmed => trimmed.to_owned(),
        };
        let at = match self.rows.iter().position(|row| row.path == path) {
            Some(at) => {
                self.rows[at].ticked = true;
                at
            }
            None => {
                let row = Candidate::chosen_by_hand(path, this_home().as_deref());
                self.rows.insert(0, row);
                0
            }
        };
        self.scroll.scroll_to_item(at);
    }
}

fn welcome_of(bench: &mut Workbench) -> Option<&mut Welcome> {
    match bench.overlays.experience.as_mut()? {
        Experience::Welcome(welcome) => Some(welcome),
        _ => None,
    }
}

fn welcome_in(bench: &Workbench) -> Option<&Welcome> {
    match bench.overlays.experience.as_ref()? {
        Experience::Welcome(welcome) => Some(welcome),
        _ => None,
    }
}

fn redraw(bench: &Workbench, cx: &mut Context<Workbench>) {
    bench.parts.redraw(&bench.parts.over, cx);
    cx.notify();
}

/// Looks for the agents here, the folders their history names, and whether git is.
pub fn opened(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.agent_setup.ended = false;
    setup::read(bench, cx);
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let tools = Target::here(&asks)
            .request_typed::<call::HostTools>(NoParams {})
            .await;
        let _ = bench.update(cx, |bench, cx| {
            if let (Some(welcome), Ok(tools)) = (welcome_of(bench), tools) {
                welcome.git_missing = !tools.git;
            }
            redraw(bench, cx);
        });
    })
    .detach();
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let asked = Target::here(&asks)
            .request_typed::<call::FoundProjects>(NoParams {})
            .await;
        let failed = asked.is_err();
        let found = asked.unwrap_or_default();
        let _ = bench.update(cx, |bench, cx| {
            let Some(welcome) = welcome_of(bench) else {
                return;
            };
            let home = this_home();
            let fresh: Vec<Candidate> = found
                .projects
                .into_iter()
                .filter(|project| !welcome.rows.iter().any(|row| row.path == project.path))
                .map(|project| Candidate::found(project, home.as_deref()))
                .collect();
            welcome.rows.extend(fresh);
            welcome.partial = found.partial;
            welcome.scan_failed = failed;
            welcome.looking = false;
            redraw(bench, cx);
        });
    })
    .detach();
}

/// Shows the mark and the step at once, for whoever pressed a key while it resolved.
fn skip_opening(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if let Some(welcome) = welcome_of(bench) {
        if matches!(welcome.beat, Beat::Resolving(_)) {
            welcome.beat = Beat::Done;
            welcome.stage_at = Instant::now();
        }
    }
    redraw(bench, cx);
}

fn opening_is_up(bench: &Workbench) -> bool {
    welcome_in(bench).is_some_and(|welcome| matches!(welcome.beat, Beat::Resolving(_)))
}

/// Which rows the field leaves showing. A path being typed narrows nothing.
fn visible_rows(welcome: &Welcome) -> Vec<usize> {
    let filter = welcome.filter.trim().to_lowercase();
    let is_path = filter.starts_with(['/', '~', '.']);
    (0..welcome.rows.len())
        .filter(|&at| {
            let row = &welcome.rows[at];
            filter.is_empty()
                || is_path
                || row.name.to_lowercase().contains(&filter)
                || row.shown.to_lowercase().contains(&filter)
        })
        .collect()
}

/// Makes the path field once a window can hold it, and keeps the keys here.
pub fn take_keys(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    if welcome_in(bench).is_none() {
        return;
    }
    let terminal = setup::terminal_focus(bench);
    let busy = bench.agent_setup.busy();
    let tile = welcome_in(bench).and_then(|welcome| pill_row(bench, welcome));
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    if welcome._activation.is_none() {
        welcome._activation = Some(cx.observe_window_activation(window, |bench, window, cx| {
            let on_agents = welcome_in(bench).is_some_and(|welcome| welcome.stage == Stage::Agents);
            if window.is_window_active() && on_agents && !bench.agent_setup.busy() {
                setup::recheck(bench, cx);
            }
        }));
    }
    if welcome.path.is_none() {
        let path = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("Find a folder or type a path")));
        let typing = cx.subscribe_in(
            &path,
            window,
            |bench, field, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let typed = field.read(cx).value().to_string();
                    if let Some(welcome) = welcome_of(bench) {
                        welcome.error = None;
                        welcome.filter = typed;
                    }
                    redraw(bench, cx);
                }
                InputEvent::Focus => {
                    if let Some(welcome) = welcome_of(bench) {
                        welcome.at = Some(Stop::Field);
                    }
                    redraw(bench, cx);
                }
                InputEvent::PressEnter { .. } => add_typed(bench, window, cx),
                _ => {}
            },
        );
        welcome.path = Some(path);
        welcome._typing = Some(typing);
    }
    if let Beat::Resolving(since) = welcome.beat {
        let opening = Duration::from_millis(RESOLVE_MILLIS + APPEAR_MILLIS);
        if cx.reduce_motion() || since.elapsed() >= opening {
            welcome.beat = Beat::Done;
        }
    }
    let finished = std::mem::replace(&mut welcome.was_busy, busy) && !busy;
    if finished && welcome.stage == Stage::Agents {
        welcome.at = tile.map(Stop::Row);
        welcome.terminal_due = false;
        welcome.root.focus(window, cx);
    } else if welcome.terminal_due {
        match terminal {
            Some(terminal) => {
                welcome.terminal_due = false;
                welcome.at = Some(Stop::Terminal);
                terminal.focus(window, cx);
                return;
            }
            None if busy => cx
                .spawn(async move |bench, cx| {
                    cx.background_executor().timer(TERMINAL_WAIT).await;
                    let _ = bench.update(cx, |bench, cx| redraw(bench, cx));
                })
                .detach(),
            None => welcome.terminal_due = false,
        }
    }
    if !welcome.root.contains_focused(window, cx) {
        welcome.root.focus(window, cx);
    }
    take_shortcut(bench, window, cx);
}

/// Opens the app straight away on a first run where an agent is signed in and its history
/// names projects: the suggested ones, or else the newest, on a new chat with that agent. Once
/// the person has done anything in the setup it stays theirs to finish.
fn take_shortcut(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    if welcome.shortcut != Shortcut::Undecided {
        return;
    }
    let touched = welcome.at.is_some() || welcome.stage != Stage::Agents;
    let ready = found_agents(bench).iter().any(is_ready);
    let decided = match (&bench.agent_setup.found, welcome.looking) {
        _ if touched || bench.agent_setup.busy() => Some(Shortcut::Declined),
        (None, _) | (_, true) => None,
        (Some(_), false) if !ready || welcome.rows.is_empty() => Some(Shortcut::Declined),
        (Some(_), false) => start_agent(bench)
            .filter(|agent| is_ready(agent))
            .map(|_| Shortcut::Taken),
    };
    let Some(decided) = decided else {
        return;
    };
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    welcome.shortcut = decided;
    if decided == Shortcut::Taken {
        if welcome.ticked().next().is_none() {
            welcome.rows[0].ticked = true;
        }
        cx.defer_in(window, finish);
    }
}

actions!(welcome, [Next, Previous, RowDown, RowUp]);

pub fn install(cx: &mut App) {
    let keys = Some("Onboarding && !Terminal");
    cx.bind_keys([
        KeyBinding::new("tab", Next, keys),
        KeyBinding::new("right", Next, keys),
        KeyBinding::new("shift-tab", Previous, keys),
        KeyBinding::new("left", Previous, keys),
        KeyBinding::new("down", RowDown, keys),
        KeyBinding::new("up", RowUp, keys),
    ]);
    cx.bind_keys([KeyBinding::new(
        "shift-tab",
        Previous,
        Some("Onboarding > Terminal"),
    )]);
    let field = Some("Onboarding > Input");
    cx.bind_keys([
        KeyBinding::new("tab", Next, field),
        KeyBinding::new("down", Next, field),
        KeyBinding::new("shift-tab", Previous, field),
        KeyBinding::new("up", Previous, field),
    ]);
}

fn found_agents(bench: &Workbench) -> &[Found] {
    bench.agent_setup.found.as_deref().unwrap_or_default()
}

fn is_ready(agent: &Found) -> bool {
    matches!(agent.status, AgentStatus::Ready { .. })
}

fn usable_agents(bench: &Workbench) -> usize {
    found_agents(bench)
        .iter()
        .filter(|agent| setup::usable(&agent.status))
        .count()
}

/// Reading the agents again helps only while none works and nothing is running.
fn recheck_offered(bench: &Workbench) -> bool {
    !bench.agent_setup.busy() && bench.agent_setup.found.is_some() && usable_agents(bench) == 0
}

fn anyway_offered(bench: &Workbench) -> bool {
    let progress = &bench.agent_setup;
    let Some(found) = &progress.found else {
        return false;
    };
    let nothing_to_try = !found
        .iter()
        .any(|agent| setup::next_step(bench, agent).is_some());
    usable_agents(bench) == 0
        && !progress.busy()
        && (progress.ended
            || bench.preferences().onboarding_tried
            || found.is_empty()
            || nothing_to_try)
}

/// Every agent, ready ones first, then those with a next step, then the rest.
fn listed_agents(bench: &Workbench) -> Vec<&Found> {
    let found = found_agents(bench);
    let ready = found.iter().filter(|agent| is_ready(agent));
    let with_step = found
        .iter()
        .filter(|agent| !is_ready(agent) && setup::next_step(bench, agent).is_some());
    let mut listed: Vec<&Found> = ready.chain(with_step).collect();
    let rest: Vec<&Found> = found
        .iter()
        .filter(|agent| !listed.iter().any(|shown| shown.kind == agent.kind))
        .collect();
    listed.extend(rest);
    listed
}

/// The agents the first chat can open on, ready ones before those only installed.
fn startable_agents(bench: &Workbench) -> Vec<&Found> {
    listed_agents(bench)
        .into_iter()
        .filter(|agent| setup::usable(&agent.status))
        .filter(|agent| bench.launcher_of_kind(&agent.kind).is_some())
        .collect()
}

/// The tile whose install or sign-in was run last, while it is listed.
fn pill_row(bench: &Workbench, welcome: &Welcome) -> Option<usize> {
    let kind = welcome.agent.as_deref()?;
    listed_agents(bench)
        .iter()
        .position(|agent| agent.kind == kind)
}

/// The tile the keys are on, on the first step.
fn keyed_tile<'a>(bench: &'a Workbench, welcome: &Welcome) -> Option<(usize, &'a Found)> {
    match (welcome.stage, welcome.at) {
        (Stage::Agents, Some(Stop::Row(row))) => Some((row, *listed_agents(bench).get(row)?)),
        _ => None,
    }
}

/// The agent the first chat starts on: the first that is ready, else the first installed.
fn start_agent(bench: &Workbench) -> Option<&Found> {
    startable_agents(bench).into_iter().next()
}

fn stops(bench: &Workbench, welcome: &Welcome) -> Vec<Stop> {
    match welcome.stage {
        Stage::Agents => {
            let progress = &bench.agent_setup;
            let sheet = (progress.shown().then_some(Stop::Terminal).into_iter())
                .chain(progress.shown().then_some(Stop::SheetClose));
            (0..listed_agents(bench).len())
                .map(Stop::Row)
                .chain(sheet)
                .chain(recheck_offered(bench).then_some(Stop::Recheck))
                .chain(anyway_offered(bench).then_some(Stop::Anyway))
                .collect()
        }
        Stage::Projects => [Stop::Field, Stop::Browse]
            .into_iter()
            .chain(visible_rows(welcome).into_iter().map(Stop::Row))
            .chain([Stop::Back])
            .collect(),
    }
}

fn go(bench: &mut Workbench, stop: Stop, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    welcome.at = Some(stop);
    match (stop, &welcome.path) {
        (Stop::Field, Some(path)) => path.update(cx, |path, cx| path.focus(window, cx)),
        (Stop::Terminal, _) => welcome.terminal_due = true,
        _ => welcome.root.focus(window, cx),
    }
    if let Stop::Row(row) = stop {
        let showing = match welcome.stage {
            Stage::Projects => visible_rows(welcome).iter().position(|&at| at == row),
            _ => Some(row),
        };
        if let Some(at) = showing {
            welcome.scroll.scroll_to_item(at);
        }
    }
    redraw(bench, cx);
}

fn step(bench: &mut Workbench, forward: bool, window: &mut Window, cx: &mut Context<Workbench>) {
    if opening_is_up(bench) {
        return skip_opening(bench, cx);
    }
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    let focused = |focus: Option<FocusHandle>| focus.is_some_and(|focus| focus.is_focused(window));
    let in_terminal = focused(setup::terminal_focus(bench));
    let here = match (welcome.stage, welcome.at) {
        _ if in_terminal => Some(Stop::Terminal),
        (_, at) => at,
    };
    let stops = stops(bench, welcome);
    let Some(last) = stops.len().checked_sub(1) else {
        return;
    };
    let from = here.and_then(|at| stops.iter().position(|stop| *stop == at));
    let to = match (from, forward) {
        (None, true) => 0,
        (None, false) => last,
        (Some(at), true) => (at + 1) % stops.len(),
        (Some(at), false) => (at + last) % stops.len(),
    };
    go(bench, stops[to], window, cx);
}

/// Up and down a row of tiles on the first step; elsewhere the arrows step like Tab.
fn step_row(bench: &mut Workbench, down: bool, window: &mut Window, cx: &mut Context<Workbench>) {
    if opening_is_up(bench) {
        return skip_opening(bench, cx);
    }
    let tile = welcome_in(bench).and_then(|welcome| match (welcome.stage, welcome.at) {
        (Stage::Agents, Some(Stop::Row(row))) => Some(row),
        _ => None,
    });
    let Some(at) = tile else {
        return step(bench, down, window, cx);
    };
    let last = listed_agents(bench).len().saturating_sub(1);
    let to = match down {
        true => (at + TILES_ACROSS).min(last),
        false => at.checked_sub(TILES_ACROSS).unwrap_or(at),
    };
    go(bench, Stop::Row(to), window, cx);
}

fn show_stage(
    bench: &mut Workbench,
    stage: Stage,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    welcome.stage = stage;
    welcome.stage_at = Instant::now();
    welcome.error = None;
    welcome.open_error = None;
    welcome.scroll = ScrollHandle::new();
    let first = match stage {
        Stage::Agents => None,
        Stage::Projects => Some(
            visible_rows(welcome)
                .first()
                .map_or(Stop::Field, |&at| Stop::Row(at)),
        ),
    };
    match first {
        Some(first) => go(bench, first, window, cx),
        None => {
            welcome.at = None;
            welcome.root.focus(window, cx);
            redraw(bench, cx);
        }
    }
}

fn back(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    if welcome_in(bench).is_some_and(|welcome| welcome.stage == Stage::Projects) {
        show_stage(bench, Stage::Agents, window, cx);
    }
}

fn run_step(bench: &mut Workbench, row: usize, cx: &mut Context<Workbench>) {
    if bench.agent_setup.busy() {
        return;
    }
    let next = listed_agents(bench)
        .get(row)
        .and_then(|agent| Some((agent.kind.clone(), setup::next_step(bench, agent)?)));
    if let Some((kind, (_, sheet))) = next {
        if let Some(welcome) = welcome_of(bench) {
            welcome.agent = Some(kind);
        }
        setup::run(bench, sheet, cx);
        hand_terminal_the_keys(bench);
    }
}

/// The running command's terminal takes the keys once it has been drawn.
fn hand_terminal_the_keys(bench: &mut Workbench) {
    if let Some(welcome) = welcome_of(bench) {
        welcome.at = Some(Stop::Terminal);
        welcome.terminal_due = true;
    }
}

fn toggle(bench: &mut Workbench, row: usize, cx: &mut Context<Workbench>) {
    if let Some(found) = welcome_of(bench).and_then(|welcome| welcome.rows.get_mut(row)) {
        found.ticked = !found.ticked;
    }
    redraw(bench, cx);
}

fn primary(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    match welcome.stage {
        Stage::Agents if usable_agents(bench) > 0 => show_stage(bench, Stage::Projects, window, cx),
        Stage::Agents => {}
        Stage::Projects if can_continue_projects(welcome) => finish(bench, window, cx),
        Stage::Projects => {}
    }
}

/// Space does what a row offers; Return presses a button or moves the setup on.
fn activate(bench: &mut Workbench, space: bool, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    match (welcome.stage, welcome.at) {
        (_, Some(Stop::Anyway)) => show_stage(bench, Stage::Projects, window, cx),
        (_, Some(Stop::Recheck)) => setup::recheck(bench, cx),
        (_, Some(Stop::SheetClose)) => {
            let tile = pill_row(bench, welcome);
            setup::close(bench, cx);
            match tile {
                Some(tile) => go(bench, Stop::Row(tile), window, cx),
                None => redraw(bench, cx),
            }
        }
        (_, Some(Stop::Browse)) => browse(window, cx),
        (_, Some(Stop::Back)) => back(bench, window, cx),
        (Stage::Agents, Some(Stop::Row(row))) => {
            let pill = listed_agents(bench)
                .get(row)
                .is_some_and(|agent| setup::next_step(bench, agent).is_some());
            match (pill, space) {
                (true, _) => run_step(bench, row, cx),
                (false, false) => primary(bench, window, cx),
                (false, true) => {}
            }
        }
        (Stage::Projects, Some(Stop::Row(row))) if space => toggle(bench, row, cx),
        _ if !space => primary(bench, window, cx),
        _ => {}
    }
}

fn key_down(
    bench: &mut Workbench,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if opening_is_up(bench) {
        cx.stop_propagation();
        return skip_opening(bench, cx);
    }
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    if !welcome.root.is_focused(window) || event.keystroke.modifiers.modified() {
        return;
    }
    match event.keystroke.key.as_str() {
        "escape" => {
            cx.stop_propagation();
            if !welcome.required {
                leave(bench, cx);
            }
        }
        "enter" => {
            cx.stop_propagation();
            activate(bench, false, window, cx);
        }
        "space" => {
            cx.stop_propagation();
            activate(bench, true, window, cx);
        }
        _ => {}
    }
}

fn leave(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.agent_setup.drop_finished();
    experience::close(bench, cx);
}

fn path_problem(typed: &str, resolved: &Resolved) -> Option<String> {
    if !Path::new(&resolved.path).is_absolute() {
        return Some(t!("Type the whole path, starting with / or ~").to_owned());
    }
    match resolved.kind.as_deref() {
        Some("dir") => None,
        Some("denied") => Some(t!("Sikemux isn't allowed to read {path}", path = typed)),
        Some(_) => Some(t!("{path} is a file, not a folder", path = typed)),
        None => Some(t!(
            "Nothing at {path}. Check the path or use Choose folder…",
            path = typed
        )),
    }
}

/// Adds the typed path once the core says it is a folder; an empty field moves the setup on.
fn add_typed(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let typed = welcome_in(bench)
        .and_then(|welcome| welcome.path.as_ref())
        .map(|path| path.read(cx).value().trim().to_owned())
        .unwrap_or_default();
    if typed.is_empty() {
        return primary(bench, window, cx);
    }
    if !typed.starts_with(['/', '~', '.']) {
        let first = welcome_in(bench).and_then(|welcome| visible_rows(welcome).first().copied());
        if let Some(at) = first {
            toggle(bench, at, cx);
            if let Some(path) = welcome_in(bench).and_then(|welcome| welcome.path.clone()) {
                path.update(cx, |path, cx| path.set_value("", window, cx));
            }
        }
        return;
    }
    let asked = Target::here(&bench.asks).request_typed::<call::ResolvePath>(ResolveParams {
        path: typed.clone(),
    });
    cx.spawn_in(window, async move |bench, cx| {
        let resolved = asked.await;
        let _ = bench.update_in(cx, |bench, window, cx| {
            let Some(welcome) = welcome_of(bench) else {
                return;
            };
            let problem = match &resolved {
                Ok(resolved) => path_problem(&typed, resolved),
                Err(error) => Some(t!("Could not check that path: {error}", error = error)),
            };
            match (problem, resolved) {
                (Some(problem), _) => welcome.error = Some(problem),
                (None, Ok(resolved)) => {
                    welcome.take_folder(resolved.path);
                    if let Some(path) = welcome.path.clone() {
                        path.update(cx, |path, cx| path.set_value("", window, cx));
                    }
                }
                (None, Err(_)) => {}
            }
            redraw(bench, cx);
        });
    })
    .detach();
}

fn browse(window: &mut Window, cx: &mut Context<Workbench>) {
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
        let path = path.to_string_lossy().into_owned();
        let _ = bench.update(cx, |bench, cx| {
            if let Some(welcome) = welcome_of(bench) {
                welcome.take_folder(path);
            }
            redraw(bench, cx);
        });
    })
    .detach();
}

fn failed(bench: &mut Workbench, why: String, cx: &mut Context<Workbench>) {
    if let Some(welcome) = welcome_of(bench) {
        welcome.opening = false;
        welcome.open_error = Some(why);
    }
    redraw(bench, cx);
}

/// Adds what is ticked and counts the setup done, then opens a new chat on the chosen agent in
/// the first project, or just the app when no agent works.
fn finish(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    let agent = start_agent(bench);
    let kind = agent.map(|agent| agent.kind.clone());
    let launcher = agent
        .and_then(|agent| bench.launcher_of_kind(&agent.kind))
        .map(|launcher| launcher.id.clone());
    let ticked: Vec<(String, String)> = welcome
        .ticked()
        .map(|row| (row.path.clone(), row.name.clone()))
        .collect();
    if welcome.opening || ticked.is_empty() {
        return;
    }
    let adding: Vec<_> = ticked
        .into_iter()
        .map(|(path, name)| (name, bench.add_project(&path, cx)))
        .collect();
    if let Some(welcome) = welcome_of(bench) {
        welcome.opening = true;
        welcome.open_error = None;
    }
    redraw(bench, cx);
    let here = Target::here(&bench.asks);
    cx.spawn_in(window, async move |bench, cx| {
        let mut first = None;
        for (name, added) in adding {
            let Some(added) = added.await else {
                let why = t!("{name} couldn't be added as a project.", name = name);
                let _ = bench.update(cx, |bench, cx| failed(bench, why, cx));
                return;
            };
            first.get_or_insert(added);
        }
        let Some(project) = first else {
            return;
        };
        let mut opened = None;
        if let Some(launcher) = launcher {
            let (started, agent) = oneshot::channel();
            here.start_chat(launcher, project.clone(), None, Some(started));
            let Ok(agent) = agent.await else {
                let _ = bench.update(cx, |bench, cx| {
                    failed(bench, t!("The agent did not start.").to_owned(), cx)
                });
                return;
            };
            opened = Some(agent);
        }
        let _ = bench.update(cx, |bench, cx| {
            if opened.is_some() {
                crate::settings::set(bench, PreferenceChange::LastAgentType(kind));
            }
            if let Some(welcome) = welcome_in(bench).filter(|welcome| welcome.required) {
                let skipped = welcome.shortcut == Shortcut::Taken;
                Target::here(&bench.asks)
                    .call_quietly::<call::FirstRunStep>(Step::SetupFinished { skipped });
            }
            leave(bench, cx);
            bench.activate_project(&project);
            if let Some(agent) = opened {
                bench.open_here(OpenTab::new(At::project(&project), PaneKind::Agent, agent));
            }
        });
    })
    .detach();
}

fn keep_press(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

/// What differs between a roomy window and a small one.
#[derive(Clone, Copy)]
struct Room {
    compact: bool,
    width: f32,
    height: f32,
}

impl Room {
    fn of(window: &Window) -> Self {
        let size = window.viewport_size();
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        Self {
            compact: height < COMPACT_HEIGHT || width < COMPACT_WIDTH,
            width,
            height,
        }
    }

    fn pick(self, roomy: f32, compact: f32) -> f32 {
        if self.compact {
            compact
        } else {
            roomy
        }
    }

    /// How far this window is from the smallest it can be towards a roomy one.
    fn ease(self) -> f32 {
        let wide = (self.width - SMALLEST_WINDOW.0) / (ROOMIEST_WINDOW.0 - SMALLEST_WINDOW.0);
        let tall = (self.height - SMALLEST_WINDOW.1) / (ROOMIEST_WINDOW.1 - SMALLEST_WINDOW.1);
        wide.min(tall).clamp(0., 1.)
    }

    fn fit(self, roomy: f32, compact: f32) -> f32 {
        compact + (roomy - compact) * self.ease()
    }

    fn poster_width(self) -> f32 {
        (self.width * self.fit(0.5, 0.4)).round()
    }

    fn pane_width(self) -> f32 {
        self.width - self.poster_width() - 1.
    }

    fn pad(self) -> f32 {
        self.fit(64., 22.)
    }

    fn text_width(self) -> f32 {
        self.poster_width() - self.pad() * 2.
    }

    fn mark_top(self) -> f32 {
        self.fit(70., 30.)
    }

    fn title_size(self) -> f32 {
        self.fit(88., 40.)
    }

    fn primary_height(self) -> f32 {
        self.fit(50., 32.)
    }

    fn poster_bottom(self) -> f32 {
        self.fit(56., 14.)
    }

    fn copy_top(self) -> f32 {
        self.fit(16., 4.)
    }

    fn foot_top(self) -> f32 {
        self.fit(30., 8.)
    }

    /// The room kept for what the poster says, so the welcome above it never moves.
    fn copy_height(self, git_missing: bool) -> f32 {
        let warning = if git_missing {
            self.said_size() * self.said_line() + self.line_gap()
        } else {
            0.
        };
        self.fit(96., 66.) + warning
    }

    /// The welcome, what it says and the row with the button: always this tall.
    fn words_height(self, git_missing: bool) -> f32 {
        self.title_size() * 2.
            + self.copy_top()
            + self.copy_height(git_missing)
            + self.foot_top()
            + self.primary_height()
    }

    fn back_gap(self) -> f32 {
        self.fit(22., 6.)
    }

    fn back_size(self) -> f32 {
        self.fit(17., 13.)
    }

    /// As large as the poster can hold above its words and the way back.
    fn mark_size(self, git_missing: bool) -> f32 {
        let words = self.poster_bottom()
            + self.words_height(git_missing)
            + self.back_gap()
            + self.back_size() * BACK_LINE;
        self.fit(580., 230.)
            .min(self.text_width())
            .min(self.height - self.mark_top() - words)
            .floor()
    }

    fn said_line(self) -> f32 {
        self.fit(1.4, 1.25)
    }

    fn said_size(self) -> f32 {
        self.fit(21., 13.)
    }

    fn mono_size(self) -> f32 {
        self.fit(13., 11.)
    }

    fn line_gap(self) -> f32 {
        self.fit(6., 2.)
    }

    fn row_height(self) -> f32 {
        self.pick(ROW_HEIGHT, ROW_HEIGHT_COMPACT)
    }

    fn list_edge(self) -> f32 {
        self.fit(44., 14.)
    }

    fn list_inset(self) -> f32 {
        self.fit(8., 4.)
    }

    fn list_top(self) -> f32 {
        self.fit(108., 54.)
    }
}

/// How an agent's mark sits in its box: the edges of what is drawn, as shares of the box, and
/// how much of that is ink. Equal targets then read as equal sizes.
fn mark_ink(kind: &str) -> [f32; 5] {
    match kind {
        "claude" => [0.063, 0.063, 0.934, 0.938, 0.324],
        "codex" => [0.125, 0.125, 0.875, 0.875, 0.42],
        "opencode" => [0.199, 0.125, 0.801, 0.875, 0.407],
        "hermes" => [0.156, 0.125, 0.875, 0.875, 0.33],
        "omp" => [0.156, 0.219, 0.844, 0.875, 0.218],
        "grok" => [0.004, 0.004, 0.996, 0.961, 0.232],
        "muse" => [0.125, 0.25, 0.875, 0.75, 0.173],
        "pi" => [0.207, 0.207, 0.793, 0.793, 0.217],
        _ => [0.125, 0.125, 0.875, 0.875, 0.3],
    }
}
/// Where each agent's dither gathers in its tile: its centre and reach, as shares of the tile.
fn dots_anchor(kind: &str) -> [f32; 4] {
    match kind {
        "claude" => [0.56, 0.47, 0.5, 0.45],
        "hermes" => [0.6, 0.34, 0.46, 0.5],
        "grok" => [0.82, 0.62, 0.38, 0.56],
        "pi" => [0.64, 0.56, 0.5, 0.34],
        "codex" => [0.74, 0.42, 0.42, 0.56],
        "muse" => [0.56, 0.5, 0.56, 0.32],
        "omp" => [0.76, 0.72, 0.42, 0.46],
        "opencode" => [0.68, 0.36, 0.5, 0.58],
        _ => [0.66, 0.5, 0.5, 0.5],
    }
}
/// A number of the agent's own, so its field keeps its shape wherever its tile lands.
fn dots_seed(kind: &str) -> f32 {
    kind.bytes()
        .fold(7_u32, |seed, byte| (seed * 31 + u32::from(byte)) % 997) as f32
}

/// How bright `colour` reads next to Claude's orange, so every agent's dots weigh the same.
fn luminance_fit(colour: Hsla) -> f32 {
    let lum = |colour: Rgba| {
        let linear = |channel: f32| {
            if channel <= 0.03928 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(colour.r) + 0.7152 * linear(colour.g) + 0.0722 * linear(colour.b)
    };
    let reference = Rgba {
        r: 217. / 255.,
        g: 119. / 255.,
        b: 87. / 255.,
        a: 1.,
    };
    (lum(reference) / lum(colour.to_rgb()).max(0.001)).sqrt()
}
/// An agent's mark at the size that reads as `target`, its ink flush with the left of a box.
fn sized_mark(kind: &str, target: f32) -> (Div, f32, f32) {
    let [left, top, right, bottom, area] = mark_ink(kind);
    let (ink_width, ink_height) = (right - left, bottom - top);
    let optical = (area * ink_width * ink_height).powf(0.25);
    let tweak = if kind == "opencode" { 0.94 } else { 1. };
    let size = (target / optical * tweak).round();
    let box_height = (target * 1.36).round();
    let box_width = (ink_width * size).ceil();
    let slot = div()
        .relative()
        .flex_none()
        .w(px(box_width))
        .h(px(box_height))
        .child(
            div()
                .absolute()
                .left(px(-left * size))
                .top(px(box_height / 2. - (top + bottom) / 2. * size))
                .child(ui::mark(kind, size)),
        );
    (slot, box_width, box_height)
}

/// What the poster says and offers under the mark on a step.
struct Poster {
    back: Option<AnyElement>,
    lines: Vec<AnyElement>,
    extras: Vec<AnyElement>,
    /// The quiet actions fit beside the button rather than on a line of their own.
    extras_inline: bool,
    primary: AnyElement,
    step: usize,
    /// In a small window the dots give way to a long button.
    dots: bool,
}

fn said(text: impl Into<SharedString>, ink: Hsla, room: Room) -> AnyElement {
    div()
        .text_size(theme::ui_text(room.said_size()))
        .line_height(relative(room.said_line()))
        .text_color(ink)
        .child(text.into())
        .into_any_element()
}

fn said_mono(text: String, room: Room, t: &Tokens) -> AnyElement {
    div()
        .min_w_0()
        .truncate()
        .font_family(theme::MONO)
        .font_features(FontFeatures::disable_ligatures())
        .text_size(theme::ui_text(room.mono_size()))
        .line_height(relative(1.3))
        .text_color(t.text_tertiary)
        .child(text)
        .into_any_element()
}

/// A failure in one short line: its first line, with paths cut to their last part and the
/// agent named as people know it.
fn short_reason(reason: &str, agent: &Found) -> String {
    let line = reason.lines().next().unwrap_or(reason);
    let line = line
        .split_whitespace()
        .map(|word| match word.rsplit_once('/') {
            Some((_, last)) if !last.is_empty() => last,
            _ => word,
        })
        .collect::<Vec<_>>()
        .join(" ")
        .replace("exit status: ", "status ");
    match line.strip_prefix(&format!("{}:", agent.kind)) {
        Some(rest) => format!("{}{rest}", agent.label),
        None => line,
    }
}

/// Whether a small window's foot can hold the dots beside a button of this label.
fn dots_fit(label: &str, closes: bool, room: Room) -> bool {
    let button = label.chars().count() as f32 * 13.5 * 0.5 + 50.;
    let closing = if closes { 66. } else { 0. };
    !room.compact || 43. + closing + button <= room.text_width()
}

fn progress_dots(step: usize, room: Room, t: &Tokens) -> Div {
    let size = room.pick(9., 7.);
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(room.pick(8., 6.)))
        .children((0..STEPS).map(|at| {
            let on = at == step;
            div()
                .h(px(size))
                .w(px(if on { size * 3.1 } else { size }))
                .rounded(px(size))
                .bg(if on { t.text_primary } else { t.border_strong })
        }))
}

/// The left of every step: the mark, the welcome, what is true right now and how to go on.
fn poster(
    poster: Poster,
    resolved: f32,
    welcome: &Welcome,
    room: Room,
    this: &Entity<Workbench>,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx).clone();
    let Poster {
        back,
        mut lines,
        extras,
        extras_inline,
        primary,
        step,
        dots,
    } = poster;
    if welcome.git_missing {
        let warning = if cfg!(target_os = "macos") {
            t!("Git isn't set up. Run xcode-select --install.")
        } else {
            t!("Git isn't installed. The Git view needs it.")
        };
        lines.push(said(warning, t.warn, room));
    }
    let (above, inline) = match extras_inline {
        true => (Vec::new(), extras),
        false => (extras, Vec::new()),
    };
    let dots = dots.then(|| progress_dots(step, room, &t));
    let closes = (!welcome.required).then(|| {
        let this = this.clone();
        let ink = t.text_primary;
        div()
            .id("welcome-close")
            .flex_none()
            .cursor_pointer()
            .text_size(theme::ui_text(room.fit(14., 12.)))
            .text_color(t.text_tertiary)
            .hover(move |close| close.text_color(ink))
            .on_mouse_down(MouseButton::Left, keep_press)
            .on_click(move |_, _, cx| this.update(cx, leave))
            .child(t!("Esc closes"))
    });
    let size = room.mark_size(welcome.git_missing);
    let words = room.words_height(welcome.git_missing);
    let mark = ui::grain::poster_mark(resolved, room.fit(3., 2.), cx).map(|mark| {
        mark.absolute()
            .left(px(((room.poster_width() - size) / 2.).floor()))
            .top(px(room.mark_top()))
            .size(px(size))
    });
    div()
        .relative()
        .flex_none()
        .w(px(room.poster_width()))
        .h_full()
        .overflow_hidden()
        .border_r_1()
        .border_color(t.border)
        .bg(t.surface_sunken)
        .children(mark)
        .children(back.map(|back| {
            div()
                .absolute()
                .left(px(room.pad()))
                .bottom(px(room.poster_bottom() + words + room.back_gap()))
                .child(back)
        }))
        .child(
            div()
                .absolute()
                .left(px(room.pad()))
                .right(px(room.pad()))
                .bottom(px(room.poster_bottom()))
                .h(px(words))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .text_size(theme::ui_text(room.title_size()))
                        .font_weight(FontWeight(650.))
                        .letter_spacing(-0.04)
                        .line_height(relative(1.))
                        .text_color(t.text_primary)
                        .child(t!("Welcome to"))
                        .child(div().text_color(t.accent).child(t!("Sikemux"))),
                )
                .child(
                    div()
                        .flex_none()
                        .mt(px(room.copy_top()))
                        .h(px(room.copy_height(welcome.git_missing)))
                        .flex()
                        .flex_col()
                        .gap(px(room.line_gap()))
                        .children(lines)
                        .when(!above.is_empty(), |copy| {
                            copy.child(div().flex().gap(px(room.fit(14., 10.))).children(above))
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .mt(px(room.foot_top()))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(room.fit(16., 8.)))
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(room.fit(14., 10.)))
                                .children(dots)
                                .children(closes),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(room.fit(14., 8.)))
                                .children(inline)
                                .child(primary),
                        ),
                ),
        )
}

#[allow(clippy::too_many_arguments)]
fn agent_tile(
    bench: &Workbench,
    agent: &Found,
    at: usize,
    keyed: bool,
    room: Room,
    size: (f32, f32),
    spans: bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let welcome_fade = welcome_in(bench).map_or(1., |welcome| welcome.fade.get());
    let state = tile_state(bench, agent, &t);
    let brand = theme::brand(&agent.kind);
    let busy = bench.agent_setup.busy();
    let (edge, left) = (room.fit(40., 10.), room.fit(44., 14.));
    let (name_size, word_size) = (room.fit(30., 16.), room.fit(17., 13.));
    let note = on_free_models(agent).then(|| t!("Free models may keep your prompts"));
    let note_size = room.fit(14., 11.);
    let labels = (name_size + word_size + note.map_or(0., |_| note_size)) * LABEL_LINE;
    let target = room
        .fit(46., 28.)
        .min((size.1 - edge * 2. - labels - MARK_TO_NAME) / MARK_BOX);
    let (slot, slot_width, slot_height) = sized_mark(&agent.kind, target);
    let widest = (agent.label.chars().count() as f32 * name_size)
        .max(state.word.chars().count() as f32 * word_size)
        * 0.56;
    let pill_size = room.fit(15., 12.);
    let pill_label = state.action.as_ref().map(|(label, _)| *label);
    let fit = luminance_fit(brand);
    let density = room.pick(state.density.roomy, state.density.compact);
    let field = density * fit.clamp(LEAST_DENSITY_FIT, 1.25);
    let dots = ui::grain::WallDots {
        colour: brand,
        density: field,
        opacity: match agent.status {
            AgentStatus::Ready { .. } => 0.74,
            AgentStatus::Unknown => 0.6,
            AgentStatus::SignedOut => 0.5,
            _ => 0.44,
        } * fit.min(1.)
            * welcome_fade,
        seed: dots_seed(&agent.kind),
        feather: room.fit(48., 30.),
        anchor: dots_anchor(&agent.kind),
        mark_right: left + slot_width,
        mark_bottom: edge + slot_height,
        label_right: left + widest,
        label_height: edge + labels,
        pill_width: pill_label.filter(|_| !spans).map_or(0., |label| {
            room.fit(36., 10.) + label.len() as f32 * pill_size * 0.6 + room.fit(28., 20.)
        }),
        pill_bottom: pill_label.map_or(0., |_| room.fit(36., 8.) + room.fit(34., 26.)),
        running: state.running,
    };
    let said = match note {
        Some(note) => format!("{}, {}, {note}", agent.label, state.word),
        None => format!("{}, {}", agent.label, state.word),
    };
    let screen_reader = match pill_label {
        Some(label) => format!("{said}, {label}"),
        None => said,
    };
    let pill = state.action.map(|(label, _)| {
        let this = this.clone();
        div()
            .absolute()
            .top(px(room.fit(36., 8.)))
            .right(px(room.fit(36., 10.)))
            .child(
                ui::Button::new(
                    SharedString::from(format!("welcome-step-{}", agent.kind)),
                    label,
                    ui::Variant::Outline,
                )
                .solid(t.text_primary.opacity(0.09), t.text_primary.opacity(0.14))
                .sized(room.fit(34., 26.), room.fit(14., 10.), pill_size)
                .disabled(busy)
                .on_click(move |_, window, cx| {
                    this.update(cx, |bench, cx| {
                        go(bench, Stop::Row(at), window, cx);
                        run_step(bench, at, cx);
                    })
                }),
            )
    });
    let picked = this.clone();
    div()
        .id(SharedString::from(format!("welcome-agent-{}", agent.kind)))
        .role(Role::Button)
        .aria_label(SharedString::from(screen_reader))
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .flex()
        .flex_col()
        .justify_between()
        .pt(px(edge))
        .pb(px(edge))
        .pl(px(left))
        .pr(px(edge))
        .bg(t.surface_sunken)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, keep_press)
        .on_click(move |_, window, cx| {
            picked.update(cx, |bench, cx| go(bench, Stop::Row(at), window, cx))
        })
        .children((field > 0.).then(|| {
            ui::grain::wall_dots(dots, cx)
                .absolute()
                .top_0()
                .left_0()
                .w(px(size.0))
                .h_full()
        }))
        .child(slot)
        .child(
            div()
                .relative()
                .child(
                    div()
                        .text_size(theme::ui_text(name_size))
                        .font_weight(FontWeight::SEMIBOLD)
                        .letter_spacing(-0.02)
                        .line_height(relative(LABEL_LINE))
                        .whitespace_nowrap()
                        .text_color(t.text_primary)
                        .child(agent.label.clone()),
                )
                .child(
                    div()
                        .mt(px(room.fit(3., 1.)))
                        .text_size(theme::ui_text(word_size))
                        .line_height(relative(LABEL_LINE))
                        .whitespace_nowrap()
                        .text_color(state.ink)
                        .child(state.word),
                )
                .children(note.map(|note| {
                    div()
                        .text_size(theme::ui_text(note_size))
                        .line_height(relative(LABEL_LINE))
                        .whitespace_nowrap()
                        .text_color(t.text_secondary)
                        .child(note)
                })),
        )
        .children(pill)
        .when(keyed, |tile| {
            tile.child(
                div()
                    .absolute()
                    .inset_0()
                    .border_2()
                    .border_color(t.border_selected),
            )
        })
        .into_any_element()
}

fn agents_grid(
    bench: &Workbench,
    welcome: &Welcome,
    keyboard: bool,
    room: Room,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let listed = listed_agents(bench);
    let keyed = keyed_tile(bench, welcome)
        .map(|(row, _)| row)
        .filter(|_| keyboard);
    let row_count = listed.len().div_ceil(TILES_ACROSS).max(1);
    let below = match bench.agent_setup.shown() {
        true => room.pick(TERMINAL_HEIGHT, TERMINAL_HEIGHT_COMPACT),
        false => 0.,
    };
    let size = (
        (room.pane_width() - 1.) / TILES_ACROSS as f32,
        (room.height - below - (row_count - 1) as f32) / row_count as f32,
    );
    let mut grid = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(1.))
        .bg(t.border);
    if listed.is_empty() {
        let looking =
            bench.agent_setup.found.is_none().then(|| {
                ui::shimmer::shimmer("welcome-agents-looking", t!("Looking on your PATH…"))
            });
        return grid.child(
            div()
                .flex_1()
                .bg(t.surface_sunken)
                .flex()
                .items_center()
                .pl(px(room.list_edge()))
                .text_size(theme::ui_text(room.fit(16., 13.)))
                .children(looking),
        );
    }
    for (row, agents) in listed.chunks(TILES_ACROSS).enumerate() {
        let spans = agents.len() < TILES_ACROSS;
        let tiles: Vec<AnyElement> = agents
            .iter()
            .enumerate()
            .map(|(column, agent)| {
                let at = row * TILES_ACROSS + column;
                agent_tile(
                    bench,
                    agent,
                    at,
                    keyed == Some(at),
                    room,
                    size,
                    spans,
                    this,
                    cx,
                )
            })
            .collect();
        grid = grid.child(div().flex_1().min_h_0().flex().gap(px(1.)).children(tiles));
    }
    grid
}

fn agents_pane(
    bench: &Workbench,
    welcome: &Welcome,
    keyboard: bool,
    room: Room,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let grid = agents_grid(bench, welcome, keyboard, room, this, cx);
    let terminal = setup::terminal_pane(bench, this, cx);
    let shown = bench.agent_setup.shown();
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(grid)
        .when(shown, |pane| {
            pane.child(
                div()
                    .flex_none()
                    .h(px(room.pick(TERMINAL_HEIGHT, TERMINAL_HEIGHT_COMPACT)))
                    .border_t_1()
                    .border_color(t.border)
                    .bg(t.surface_sunken)
                    .py(px(room.pick(20., 10.)))
                    .px(px(room.list_edge()))
                    .on_mouse_down(MouseButton::Left, keep_press)
                    .child(
                        div()
                            .size_full()
                            .ml(-theme::SPACE_2)
                            .mt(-theme::SPACE_2)
                            .children(terminal),
                    ),
            )
        })
}

fn tick(on: bool, t: &Tokens) -> Div {
    let tick = div()
        .flex_none()
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.));
    match on {
        true => tick
            .bg(t.accent)
            .child(ui::icon(Icon::Check, 12.).text_color(t.on_accent)),
        false => tick.border_1().border_color(t.border_strong),
    }
}

fn project_row(
    at: usize,
    candidate: &Candidate,
    lit: bool,
    room: Room,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let this = this.clone();
    let lift = t.surface_sunken.blend(t.surface_chosen);
    let hovered = t.surface_sunken.blend(t.surface_hover);
    let chats = match candidate.chats {
        0 => String::new(),
        1 => t!("1 chat").to_owned(),
        chats => t!("{count} chats", count = chats),
    };
    let right = |width: f32, text: String, ink: Hsla| {
        div()
            .flex_none()
            .w(px(width))
            .flex()
            .justify_end()
            .whitespace_nowrap()
            .text_size(theme::ui_text(room.fit(14., 12.5)))
            .text_color(ink)
            .child(text)
    };
    let columns = room.fit(30., 24.) + room.fit(86., 60.) + room.fit(84., 62.);
    let longest_name = (room.pane_width() - room.list_edge() * 2. - columns) * NAME_SHARE;
    div()
        .id(SharedString::from(format!("welcome-project-{at}")))
        .flex_none()
        .h(px(room.row_height()))
        .px(px(room.list_edge()))
        .flex()
        .items_center()
        .cursor_pointer()
        .map(|row| match lit {
            true => row.bg(lift),
            false => row.hover(move |row| row.bg(hovered)),
        })
        .on_mouse_down(MouseButton::Left, keep_press)
        .on_click(move |_, window, cx| {
            this.update(cx, |bench, cx| {
                go(bench, Stop::Row(at), window, cx);
                toggle(bench, at, cx);
            })
        })
        .child(
            div()
                .flex_none()
                .w(px(room.fit(30., 24.)))
                .child(tick(candidate.ticked, &t)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(room.fit(14., 8.)))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(longest_name))
                        .truncate()
                        .text_size(theme::ui_text(room.fit(17., 13.5)))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text_primary)
                        .child(candidate.name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(theme::ui_text(room.fit(13., 11.5)))
                        .text_color(t.text_tertiary)
                        .child(candidate.shown.clone()),
                ),
        )
        .child(right(room.fit(86., 60.), chats, t.text_secondary))
        .child(right(
            room.fit(84., 62.),
            candidate.ago.clone().unwrap_or_default(),
            t.text_tertiary,
        ))
        .into_any_element()
}

fn project_list(
    welcome: &Welcome,
    lit: Option<Stop>,
    room: Room,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let visible = visible_rows(welcome);
    let mut rows: Vec<AnyElement> = visible
        .iter()
        .map(|&at| {
            project_row(
                at,
                &welcome.rows[at],
                lit == Some(Stop::Row(at)),
                room,
                this,
                cx,
            )
        })
        .collect();
    let note = |note: AnyElement| {
        div()
            .flex_none()
            .h(px(room.row_height()))
            .px(px(room.list_edge()))
            .flex()
            .items_center()
            .text_size(theme::ui_text(room.fit(15., 12.5)))
            .text_color(t.text_tertiary)
            .child(note)
            .into_any_element()
    };
    if welcome.looking {
        rows.push(note(ui::shimmer::shimmer(
            "welcome-projects-looking",
            t!("Reading your agents' history…"),
        )));
    } else if rows.is_empty() {
        let words = match (welcome.filter.is_empty(), welcome.scan_failed) {
            (false, _) => Some(t!("Nothing here matches. Type a whole path to add it.")),
            (true, true) => Some(t!("Couldn't read your agents' history. Add a folder.")),
            (true, false) => Some(t!(
                "No projects found. Type a path above or choose a folder."
            )),
        };
        rows.extend(words.map(|words| note(words.into_any_element())));
    }
    let inset = room.list_inset();
    let overflows =
        rows.len() as f32 * room.row_height() + inset * 2. + room.list_top() > room.height;
    let thumb_ink = t.border_strong;
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .child(
            div()
                .id("welcome-rows")
                .track_scroll(&welcome.scroll)
                .relative()
                .size_full()
                .py(px(inset))
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .children(rows),
        )
        .when(overflows, |list| {
            list.child(
                Scrollbar::vertical(&welcome.scroll)
                    .mode(ScrollbarMode::Always)
                    .styles(|styles| {
                        let thumb = |style: ScrollbarThumbStyle| {
                            style
                                .width(px(4.))
                                .inset(px(4.))
                                .radius(px(2.))
                                .bg(thumb_ink)
                        };
                        styles
                            .track(|track| track.bg(transparent_black()))
                            .thumb(thumb)
                            .thumb_hover(thumb)
                            .thumb_active(thumb)
                    }),
            )
        })
}

fn projects_pane(
    welcome: &Welcome,
    lit: Option<Stop>,
    room: Room,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let browse = {
        let this = this.clone();
        ui::Button::new("welcome-browse", t!("Choose folder…"), ui::Variant::Outline)
            .solid(t.text_primary.opacity(0.09), t.text_primary.opacity(0.14))
            .sized(room.fit(48., 32.), room.fit(18., 10.), room.fit(16., 12.5))
            .pressed(lit == Some(Stop::Browse))
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| {
                    go(bench, Stop::Browse, window, cx);
                    activate(bench, false, window, cx);
                })
            })
    };
    let typing = welcome
        .path
        .as_ref()
        .is_some_and(|path| !path.read(cx).value().is_empty());
    let list = project_list(welcome, lit, room, this, cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(t.surface_sunken)
        .child(
            div()
                .flex_none()
                .pt(px(room.fit(40., 12.)))
                .pb(px(room.fit(20., 8.)))
                .px(px(room.list_edge()))
                .flex()
                .flex_col()
                .gap(px(room.fit(8., 4.)))
                .on_mouse_down(MouseButton::Left, keep_press)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(room.fit(10., 8.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(room.fit(48., 32.)))
                                .px(px(room.fit(16., 10.)))
                                .flex()
                                .items_center()
                                .rounded(px(room.fit(9., 6.)))
                                .bg(t.surface_raised)
                                .map(|field| match typing {
                                    true => field
                                        .font_family(theme::MONO)
                                        .text_size(theme::ui_text(room.fit(15., 12.5))),
                                    false => field
                                        .font_family(theme::UI)
                                        .text_size(theme::ui_text(room.fit(15., 13.))),
                                })
                                .children(welcome.path.as_ref().map(|path| {
                                    div().flex_1().min_w_0().child(
                                        Input::new(path).appearance(false).bordered(false).px_0(),
                                    )
                                })),
                        )
                        .child(div().flex_none().child(browse)),
                )
                .children(welcome.error.as_ref().map(|error| {
                    div()
                        .text_size(theme::ui_text(room.fit(14., 12.)))
                        .line_height(relative(1.3))
                        .text_color(t.warn)
                        .child(error.clone())
                })),
        )
        .child(list)
}

/// The way back to the agents.
fn back_link(lit: Option<Stop>, room: Room, this: &Entity<Workbench>, t: &Tokens) -> AnyElement {
    let this = this.clone();
    let ink = t.text_primary;
    div()
        .id("welcome-back")
        .role(Role::Button)
        .aria_label(SharedString::from(t!("Back to Agents")))
        .flex()
        .items_center()
        .gap(px(room.fit(9., 6.)))
        .cursor_pointer()
        .text_size(theme::ui_text(room.back_size()))
        .line_height(relative(BACK_LINE))
        .text_color(if lit == Some(Stop::Back) {
            t.text_primary
        } else {
            t.text_secondary
        })
        .hover(move |back| back.text_color(ink))
        .on_mouse_down(MouseButton::Left, keep_press)
        .on_click(move |_, window, cx| {
            this.update(cx, |bench, cx| {
                go(bench, Stop::Back, window, cx);
                activate(bench, false, window, cx);
            })
        })
        .child("‹")
        .child(t!("Agents"))
        .into_any_element()
}

fn projects_poster(
    welcome: &Welcome,
    lit: Option<Stop>,
    room: Room,
    this: &Entity<Workbench>,
    t: &Tokens,
) -> Poster {
    let ticked = welcome.ticked().count();
    let empty = welcome.rows.is_empty() && !welcome.looking;
    let older = if welcome.partial {
        t!("Older ones may be missing.")
    } else {
        ""
    };
    let mut lines = vec![match (ticked > MAX_PROJECTS, empty) {
        (true, _) => said(
            t!(
                "{count} selected. At most {most} can come with you.",
                count = ticked,
                most = MAX_PROJECTS
            ),
            t.warn,
            room,
        ),
        (false, true) => said(t!("Add a folder to continue."), t.text_secondary, room),
        (false, false) => said(
            match older {
                "" => comes_with_you(welcome),
                older => format!("{} {older}", comes_with_you(welcome)),
            },
            t.text_secondary,
            room,
        ),
    }];
    lines.extend(
        welcome
            .open_error
            .as_ref()
            .map(|error| said(error.clone(), t.danger, room)),
    );
    let label = if welcome.opening {
        t!("Opening…")
    } else {
        t!("Open Sikemux")
    };
    let enabled = can_continue_projects(welcome) && !welcome.opening;
    Poster {
        back: Some(back_link(lit, room, this, t)),
        lines,
        extras: Vec::new(),
        extras_inline: true,
        primary: primary_button(label, enabled, room, this, t),
        step: 1,
        dots: dots_fit(label, !welcome.required, room),
    }
}

/// A word in the band that acts, quiet until it is under the keys or the pointer.
fn quiet(
    id: &'static str,
    label: String,
    stop: Stop,
    lit: Option<Stop>,
    room: Room,
    this: &Entity<Workbench>,
    t: &Tokens,
) -> AnyElement {
    let this = this.clone();
    let ink = t.text_primary;
    div()
        .id(id)
        .flex_none()
        .whitespace_nowrap()
        .cursor_pointer()
        .text_size(theme::ui_text(room.pick(16., 13.)))
        .text_color(if lit == Some(stop) {
            t.text_primary
        } else {
            t.text_secondary
        })
        .hover(move |words| words.text_color(ink))
        .on_mouse_down(MouseButton::Left, keep_press)
        .on_click(move |_, window, cx| {
            this.update(cx, |bench, cx| {
                go(bench, stop, window, cx);
                activate(bench, false, window, cx);
            })
        })
        .child(label)
        .into_any_element()
}

fn primary_button(
    label: impl Into<SharedString>,
    enabled: bool,
    room: Room,
    this: &Entity<Workbench>,
    t: &Tokens,
) -> AnyElement {
    let this = this.clone();
    let button = ui::Button::new(
        "welcome-primary",
        label,
        match enabled {
            true => ui::Variant::Primary,
            false => ui::Variant::Outline,
        },
    )
    .sized(
        room.primary_height(),
        room.fit(22., 14.),
        room.fit(18., 13.5),
    )
    .when(!enabled, |button| {
        button.solid(t.surface_raised, t.surface_raised)
    })
    .when(enabled, |button| button.keys(Some("↩".into())))
    .disabled(!enabled)
    .on_click(move |_, window, cx| this.update(cx, |bench, cx| primary(bench, window, cx)));
    button.into_any_element()
}

/// What the poster says about the agents, and the command Return would run on the tile the
/// keys are on.
fn agents_say(
    bench: &Workbench,
    welcome: &Welcome,
    keyboard: bool,
    t: &Tokens,
) -> (String, Option<String>) {
    let progress = &bench.agent_setup;
    let last = pill_row(bench, welcome).and_then(|row| listed_agents(bench).get(row).copied());
    let failed = last.is_some_and(|agent| progress.failure(&agent.kind).is_some());
    let keyed = keyed_tile(bench, welcome)
        .filter(|_| keyboard && !progress.busy())
        .map(|(_, agent)| tile_state(bench, agent, t))
        .filter(|state| state.action.is_some());
    let sentence = match (progress.busy() || (progress.shown() && !failed), last) {
        (true, _) => progress.sheet_state().unwrap_or_default(),
        (false, Some(agent)) if failed => tile_state(bench, agent, t).sentence,
        _ if usable_agents(bench) > 0 => t!("Choose who does the work.").to_owned(),
        _ => match &keyed {
            Some(state) => state.sentence.clone(),
            None => match listed_agents(bench).iter().find(|agent| agent.free_models) {
                Some(free) => t!(
                    "Install or sign in to one agent to continue. {agent} is free with no account.",
                    agent = free.label
                ),
                None => t!("Install or sign in to one agent to continue.").to_owned(),
            },
        },
    };
    let command = keyed.and_then(|state| {
        Some(t!(
            "Return runs: {command}",
            command = state.action?.1.shown_command()
        ))
    });
    (sentence, command)
}

fn agents_poster(
    bench: &Workbench,
    welcome: &Welcome,
    lit: Option<Stop>,
    keyboard: bool,
    room: Room,
    this: &Entity<Workbench>,
    cx: &App,
) -> Poster {
    let t = Tokens::get(cx).clone();
    let progress = &bench.agent_setup;
    let busy = progress.busy();
    let (sentence, command) = agents_say(bench, welcome, keyboard, &t);
    let action = t!("Continue").to_owned();
    let enabled = usable_agents(bench) > 0;
    let reason = pill_row(bench, welcome)
        .and_then(|row| listed_agents(bench).get(row).copied())
        .filter(|_| !progress.shown())
        .and_then(|agent| match &agent.status {
            AgentStatus::Broken { reason } => Some(short_reason(reason, agent)),
            _ => None,
        });
    let check_words = if progress.checking {
        t!("Checking…")
    } else {
        t!("Check again")
    };
    let close_words = if busy { t!("Cancel") } else { t!("Close") };
    let mut quiet_lengths = Vec::new();
    let mut extras = Vec::new();
    if recheck_offered(bench) {
        quiet_lengths.push(check_words.chars().count());
        extras.push(quiet(
            "welcome-check",
            check_words.to_owned(),
            Stop::Recheck,
            lit,
            room,
            this,
            &t,
        ));
    }
    if progress.shown() {
        quiet_lengths.push(close_words.chars().count());
        extras.push(quiet(
            "welcome-sheet-close",
            close_words.to_owned(),
            Stop::SheetClose,
            lit,
            room,
            this,
            &t,
        ));
    }
    if anyway_offered(bench) {
        let anyway = t!("Continue anyway");
        quiet_lengths.push(anyway.chars().count());
        extras.push(quiet(
            "welcome-anyway",
            anyway.to_owned(),
            Stop::Anyway,
            lit,
            room,
            this,
            &t,
        ));
    }
    let mut lines = vec![said(sentence, t.text_secondary, room)];
    if let Some(reason) = reason {
        lines.push(
            div()
                .truncate()
                .text_size(theme::ui_text(room.fit(15., 13.)))
                .line_height(relative(1.3))
                .text_color(t.warn)
                .child(reason)
                .into_any_element(),
        );
    }
    let command_fits = |command: &String| {
        command.chars().count() as f32 * room.mono_size() * MONO_CHAR <= room.text_width()
    };
    lines.extend(
        command
            .filter(command_fits)
            .map(|command| said_mono(command, room, &t)),
    );
    let quiet_words: usize = quiet_lengths.iter().sum();
    let beside_button = quiet_words as f32 * room.fit(16., 13.) * SAID_CHAR
        + quiet_lengths.len() as f32 * room.fit(14., 8.)
        + action.chars().count() as f32 * room.fit(18., 13.5) * SAID_CHAR
        + room.fit(68., 50.)
        + DOTS_WIDTH;
    let extras_inline =
        room.text_width() >= EXTRAS_INLINE_WIDTH && beside_button <= room.text_width();
    Poster {
        back: None,
        lines,
        dots: dots_fit(&action, !welcome.required, room),
        extras,
        extras_inline,
        primary: primary_button(action, enabled, room, this, &t),
        step: 0,
    }
}

/// How far a step of `millis` has come since `since`, from 0 to 1, asking for the next frame
/// while it runs and finishing at once for anyone who asked for less motion.
fn progress(since: Instant, millis: u64, window: &mut Window, cx: &App) -> f32 {
    if cx.reduce_motion() {
        return 1.;
    }
    let share = since.elapsed().as_secs_f32() / Duration::from_millis(millis).as_secs_f32();
    if share < 1. {
        window.request_animation_frame();
    }
    share.min(1.)
}
fn ease_out(share: f32) -> f32 {
    1. - (1. - share) * (1. - share)
}

pub fn render(
    bench: &Workbench,
    this: &Entity<Workbench>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let Some(welcome) = welcome_in(bench) else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let room = Room::of(window);
    let lit = welcome
        .root
        .is_focused(window)
        .then_some(welcome.at)
        .flatten();
    let (resolved, appeared) = match welcome.beat {
        Beat::Resolving(since) => {
            let spent = progress(since, RESOLVE_MILLIS + APPEAR_MILLIS, window, cx)
                * (RESOLVE_MILLIS + APPEAR_MILLIS) as f32;
            (
                ease_out((spent / RESOLVE_MILLIS as f32).min(1.)),
                ((spent - RESOLVE_MILLIS as f32) / APPEAR_MILLIS as f32).clamp(0., 1.),
            )
        }
        Beat::Done => (1., 1.),
    };
    let step_fade = progress(welcome.stage_at, STEP_MILLIS, window, cx).min(ease_out(appeared));
    welcome.fade.set(step_fade);
    let keyboard = welcome.root.is_focused(window) && window.last_input_was_keyboard();
    let (left, pane) = match welcome.stage {
        Stage::Agents => (
            agents_poster(bench, welcome, lit, keyboard, room, this, cx),
            agents_pane(bench, welcome, keyboard, room, this, cx),
        ),
        Stage::Projects => (
            projects_poster(welcome, lit, room, this, &t),
            projects_pane(welcome, lit, room, this, cx),
        ),
    };
    let keys = this.clone();
    let next = this.clone();
    let previous = this.clone();
    let row_down = this.clone();
    let row_up = this.clone();
    div()
        .id("welcome")
        .key_context("Modal Onboarding")
        .track_focus(&welcome.root)
        .on_action(move |_: &Next, window, cx| {
            next.update(cx, |bench, cx| step(bench, true, window, cx))
        })
        .on_action(move |_: &Previous, window, cx| {
            previous.update(cx, |bench, cx| step(bench, false, window, cx))
        })
        .on_action(move |_: &RowDown, window, cx| {
            row_down.update(cx, |bench, cx| step_row(bench, true, window, cx))
        })
        .on_action(move |_: &RowUp, window, cx| {
            row_up.update(cx, |bench, cx| step_row(bench, false, window, cx))
        })
        .absolute()
        .inset_0()
        .flex()
        .overflow_hidden()
        .bg(t.surface_sunken)
        .occlude()
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
        .on_key_down(move |event, window, cx| {
            keys.update(cx, |bench, cx| key_down(bench, event, window, cx))
        })
        .child(poster(left, resolved, welcome, room, this, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .opacity(step_fade)
                .child(pane),
        )
        .into_any_element()
}

/// Whether the agent is offered, or runs, on free models only, whose providers may keep what
/// is sent to them.
fn on_free_models(agent: &Found) -> bool {
    match &agent.status {
        AgentStatus::Missing => agent.free_models,
        AgentStatus::Ready { account } => account.as_deref() == Some(FREE_MODELS),
        _ => false,
    }
}

/// How a tile reads for what its agent is doing, in this screen's own words.
struct TileState {
    word: &'static str,
    ink: Hsla,
    action: Option<(&'static str, Sheet)>,
    density: Density,
    running: bool,
    sentence: String,
}

fn tile_state(bench: &Workbench, agent: &Found, t: &Tokens) -> TileState {
    let label = &agent.label;
    let running = |word, sentence| TileState {
        word,
        ink: t.text_primary,
        action: None,
        density: DENSITY_RUNNING,
        running: true,
        sentence,
    };
    match bench.agent_setup.underway(&agent.kind) {
        Some(Doing::Installing) => {
            let word = match agent.status {
                AgentStatus::Broken { .. } => t!("Reinstalling…"),
                _ => t!("Installing…"),
            };
            return running(word, t!("Downloading {agent}…", agent = label));
        }
        Some(Doing::SigningIn) => {
            return running(
                t!("Waiting for sign-in…"),
                t!("Finish in the terminal below, or the browser if one opened.").to_owned(),
            );
        }
        None => {}
    }
    let next = setup::next_step(bench, agent);
    let failure = bench
        .agent_setup
        .failure(&agent.kind)
        .filter(|_| !setup::usable(&agent.status));
    if let Some(doing) = failure {
        let (word, sentence) = match doing {
            Doing::Installing => (
                t!("Install failed"),
                t!("The installer stopped. Its output is on the right.").to_owned(),
            ),
            Doing::SigningIn => (
                t!("Signed out"),
                t!("Sign-in stopped. Its output is on the right.").to_owned(),
            ),
        };
        return TileState {
            word,
            ink: t.warn,
            action: next.map(|(_, sheet)| (t!("Try again"), sheet)),
            density: DENSITY_FAILED,
            running: false,
            sentence,
        };
    }
    let (word, density, state) = match &agent.status {
        AgentStatus::Ready { account } if account.as_deref() == Some(FREE_MODELS) => (
            t!("Ready · free models"),
            DENSITY_READY,
            t!("{agent} is ready on its free models.", agent = label),
        ),
        AgentStatus::Ready { .. } => (
            t!("Ready"),
            DENSITY_READY,
            t!("{agent} is ready.", agent = label),
        ),
        AgentStatus::Unknown => (
            t!("Installed"),
            DENSITY_INSTALLED,
            t!("{agent} is installed.", agent = label),
        ),
        AgentStatus::Broken { .. } => (
            t!("Won't start"),
            DENSITY_FAILED,
            match next {
                Some(_) => t!("{agent} won't start. Return reinstalls it.", agent = label),
                None => t!(
                    "{agent} won't start. Reinstall it yourself, then Check again.",
                    agent = label
                ),
            },
        ),
        AgentStatus::SignedOut => (
            t!("Signed out"),
            DENSITY_SIGNED_OUT,
            match next {
                Some(_) => t!("Signed out. Return opens {agent}'s sign-in.", agent = label),
                None => t!(
                    "Signed out. Sign in from inside {agent}, then Check again.",
                    agent = label
                ),
            },
        ),
        AgentStatus::Missing if agent.free_models => (
            t!("Not installed · free"),
            DENSITY_MISSING,
            match next {
                Some(_) => t!(
                    "Free with no account. Return installs {agent}.",
                    agent = label
                ),
                None => t!(
                    "Free with no account. Install {agent} yourself, then Check again.",
                    agent = label
                ),
            },
        ),
        AgentStatus::Missing => (
            t!("Not installed"),
            DENSITY_MISSING,
            match next {
                Some(_) => t!("Not installed. Return runs its installer.").to_owned(),
                None => t!(
                    "Not installed. Install {agent} yourself, then Check again.",
                    agent = label
                ),
            },
        ),
    };
    TileState {
        word,
        ink: match &agent.status {
            AgentStatus::Ready { .. } => t.live,
            AgentStatus::Broken { .. } => t.warn,
            _ => t.text_secondary,
        },
        action: next,
        density,
        running: false,
        sentence: state,
    }
}

/// What comes along with what is ticked, as the Projects screen says it.
fn comes_with_you(welcome: &Welcome) -> String {
    let projects = welcome.ticked().count();
    let chats: u32 = welcome.ticked().map(|row| row.chats).sum();
    match (projects, chats) {
        (0, _) => t!("Tick what should come with you.").to_owned(),
        (1, 0) => t!("1 project comes with you.").to_owned(),
        (projects, 0) => t!("{count} projects come with you.", count = projects),
        (_, 1) => t!("1 past chat will appear under Recent.").to_owned(),
        (_, chats) => t!(
            "{count} past chats will appear under Recent.",
            count = chats
        ),
    }
}

fn can_continue_projects(welcome: &Welcome) -> bool {
    (1..=MAX_PROJECTS).contains(&welcome.ticked().count())
}
