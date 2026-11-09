//! The first-run setup over the whole window: an agent that works, the projects to open, then
//! the first thing to get done. On a real first run nothing leaves it but finishing, and only
//! finishing counts it done.

use std::cell::Cell;
use std::f32::consts::FRAC_PI_2;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::base::{Button as BaseButton, ScrollbarMode, ScrollbarThumbStyle};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::Role;
use gpui_kit::{
    actions, div, px, radians, relative, transparent_black, Anchor, AnyElement, App, AppContext,
    Context, Div, Entity, FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement,
    IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement,
    PathPromptOptions, Rgba, ScrollHandle, SharedString, Stateful, StatefulInteractiveElement,
    Styled, Subscription, Transformation, Window, WindowControlArea,
};
use sikemux_core::agent_accounts::status::AgentStatus;
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_core::server::project_roots::{ResolveParams, Resolved};
use sikemux_core::sessions::found_projects::FoundProject;
use sikemux_core::sessions::recent::MAX_PROJECTS;
use sikemux_view::layout::{At, OpenTab, PaneKind};
use sikemux_view::prefs::PreferenceChange;
use tokio::sync::oneshot;

use crate::agents::setup::{self, Doing, Found, Sheet};
use crate::chat::composer::Act;
use crate::chat::SETUP;
use crate::core::{ChatAsk, Target};
use crate::icons::Icon;
use crate::screens::experience::{self, Experience};
use crate::theme::{self, Tokens, TEXT_2XS};
use crate::ui;
use crate::ui::path::{this_home, tilde};
use crate::workbench::Workbench;

const TERMINAL_HEIGHT: f32 = 190.;
const TERMINAL_HEIGHT_COMPACT: f32 = 104.;
/// How often to look again for a starting terminal that can take the keys.
const TERMINAL_WAIT: Duration = Duration::from_millis(50);
const COMPACT_HEIGHT: f32 = 700.;
const COMPACT_WIDTH: f32 = 1100.;
const ROW_HEIGHT: f32 = 40.;
const ROW_HEIGHT_COMPACT: f32 = 32.;
const FIELD_HEIGHT: f32 = 52.;
const FIELD_HEIGHT_COMPACT: f32 = 40.;
const TILES_ACROSS: usize = 4;
const LONG_DRAFT: usize = 80;
/// Below this window width the band's segments narrow so its sentence still fits.
const WIDE_BAND: f32 = 1500.;
const LABEL_LINE: f32 = 1.4;
const COMPACT_REASON_HEIGHT: f32 = 28.;
const FIELD_LEAST_WIDTH: f32 = 140.;
const LOGO_FILL: f32 = 1.55;
const PLACEHOLDER_CHARS: usize = 28;
/// How much dither a tile gets for its state, in a roomy window and a small one.
#[derive(Clone, Copy)]
struct Density {
    roomy: f32,
    compact: f32,
}

const DENSITY_READY: Density = Density {
    roomy: 0.26,
    compact: 0.182,
};
const DENSITY_INSTALLED: Density = Density {
    roomy: 0.15,
    compact: 0.105,
};
const DENSITY_SIGNED_OUT: Density = Density {
    roomy: 0.1,
    compact: 0.07,
};
const DENSITY_MISSING: Density = Density {
    roomy: 0.12,
    compact: 0.,
};
const DENSITY_FAILED: Density = Density {
    roomy: 0.1,
    compact: 0.,
};
const DENSITY_RUNNING: Density = Density {
    roomy: 0.15,
    compact: 0.105,
};
const RISE_MILLIS: u64 = 900;
const FOLD_MILLIS: u64 = 280;
const STEP_MILLIS: u64 = 200;
/// The shortest the opening holds, and the longest it waits for the agents to be known.
const OPENING_LEAST: Duration = Duration::from_millis(1300);
const OPENING_MOST: Duration = Duration::from_millis(3200);
const OPENING_POLL: Duration = Duration::from_millis(100);

/// Where the opening beat has got to.
#[derive(Clone, Copy)]
enum Beat {
    Holding(Instant),
    Folding(Instant),
    Done,
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Agents,
    Projects,
    Start,
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
    Composer,
    ProjectChip,
    AgentChip,
    JustOpen,
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
    /// Holds the keys whenever the path field, the composer or a terminal does not.
    root: FocusHandle,
    /// A real first run, which has no way out but finishing.
    required: bool,
    stage: Stage,
    at: Option<Stop>,
    git_missing: bool,
    rows: Vec<Candidate>,
    looking: bool,
    partial: bool,
    path: Option<Entity<InputState>>,
    error: Option<String>,
    project: Option<String>,
    agent: Option<String>,
    opening: bool,
    /// The composer takes the keys on its first frame, before which it cannot be focused.
    composer_due: bool,
    /// The terminal of a command just started takes the keys once it has been drawn.
    terminal_due: bool,
    /// A command was running as the last frame drew, so its ending can take the keys back.
    was_busy: bool,
    scan_failed: bool,
    /// What is typed in the field, which narrows the list unless it is a path.
    filter: String,
    beat: Beat,
    stage_at: Instant,
    /// The window's room as last drawn, for the composer, which draws apart from the screen.
    room: Cell<Room>,
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
            stage: Stage::Agents,
            at: Some(Stop::Row(0)),
            git_missing: false,
            rows: Vec::new(),
            looking: true,
            partial: false,
            path: None,
            error: None,
            project: None,
            agent: None,
            opening: false,
            composer_due: false,
            terminal_due: false,
            was_busy: false,
            scan_failed: false,
            filter: String::new(),
            beat: Beat::Holding(Instant::now()),
            stage_at: Instant::now(),
            room: Cell::new(ROOMY),
            fade: Cell::new(1.),
            scroll: ScrollHandle::new(),
            _typing: None,
            _activation: None,
        }
    }

    fn ticked(&self) -> impl Iterator<Item = &Candidate> {
        self.rows.iter().filter(|row| row.ticked)
    }

    /// The project the first chat starts in: the one picked while it stays ticked, else the first.
    fn start_project(&self) -> Option<&Candidate> {
        let picked = self.project.as_deref();
        self.ticked()
            .find(|row| Some(row.path.as_str()) == picked)
            .or_else(|| self.ticked().next())
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

/// Draws the screen again, and the composer, which is a view of its own that follows the bench.
fn redraw(bench: &Workbench, cx: &mut Context<Workbench>) {
    bench.parts.redraw(&bench.parts.over, cx);
    cx.notify();
}

/// Looks for the agents here, the folders their history names, and whether git is.
pub fn opened(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.agent_setup.ended = false;
    setup::read(bench, cx);
    cx.spawn(async move |bench, cx| {
        let opened = Instant::now();
        loop {
            cx.background_executor().timer(OPENING_POLL).await;
            let known = bench
                .update(cx, |bench, _| bench.agent_setup.found.is_some())
                .unwrap_or(true);
            if opened.elapsed() >= OPENING_LEAST && (known || opened.elapsed() >= OPENING_MOST) {
                break;
            }
        }
        let _ = bench.update(cx, begin_fold);
    })
    .detach();
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

/// Ends the opening beat, folding it away unless the person asked for less motion.
fn begin_fold(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let still = cx.reduce_motion();
    if let Some(welcome) = welcome_of(bench) {
        if matches!(welcome.beat, Beat::Holding(_)) {
            welcome.beat = if still {
                Beat::Done
            } else {
                Beat::Folding(Instant::now())
            };
            welcome.stage_at = Instant::now();
        }
    }
    redraw(bench, cx);
}

fn opening_is_up(bench: &Workbench) -> bool {
    welcome_in(bench).is_some_and(|welcome| matches!(welcome.beat, Beat::Holding(_)))
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

/// Makes the path field and the composer once a window can hold them, and keeps the keys here.
pub fn take_keys(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    if welcome_in(bench).is_none() {
        return;
    }
    if bench.chats.setup.is_none() {
        let composing = bench.new_composing(SETUP, window, cx);
        composing.draft.update(cx, |draft, cx| {
            draft.set_placeholder("Fix the flaky checkout test…", window, cx)
        });
        bench.chats.setup = Some(composing);
    }
    let composer = composer_focus(bench, cx);
    let terminal = setup::terminal_focus(bench);
    let busy = bench.agent_setup.busy();
    let tile = welcome_in(bench).map_or(0, |welcome| chosen_index(bench, welcome));
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
        let path =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find a folder or type a path"));
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
    if let Beat::Folding(since) = welcome.beat {
        if since.elapsed() >= Duration::from_millis(FOLD_MILLIS) {
            welcome.beat = Beat::Done;
        }
    }
    let finished = std::mem::replace(&mut welcome.was_busy, busy) && !busy;
    if finished && welcome.stage == Stage::Agents {
        welcome.at = Some(Stop::Row(tile));
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
    match (std::mem::take(&mut welcome.composer_due), composer) {
        (true, Some(composer)) => composer.focus(window, cx),
        _ if !welcome.root.contains_focused(window, cx) => welcome.root.focus(window, cx),
        _ => {}
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
    let composer = Some("Onboarding > ChatComposer > Input");
    cx.bind_keys([
        KeyBinding::new("tab", Next, composer),
        KeyBinding::new("shift-tab", Previous, composer),
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

/// Reading the agents again helps only while the chosen one doesn't work and nothing is running.
fn recheck_offered(bench: &Workbench, welcome: &Welcome) -> bool {
    !bench.agent_setup.busy()
        && chosen_agent(bench, welcome).is_some_and(|agent| !setup::usable(&agent.status))
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

/// The agents the first chat can start on: those ready, the one chosen on the first step, or
/// all that were found when none is ready.
fn startable_agents(bench: &Workbench) -> Vec<&Found> {
    let found = found_agents(bench);
    let any_ready = found.iter().any(is_ready);
    let chosen = welcome_in(bench).and_then(|welcome| welcome.agent.as_deref());
    found
        .iter()
        .filter(|agent| {
            is_ready(agent)
                || (setup::usable(&agent.status)
                    && (!any_ready || Some(agent.kind.as_str()) == chosen))
        })
        .filter(|agent| bench.launcher_of_kind(&agent.kind).is_some())
        .collect()
}

/// The tile the first step has chosen: the one picked, else the first ready, else the first
/// installed, else the first.
fn chosen_agent<'a>(bench: &'a Workbench, welcome: &Welcome) -> Option<&'a Found> {
    let listed = listed_agents(bench);
    let picked = welcome.agent.as_deref();
    listed
        .iter()
        .find(|agent| Some(agent.kind.as_str()) == picked)
        .or_else(|| listed.iter().find(|agent| is_ready(agent)))
        .or_else(|| listed.iter().find(|agent| setup::usable(&agent.status)))
        .or_else(|| listed.first())
        .copied()
}

fn chosen_index(bench: &Workbench, welcome: &Welcome) -> usize {
    let chosen = chosen_agent(bench, welcome).map(|agent| agent.kind.as_str());
    listed_agents(bench)
        .iter()
        .position(|agent| Some(agent.kind.as_str()) == chosen)
        .unwrap_or(0)
}

/// The agent the first chat starts on: the one picked, the one a new chat would use anyway, or
/// the first that can.
fn start_agent<'a>(bench: &'a Workbench, welcome: &Welcome) -> Option<&'a Found> {
    let agents = startable_agents(bench);
    let picked = welcome.agent.as_deref();
    let usual = bench.launcher().map(|launcher| launcher.provider.as_str());
    let of = |kind: Option<&str>| {
        agents
            .iter()
            .find(|agent| Some(agent.kind.as_str()) == kind)
    };
    of(picked)
        .or_else(|| of(usual))
        .or_else(|| agents.first())
        .copied()
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
                .chain(recheck_offered(bench, welcome).then_some(Stop::Recheck))
                .chain(anyway_offered(bench).then_some(Stop::Anyway))
                .collect()
        }
        Stage::Projects => [Stop::Field, Stop::Browse]
            .into_iter()
            .chain(visible_rows(welcome).into_iter().map(Stop::Row))
            .chain([Stop::Back])
            .collect(),
        Stage::Start if startable_agents(bench).is_empty() => vec![Stop::JustOpen],
        Stage::Start => vec![
            Stop::Composer,
            Stop::AgentChip,
            Stop::ProjectChip,
            Stop::JustOpen,
        ],
    }
}

fn composer_focus(bench: &Workbench, cx: &App) -> Option<FocusHandle> {
    Some(bench.composing(SETUP)?.draft.read(cx).focus_handle(cx))
}

fn go(bench: &mut Workbench, stop: Stop, window: &mut Window, cx: &mut Context<Workbench>) {
    let tile = match (stop, welcome_in(bench).map(|welcome| welcome.stage)) {
        (Stop::Row(row), Some(Stage::Agents)) => listed_agents(bench)
            .get(row)
            .map(|agent| agent.kind.clone()),
        _ => None,
    };
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    welcome.at = Some(stop);
    if tile.is_some() {
        welcome.agent = tile;
    }
    match (stop, &welcome.path) {
        (Stop::Field, Some(path)) => path.update(cx, |path, cx| path.focus(window, cx)),
        (Stop::Composer, _) => welcome.composer_due = true,
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
        return begin_fold(bench, cx);
    }
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    let focused = |focus: Option<FocusHandle>| focus.is_some_and(|focus| focus.is_focused(window));
    let in_composer = focused(composer_focus(bench, cx));
    let in_terminal = focused(setup::terminal_focus(bench));
    let here = match (in_composer, welcome.stage, welcome.at) {
        (true, ..) => Some(Stop::Composer),
        _ if in_terminal => Some(Stop::Terminal),
        (_, Stage::Agents, Some(Stop::Row(_))) => Some(Stop::Row(chosen_index(bench, welcome))),
        (_, _, at) => at,
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
        return begin_fold(bench, cx);
    }
    let tile = welcome_in(bench).and_then(|welcome| match (welcome.stage, welcome.at) {
        (Stage::Agents, Some(Stop::Row(_))) => Some(chosen_index(bench, welcome)),
        _ => None,
    });
    let Some(at) = tile else {
        return step(bench, down, window, cx);
    };
    let last = listed_agents(bench).len().saturating_sub(1);
    let to = match down {
        true => (at + TILES_ACROSS).min(last),
        false => at.saturating_sub(TILES_ACROSS),
    };
    go(bench, Stop::Row(to), window, cx);
}

fn show_stage(
    bench: &mut Workbench,
    stage: Stage,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let startable = !startable_agents(bench).is_empty();
    let tile = welcome_in(bench).map_or(0, |welcome| chosen_index(bench, welcome));
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    welcome.stage = stage;
    welcome.stage_at = Instant::now();
    welcome.error = None;
    welcome.scroll = ScrollHandle::new();
    let first = match stage {
        Stage::Agents => Stop::Row(tile),
        Stage::Projects => visible_rows(welcome)
            .first()
            .map_or(Stop::Field, |&at| Stop::Row(at)),
        Stage::Start if startable => Stop::Composer,
        Stage::Start => Stop::JustOpen,
    };
    go(bench, first, window, cx);
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
        .and_then(|agent| setup::next_step(bench, agent));
    if let Some((_, sheet)) = next {
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

/// Moves a chip on to its next choice, round its end.
fn cycle(bench: &mut Workbench, stop: Stop, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    let next = |all: Vec<String>, current: Option<String>| {
        let at = all.iter().position(|each| Some(each) == current.as_ref());
        all.get(at.map_or(0, |at| (at + 1) % all.len())).cloned()
    };
    if stop == Stop::ProjectChip {
        let all = welcome.ticked().map(|row| row.path.clone()).collect();
        let chosen = next(all, welcome.start_project().map(|row| row.path.clone()));
        if let Some(welcome) = welcome_of(bench) {
            welcome.project = chosen;
        }
    } else {
        let all = startable_agents(bench)
            .iter()
            .map(|agent| agent.kind.clone())
            .collect();
        let chosen = next(
            all,
            start_agent(bench, welcome).map(|agent| agent.kind.clone()),
        );
        if let Some(welcome) = welcome_of(bench) {
            welcome.agent = chosen;
        }
    }
    redraw(bench, cx);
}

fn primary(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    match welcome.stage {
        Stage::Agents => {
            let tile = chosen_index(bench, welcome);
            let chosen = chosen_agent(bench, welcome).filter(|agent| setup::usable(&agent.status));
            let Some(kind) = chosen.map(|agent| agent.kind.clone()) else {
                return run_step(bench, tile, cx);
            };
            if let Some(welcome) = welcome_of(bench) {
                welcome.agent = Some(kind);
            }
            show_stage(bench, Stage::Projects, window, cx)
        }
        Stage::Projects if can_continue_projects(welcome) => {
            show_stage(bench, Stage::Start, window, cx)
        }
        Stage::Start if startable_agents(bench).is_empty() => finish(bench, false, window, cx),
        Stage::Start => finish(bench, true, window, cx),
        _ => {}
    }
}

/// Space does what a row or chip offers; Return presses a button or moves the setup on.
fn activate(bench: &mut Workbench, space: bool, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    match (welcome.stage, welcome.at) {
        (_, Some(Stop::Anyway)) => show_stage(bench, Stage::Projects, window, cx),
        (_, Some(Stop::Recheck)) => setup::recheck(bench, cx),
        (_, Some(Stop::SheetClose)) => {
            let tile = chosen_index(bench, welcome);
            setup::close(bench, cx);
            go(bench, Stop::Row(tile), window, cx);
        }
        (_, Some(Stop::Browse)) => browse(window, cx),
        (_, Some(Stop::Back)) => back(bench, window, cx),
        (_, Some(Stop::JustOpen)) => finish(bench, false, window, cx),
        (_, Some(stop @ (Stop::ProjectChip | Stop::AgentChip))) if space => cycle(bench, stop, cx),
        (_, Some(Stop::ProjectChip)) => show_stage(bench, Stage::Projects, window, cx),
        (_, Some(Stop::AgentChip)) => show_stage(bench, Stage::Agents, window, cx),
        (Stage::Agents, Some(Stop::Row(_))) => {
            let chosen = chosen_agent(bench, welcome).filter(|agent| setup::usable(&agent.status));
            match (space, chosen) {
                (false, Some(_)) => primary(bench, window, cx),
                (_, None) => run_step(bench, chosen_index(bench, welcome), cx),
                (true, Some(_)) => {}
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
        return begin_fold(bench, cx);
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
    bench.chats.setup = None;
    experience::close(bench, cx);
}

fn path_problem(typed: &str, resolved: &Resolved) -> Option<String> {
    if !Path::new(&resolved.path).is_absolute() {
        return Some("Type the whole path, starting with / or ~".to_owned());
    }
    match resolved.kind.as_deref() {
        Some("dir") => None,
        Some("denied") => Some(format!("Sikemux isn't allowed to read {typed}")),
        Some(_) => Some(format!("{typed} is a file, not a folder")),
        None => Some(format!(
            "Nothing at {typed}. Check the path or use Add folder…"
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
                Err(error) => Some(format!("Could not check that path: {error}")),
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

/// The composer's Return, or its Start.
pub fn send(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    finish(bench, true, window, cx);
}

fn failed(bench: &mut Workbench, why: &str, cx: &mut Context<Workbench>) {
    if let Some(welcome) = welcome_of(bench) {
        welcome.opening = false;
        welcome.error = Some(why.to_owned());
    }
    redraw(bench, cx);
}

/// Adds what is ticked and counts the setup done, then with `start` sends what was written to a
/// new chat on the agent and in the project the chips name.
fn finish(bench: &mut Workbench, start: bool, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    let agent = start_agent(bench, welcome);
    let kind = agent.map(|agent| agent.kind.clone());
    let launcher = agent
        .and_then(|agent| bench.launcher_of_kind(&agent.kind))
        .map(|launcher| launcher.id.clone());
    let written = bench.composing(SETUP).map(|setup| {
        (
            setup.draft.read(cx).value().trim().to_owned(),
            setup.outgoing(cx),
        )
    });
    let chosen = welcome.start_project();
    let chosen_name = chosen.map_or_else(String::new, |row| row.name.clone());
    let chosen = chosen.map(|row| row.path.clone());
    let paths: Vec<String> = welcome.ticked().map(|row| row.path.clone()).collect();
    if welcome.opening || paths.is_empty() {
        return;
    }
    let chat = match (start, launcher, written) {
        (true, Some(launcher), Some((text, outgoing))) if !text.is_empty() => {
            Some((launcher, Some((text, outgoing))))
        }
        (true, ..) => return,
        (false, launcher, _) => launcher.map(|launcher| (launcher, None)),
    };
    let adding: Vec<_> = paths
        .iter()
        .map(|path| (path.clone(), bench.add_project(path, cx)))
        .collect();
    if let Some(welcome) = welcome_of(bench) {
        welcome.opening = true;
        welcome.error = None;
    }
    redraw(bench, cx);
    let here = Target::here(&bench.asks);
    cx.spawn_in(window, async move |bench, cx| {
        let mut first = None;
        let mut picked = None;
        for (path, added) in adding {
            let added = added.await;
            if Some(&path) == chosen.as_ref() {
                picked = added.clone();
            }
            first = first.or(added);
        }
        let project = if start { picked } else { picked.or(first) };
        let Some(project) = project else {
            let why = format!("{chosen_name} couldn't be added as a project.");
            let _ = bench.update(cx, |bench, cx| failed(bench, &why, cx));
            return;
        };
        let mut opened = None;
        if let Some((launcher, prompt)) = chat {
            let (started, agent) = oneshot::channel();
            here.start_chat(launcher, project.clone(), None, Some(started));
            let Ok(agent) = agent.await else {
                let _ = bench.update(cx, |bench, cx| {
                    failed(bench, "The agent did not start.", cx)
                });
                return;
            };
            opened = Some((agent, prompt));
        }
        let _ = bench.update(cx, |bench, cx| {
            if opened.is_some() {
                crate::settings::set(bench, PreferenceChange::LastAgentType(kind));
            }
            leave(bench, cx);
            bench.show_workbench(cx);
            bench.activate_project(&project);
            if let Some((agent, prompt)) = opened {
                bench.open_here(OpenTab::new(
                    At::project(&project),
                    PaneKind::Agent,
                    agent.clone(),
                ));
                if let Some((text, (paths, context))) = prompt {
                    bench.ask_chat(ChatAsk::Prompt {
                        agent,
                        text,
                        paths,
                        context,
                        attach: false,
                    });
                }
            }
        });
    })
    .detach();
}

fn keep_press(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

const ROOMY: Room = Room {
    compact: false,
    width: 1340.,
    height: 880.,
};

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

    fn pad(self) -> f32 {
        self.pick(48., 22.)
    }

    fn bar(self) -> f32 {
        self.pick(52., 38.)
    }

    fn band(self) -> f32 {
        self.pick(64., 48.)
    }

    /// The heading's height, for placing what is measured from the top of the window.
    fn heading(self) -> f32 {
        self.pick(135., 62.)
    }
}

/// The mark filling a `size` box. The icon's own artwork sits inside a wide margin.
fn logo(size: f32, ink: Hsla) -> Div {
    let drawn = size * LOGO_FILL;
    let inset = (size - drawn) / 2.;
    div().relative().flex_none().size(px(size)).child(
        div()
            .absolute()
            .left(px(inset))
            .top(px(inset))
            .child(ui::icon(Icon::Logo, drawn).text_color(ink)),
    )
}

fn title_line(welcome: &Welcome, room: Room, this: &Entity<Workbench>, cx: &App) -> Div {
    let t = Tokens::get(cx).clone();
    let close = (!welcome.required).then(|| {
        let this = this.clone();
        div()
            .id("welcome-close")
            .cursor_pointer()
            .text_size(theme::ui_text(TEXT_2XS))
            .font_weight(FontWeight::NORMAL)
            .text_color(t.text_tertiary)
            .hover(|close| close.text_color(t.text_primary))
            .on_mouse_down(MouseButton::Left, keep_press)
            .on_click(move |_, _, cx| this.update(cx, leave))
            .child("esc to close")
    });
    let git = welcome.git_missing.then(|| {
        div()
            .text_size(theme::ui_text(TEXT_2XS))
            .font_weight(FontWeight::NORMAL)
            .text_color(t.warn)
            .child(if cfg!(target_os = "macos") {
                "Git isn't set up. Run xcode-select --install."
            } else {
                "Git isn't installed. The Git view needs it."
            })
    });
    div()
        .flex_none()
        .h(px(room.bar()))
        .pl(px(room.pick(92., 84.)))
        .pr(px(room.pad()))
        .flex()
        .items_center()
        .gap(px(9.))
        .text_size(theme::ui_text(room.pick(15., 13.)))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.text_primary)
        .child(logo(room.pick(17., 14.), t.text_primary))
        .child("Welcome to")
        .child(div().ml(px(-5.)).text_color(t.accent).child("Sikemux"))
        .child(div().flex_1())
        .children(git)
        .children(close)
}

fn heading(title: &'static str, sub: String, room: Room, t: &Tokens) -> Div {
    div()
        .flex_none()
        .pt(px(room.pick(14., 2.)))
        .px(px(room.pad()))
        .pb(px(room.pick(26., 12.)))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .text_size(theme::ui_text(room.pick(60., 27.)))
                .font_weight(FontWeight::SEMIBOLD)
                .letter_spacing(-0.03)
                .line_height(relative(1.06))
                .text_color(t.text_primary)
                .child(title),
        )
        .child(
            div()
                .mt(px(room.pick(8., 2.)))
                .text_size(theme::ui_text(room.pick(18., 13.)))
                .text_color(t.text_secondary)
                .child(sub),
        )
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
        "claude" => [0.56, 0.47, 0.7, 0.62],
        "hermes" => [0.6, 0.34, 0.46, 0.5],
        "grok" => [0.82, 0.62, 0.38, 0.56],
        "pi" => [0.64, 0.56, 0.5, 0.34],
        "codex" => [0.86, 0.3, 0.3, 0.56],
        "muse" => [0.56, 0.5, 0.56, 0.32],
        "omp" => [0.76, 0.72, 0.42, 0.46],
        "opencode" => [0.68, 0.36, 0.5, 0.58],
        _ => [0.66, 0.5, 0.5, 0.5],
    }
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

#[allow(clippy::too_many_arguments)]
fn agent_tile(
    bench: &Workbench,
    agent: &Found,
    at: usize,
    chosen: bool,
    room: Room,
    bright: (f32, f32),
    column: Option<f32>,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let welcome_fade = welcome_in(bench).map_or(1., |welcome| welcome.fade.get());
    let state = tile_state(bench, agent, &t);
    let brand = theme::brand(&agent.kind);
    let busy = bench.agent_setup.busy();
    let (edge, left) = (room.pick(34., 14.), room.pad());
    let (slot, slot_width, slot_height) = sized_mark(&agent.kind, room.pick(46., 28.));
    let (name_size, word_size) = (room.pick(32., 16.), room.pick(18., 13.));
    let widest = (agent.label.chars().count() as f32 * name_size)
        .max(state.word.chars().count() as f32 * word_size)
        * 0.56;
    let pill_size = room.pick(14.5, 12.);
    let pill_label = state.action.as_ref().map(|(label, _)| *label);
    let fit = luminance_fit(brand);
    let density = room.pick(state.density.roomy, state.density.compact);
    let dots = ui::grain::WallDots {
        colour: brand,
        density: density * fit.min(1.25),
        opacity: if matches!(agent.status, AgentStatus::Ready { .. }) {
            0.74
        } else {
            0.56
        } * fit.min(1.)
            * welcome_fade,
        seed: at as f32 * 13. + 5.,
        feather: room.pick(240., 80.),
        anchor: dots_anchor(&agent.kind),
        mark_right: left + slot_width,
        mark_bottom: edge + slot_height,
        label_right: left + widest,
        label_height: edge + (name_size + word_size) * LABEL_LINE,
        pill_width: pill_label.map_or(0., |label| {
            edge + label.len() as f32 * pill_size * 0.6 + room.pick(26., 20.)
        }),
        pill_bottom: pill_label.map_or(0., |_| room.pick(30., 12.) + room.pick(32., 26.)),
        bright,
        running: state.running,
    };
    let screen_reader = match pill_label {
        Some(label) => format!("{}, {}, {label}", agent.label, state.word),
        None => format!("{}, {}", agent.label, state.word),
    };
    let ground = if chosen {
        t.surface_sunken.blend(t.surface_chosen)
    } else {
        t.surface_sunken
    };
    let hovered = t.surface_sunken.blend(t.surface_hover);
    let pill = state.action.map(|(label, _)| {
        let this = this.clone();
        div()
            .absolute()
            .top(px(room.pick(30., 12.)))
            .right(px(edge))
            .child(
                ui::Button::new(
                    SharedString::from(format!("welcome-step-{}", agent.kind)),
                    label,
                    ui::Variant::Outline,
                )
                .solid(t.text_primary.opacity(0.09), t.text_primary.opacity(0.14))
                .sized(room.pick(32., 26.), room.pick(13., 10.), pill_size)
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
        .map(|tile| match column {
            Some(width) => tile.flex_none().w(px(width)),
            None => tile.flex_1(),
        })
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
        .bg(ground)
        .when(!chosen, |tile| tile.hover(move |tile| tile.bg(hovered)))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, keep_press)
        .on_click(move |_, window, cx| {
            picked.update(cx, |bench, cx| go(bench, Stop::Row(at), window, cx))
        })
        .children((density > 0.).then(|| {
            ui::grain::wall_dots(dots, cx)
                .absolute()
                .inset_0()
                .size_full()
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
                        .mt(px(room.pick(3., 1.)))
                        .text_size(theme::ui_text(word_size))
                        .line_height(relative(LABEL_LINE))
                        .whitespace_nowrap()
                        .text_color(state.ink)
                        .child(state.word),
                ),
        )
        .children(pill)
        .into_any_element()
}

fn agents_wall(
    bench: &Workbench,
    welcome: &Welcome,
    room: Room,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let listed = listed_agents(bench);
    let chosen = chosen_agent(bench, welcome).map(|agent| agent.kind.clone());
    let rows = listed.len().div_ceil(TILES_ACROSS).max(1);
    let terminal = bench.agent_setup.shown();
    let wall_height = room.height
        - room.bar()
        - room.heading()
        - room.band()
        - if terminal {
            room.pick(TERMINAL_HEIGHT, TERMINAL_HEIGHT_COMPACT)
        } else {
            0.
        };
    let tile_height = wall_height / rows as f32;
    let bright = (
        1. / (TILES_ACROSS as f32 * 2.),
        (room.bar() + room.heading() + tile_height / 2.) / room.width,
    );
    let mut wall = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(1.))
        .bg(t.border);
    if bench.agent_setup.found.is_none() {
        return wall.child(
            div()
                .flex_1()
                .bg(t.surface_sunken)
                .flex()
                .items_center()
                .pl(px(room.pad()))
                .text_size(theme::ui_text(room.pick(16., 13.)))
                .child(ui::shimmer::shimmer(
                    "welcome-agents-looking",
                    "Looking on your PATH…",
                )),
        );
    }
    for (row, agents) in listed.chunks(TILES_ACROSS).enumerate() {
        let last = agents.len() - 1;
        let spans = agents.len() < TILES_ACROSS;
        let column_width = (room.width - (TILES_ACROSS - 1) as f32) / TILES_ACROSS as f32;
        let tiles: Vec<AnyElement> = agents
            .iter()
            .enumerate()
            .map(|(column, agent)| {
                let is_chosen = chosen.as_deref() == Some(agent.kind.as_str());
                let at = row * TILES_ACROSS + column;
                let width = (spans && column != last).then_some(column_width);
                agent_tile(bench, agent, at, is_chosen, room, bright, width, this, cx)
            })
            .collect();
        wall = wall.child(div().flex_1().min_h_0().flex().gap(px(1.)).children(tiles));
    }
    wall
}

fn agents_screen(
    bench: &Workbench,
    welcome: &Welcome,
    room: Room,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let wall = agents_wall(bench, welcome, room, this, cx);
    let terminal = setup::terminal_pane(bench, this, cx);
    let shown = bench.agent_setup.shown();
    let reason = compact_reason(bench, welcome, room, &t);
    let broken = broken_reason(bench, welcome, room, &t);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(heading(
            "Choose who does the work",
            "Arrows choose. Return installs, signs in or continues.".to_owned(),
            room,
            &t,
        ))
        .child(wall)
        .when(shown, |main| {
            main.child(
                div()
                    .flex_none()
                    .h(px(room.pick(TERMINAL_HEIGHT, TERMINAL_HEIGHT_COMPACT)))
                    .border_t_1()
                    .border_color(t.border)
                    .bg(t.surface_sunken)
                    .py(px(room.pick(20., 10.)))
                    .px(px(room.pad()))
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
        .children(broken)
        .children(reason)
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
        1 => "1 chat".to_owned(),
        chats => format!("{chats} chats"),
    };
    let right = |width: f32, text: String, ink: Hsla| {
        div()
            .flex_none()
            .w(px(width))
            .flex()
            .justify_end()
            .whitespace_nowrap()
            .text_size(theme::ui_text(room.pick(13.5, 12.5)))
            .text_color(ink)
            .child(text)
    };
    div()
        .id(SharedString::from(format!("welcome-project-{at}")))
        .flex_none()
        .h(px(room.pick(ROW_HEIGHT, ROW_HEIGHT_COMPACT)))
        .px(px(room.pad()))
        .flex()
        .items_center()
        .gap(px(room.pick(18., 10.)))
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
        .child(tick(candidate.ticked, &t))
        .child(
            div()
                .flex_none()
                .w(px(room.pick(360., 190.)))
                .truncate()
                .text_size(theme::ui_text(room.pick(15., 13.5)))
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
                .text_size(theme::ui_text(room.pick(12.5, 11.5)))
                .text_color(t.text_tertiary)
                .child(candidate.shown.clone()),
        )
        .child(right(room.pick(110., 66.), chats, t.text_secondary))
        .child(right(
            room.pick(100., 58.),
            candidate.ago.clone().unwrap_or_default(),
            t.text_tertiary,
        ))
        .into_any_element()
}

fn projects_screen(
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
            .h(px(room.pick(ROW_HEIGHT, ROW_HEIGHT_COMPACT)))
            .px(px(room.pad()))
            .flex()
            .items_center()
            .text_size(theme::ui_text(room.pick(14., 12.5)))
            .text_color(t.text_tertiary)
            .child(note)
            .into_any_element()
    };
    if welcome.looking {
        rows.push(note(ui::shimmer::shimmer(
            "welcome-projects-looking",
            "Reading your agents' history…",
        )));
    } else if rows.is_empty() {
        let words = match (welcome.filter.is_empty(), welcome.scan_failed) {
            (false, _) => Some("Nothing here matches. Type a whole path to add it."),
            (true, true) => Some("Couldn't read your agents' history. Add a folder above."),
            (true, false) => None,
        };
        rows.extend(words.map(|words| note(words.into_any_element())));
    }
    let row_height = room.pick(ROW_HEIGHT, ROW_HEIGHT_COMPACT);
    let area = room.height
        - room.bar()
        - room.heading()
        - room.pick(FIELD_HEIGHT, FIELD_HEIGHT_COMPACT)
        - room.band();
    let free = area - rows.len() as f32 * row_height;
    let sky = (free > row_height).then(|| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(free))
            .child(
                ui::grain::setup_sky(
                    ui::grain::SetupSky {
                        level: 0.7,
                        densest: 0.18,
                        seed: 8.,
                        opacity: 0.42 * welcome.fade.get(),
                        clear: [0.; 4],
                        margin: 0.,
                        feather: 1.,
                    },
                    cx,
                )
                .size_full(),
            )
    });
    let thumb_ink = t.border_strong;
    let count = welcome.rows.len();
    let more = if welcome.partial {
        " Older ones may be missing."
    } else {
        ""
    };
    let sub = match count {
        0 => "Add the folders you work in.".to_owned(),
        1 => format!("1 folder your agents have worked in.{more}"),
        count => format!("{count} folders your agents have worked in.{more}"),
    };
    let browse = {
        let this = this.clone();
        ui::Button::new("welcome-browse", "Add folder…", ui::Variant::Outline)
            .solid(t.text_primary.opacity(0.09), t.text_primary.opacity(0.14))
            .sized(
                room.pick(34., 28.),
                room.pick(13., 9.),
                room.pick(14., 12.5),
            )
            .pressed(lit == Some(Stop::Browse))
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| {
                    go(bench, Stop::Browse, window, cx);
                    activate(bench, false, window, cx);
                })
            })
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(heading("Bring your projects", sub, room, &t))
        .child(
            div()
                .flex_none()
                .h(px(room.pick(FIELD_HEIGHT, FIELD_HEIGHT_COMPACT)))
                .pl(px(room.pad()))
                .pr(px(room.pick(32., 12.)))
                .flex()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(t.border)
                .font_family(theme::MONO)
                .text_size(theme::ui_text(room.pick(14., 12.5)))
                .on_mouse_down(MouseButton::Left, keep_press)
                .children(welcome.path.as_ref().map(|path| {
                    div()
                        .flex_1()
                        .min_w(px(FIELD_LEAST_WIDTH))
                        .child(Input::new(path).appearance(false).bordered(false).px_0())
                }))
                .children(welcome.error.as_ref().map(|error| {
                    div()
                        .min_w_0()
                        .mr(px(16.))
                        .truncate()
                        .font_family(theme::UI)
                        .text_size(theme::ui_text(room.pick(13.5, 12.)))
                        .text_color(t.warn)
                        .child(error.clone())
                }))
                .child(div().font_family(theme::UI).child(browse)),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .children(sky)
                .child(
                    div()
                        .id("welcome-rows")
                        .track_scroll(&welcome.scroll)
                        .relative()
                        .size_full()
                        .flex()
                        .flex_col()
                        .overflow_y_scroll()
                        .children(rows),
                )
                .child(
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
                ),
        )
}

fn start_screen(
    bench: &Workbench,
    welcome: &Welcome,
    room: Room,
    cx: &mut Context<Workbench>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let agent = start_agent(bench, welcome);
    let composer = (!startable_agents(bench).is_empty())
        .then(|| bench.composing(SETUP).map(|setup| setup.view.clone()))
        .flatten();
    let sub = match agent.filter(|_| composer.is_some()) {
        Some(agent) => format!("{} starts on it the moment you press Return.", agent.label),
        None => "Set one up in Settings, or go back to Agents.".to_owned(),
    };
    let title = match composer {
        Some(_) => "What should get done first?",
        None => "No agent yet",
    };
    let top = room.pick(40., 16.);
    let chars = bench
        .composing(SETUP)
        .map_or(0, |setup| setup.draft.read(cx).value().chars().count());
    let size = draft_size(chars, room);
    let text_width =
        (chars.max(PLACEHOLDER_CHARS) as f32 * size * 0.52).min(room.width - room.pad() * 2.);
    let lines = (chars as f32 * size * 0.52 / (room.width - room.pad() * 2.))
        .ceil()
        .max(1.);
    let sky = ui::grain::SetupSky {
        level: 0.68,
        densest: 0.2,
        seed: 2.,
        opacity: 0.42 * welcome.fade.get(),
        clear: [
            room.pad(),
            top,
            room.pad() + text_width,
            top + lines * size * 1.14,
        ],
        margin: room.pick(18., 8.),
        feather: room.pick(160., 60.),
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(heading(title, sub, room, &t))
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .pt(px(top))
                .px(px(room.pad()))
                .pb(px(room.pick(34., 14.)))
                .child(
                    ui::grain::setup_sky(sky, cx)
                        .absolute()
                        .inset_0()
                        .size_full(),
                )
                .children(composer.map(|view| {
                    div()
                        .relative()
                        .on_mouse_down(MouseButton::Left, keep_press)
                        .child(view)
                })),
        )
}

fn draft_size(chars: usize, room: Room) -> f32 {
    match chars > LONG_DRAFT {
        true => room.pick(32., 20.),
        false => room.pick(56., 30.),
    }
}

/// One of the band's three segments, wherever it is on the way.
fn segment(index: usize, now: usize, lit: bool, room: Room, t: &Tokens) -> Stateful<Div> {
    let lift = t.surface_sunken.blend(t.surface_chosen);
    let widths = match (room.compact, room.width < WIDE_BAND) {
        (true, _) => [124., 176., 74.],
        (false, true) => [180., 240., 110.],
        (false, false) => [244., 380., 160.],
    };
    let left = if index == 0 {
        room.pad()
    } else {
        room.pick(28., 14.)
    };
    div()
        .id(SharedString::from(format!("welcome-segment-{index}")))
        .flex_none()
        .w(px(widths[index]))
        .h_full()
        .pl(px(left))
        .pr(px(room.pick(16., 8.)))
        .flex()
        .items_center()
        .gap(px(10.))
        .overflow_hidden()
        .whitespace_nowrap()
        .border_r_1()
        .border_color(t.border)
        .text_size(theme::ui_text(room.pick(17., 13.5)))
        .font_weight(if index <= now {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(if index <= now {
            t.text_primary
        } else {
            t.text_tertiary
        })
        .when(index == now || lit, |segment| segment.bg(lift))
}

fn agent_answer(bench: &Workbench, welcome: &Welcome, room: Room) -> Vec<AnyElement> {
    start_agent(bench, welcome)
        .or_else(|| chosen_agent(bench, welcome))
        .map(|agent| {
            vec![
                ui::mark(&agent.kind, room.pick(20., 16.)),
                div()
                    .truncate()
                    .child(agent.label.clone())
                    .into_any_element(),
            ]
        })
        .unwrap_or_default()
}

fn project_answer(welcome: &Welcome, room: Room, t: &Tokens) -> Vec<AnyElement> {
    let others = welcome.ticked().count().saturating_sub(1);
    let more = (others > 0).then(|| match room.compact {
        true => format!("+{others}"),
        false => format!("and {others} more"),
    });
    let name = welcome
        .start_project()
        .map_or_else(String::new, |row| row.name.clone());
    vec![div()
        .min_w_0()
        .flex()
        .items_baseline()
        .gap(px(4.))
        .child(div().min_w_0().truncate().child(name))
        .children(more.map(|more| {
            div()
                .flex_none()
                .text_size(theme::ui_text(room.pick(13., 11.5)))
                .font_weight(FontWeight::NORMAL)
                .text_color(t.text_secondary)
                .child(more)
        }))
        .into_any_element()]
}

/// A finished step on the Start screen: its answer, opening a menu of the other choices and a
/// way back to that step.
#[allow(clippy::too_many_arguments)]
fn answer_menu(
    index: usize,
    lit: bool,
    answer: Vec<AnyElement>,
    picks: Vec<ui::Pick>,
    chosen: Option<SharedString>,
    back: (&'static str, Stage),
    on_pick: impl Fn(&mut Welcome, &str) + 'static,
    tools: Vec<AnyElement>,
    room: Room,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let trigger = BaseButton::new(SharedString::from(format!("welcome-answer-{index}")))
        .size_full()
        .flex()
        .items_center()
        .gap(px(10.))
        .children(answer)
        .child(
            ui::icon(Icon::Chevron, 10.)
                .flex_none()
                .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                .text_color(t.text_secondary),
        );
    let this = this.clone();
    let on_pick = Rc::new(on_pick);
    let menu = ui::menu_button(
        SharedString::from(format!("welcome-answer-menu-{index}")),
        Anchor::BottomLeft,
        trigger,
        move |mut popup, _, cx| {
            popup = popup.min_w(px(260.));
            for pick in &picks {
                let (this, on_pick, key) = (this.clone(), on_pick.clone(), pick.key.clone());
                popup = popup.item(
                    ui::MenuRow::new(pick.label.clone())
                        .item(cx)
                        .checked(chosen.as_ref() == Some(&pick.key))
                        .on_click(move |_, _, cx| {
                            this.update(cx, |bench, cx| {
                                if let Some(welcome) = welcome_of(bench) {
                                    on_pick(welcome, &key);
                                }
                                redraw(bench, cx);
                            })
                        }),
                );
            }
            let this = this.clone();
            popup
                .separator()
                .item(
                    ui::MenuRow::new(back.0)
                        .item(cx)
                        .on_click(move |_, window, cx| {
                            this.update(cx, |bench, cx| show_stage(bench, back.1, window, cx))
                        }),
                )
        },
    );
    segment(index, 2, lit, room, &t)
        .on_mouse_down(MouseButton::Left, keep_press)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .child(menu),
        )
        .children(tools)
        .into_any_element()
}

/// Attaching files and dictating, for the first task.
fn start_tools(
    bench: &Workbench,
    room: Room,
    this: &Entity<Workbench>,
    t: &Tokens,
) -> Vec<AnyElement> {
    let square = room.pick(36., 30.);
    let raised = |tool: AnyElement| {
        div()
            .flex_none()
            .size(px(square))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .bg(t.surface_raised)
            .child(tool)
            .into_any_element()
    };
    let add = {
        let this = this.clone();
        raised(
            ui::IconButton::new("welcome-add-files", Icon::Plus, "Add files")
                .width(square)
                .height(square)
                .on_click(move |_, window, cx| {
                    this.update(cx, |bench, cx| bench.add_files(SETUP, window, cx))
                })
                .into_any_element(),
        )
    };
    let mic = bench
        .composing(SETUP)
        .map(|setup| raised(div().child(setup.mic.clone()).into_any_element()));
    std::iter::once(add).chain(mic).collect()
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
    let (height, padding, text) = (
        room.pick(44., 34.),
        room.pick(16., 13.),
        room.pick(16., 13.5),
    );
    let button = ui::Button::new(
        "welcome-primary",
        label,
        match enabled {
            true => ui::Variant::Primary,
            false => ui::Variant::Outline,
        },
    )
    .sized(height, padding, text)
    .when(!enabled, |button| {
        button.solid(t.surface_raised, t.surface_raised)
    })
    .when(enabled, |button| button.keys(Some("↩".into())))
    .disabled(!enabled)
    .on_click(move |_, window, cx| this.update(cx, |bench, cx| primary(bench, window, cx)));
    button.into_any_element()
}

/// What is said about the chosen agent, and the command Return would run.
fn agents_say(bench: &Workbench, welcome: &Welcome, t: &Tokens) -> (String, Option<String>) {
    let progress = &bench.agent_setup;
    let chosen = chosen_agent(bench, welcome);
    let state = chosen.map(|agent| tile_state(bench, agent, t));
    let failure = chosen
        .and_then(|agent| progress.failure(&agent.kind))
        .is_some();
    let sentence = match (progress.busy() || (progress.shown() && !failure), &state) {
        (true, _) => progress.sheet_state().unwrap_or_default(),
        (false, Some(state)) => state.sentence.clone(),
        (false, None) => String::new(),
    };
    let command = state
        .filter(|_| !progress.busy())
        .and_then(|state| Some(format!("Return runs: {}", state.action?.1.shown_command())));
    (sentence, command)
}

/// The line a small window has room for, instead of the band's sentence, when the chosen agent
/// isn't ready.
fn compact_reason(bench: &Workbench, welcome: &Welcome, room: Room, t: &Tokens) -> Option<Div> {
    let agent = chosen_agent(bench, welcome)?;
    if !room.compact || tile_state(bench, agent, t).works {
        return None;
    }
    let (sentence, command) = agents_say(bench, welcome, t);
    Some(
        div()
            .flex_none()
            .h(px(COMPACT_REASON_HEIGHT))
            .px(px(room.pad()))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::ui_text(13.))
            .text_color(t.text_secondary)
            .child(div().flex_none().child(sentence))
            .children(command.map(|command| {
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::MONO)
                    .text_size(theme::ui_text(11.5))
                    .text_color(t.text_tertiary)
                    .child(command)
            })),
    )
}

/// Why the chosen agent won't start, in full, where a terminal would otherwise be.
fn broken_reason(
    bench: &Workbench,
    welcome: &Welcome,
    room: Room,
    t: &Tokens,
) -> Option<AnyElement> {
    let agent = chosen_agent(bench, welcome).filter(|_| !bench.agent_setup.shown())?;
    let AgentStatus::Broken { reason } = &agent.status else {
        return None;
    };
    Some(
        div()
            .id("welcome-broken-reason")
            .flex_none()
            .max_h(px(room.pick(TERMINAL_HEIGHT, TERMINAL_HEIGHT_COMPACT)))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(t.border)
            .py(px(room.pick(20., 10.)))
            .px(px(room.pad()))
            .font_family(theme::MONO)
            .text_size(theme::ui_text(room.pick(15., 12.)))
            .line_height(relative(1.65))
            .text_color(t.warn)
            .child(reason.clone())
            .into_any_element(),
    )
}

/// What the band says and offers on the Agents step.
fn agents_band(
    bench: &Workbench,
    welcome: &Welcome,
    lit: Option<Stop>,
    room: Room,
    this: &Entity<Workbench>,
    cx: &App,
) -> (Option<AnyElement>, Vec<AnyElement>, AnyElement) {
    let t = Tokens::get(cx).clone();
    let progress = &bench.agent_setup;
    let chosen = chosen_agent(bench, welcome);
    let chosen_state = chosen.map(|agent| (agent, tile_state(bench, agent, &t)));
    let busy = progress.busy();
    let (sentence, command) = agents_say(bench, welcome, &t);
    let (action, enabled) = match &chosen_state {
        Some((agent, state)) if state.works => (format!("Continue with {}", agent.label), true),
        Some((agent, state)) => match &state.action {
            Some((label, _)) => (action_words(label, &agent.label), !busy),
            None => ("Continue".to_owned(), false),
        },
        None => ("Continue".to_owned(), false),
    };
    let say = (!room.compact).then(|| {
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_size(theme::ui_text(16.))
                    .text_color(t.text_secondary)
                    .child(sentence),
            )
            .children(command.map(|command| {
                div()
                    .max_w_full()
                    .truncate()
                    .font_family(theme::MONO)
                    .text_size(theme::ui_text(11.5))
                    .text_color(t.text_tertiary)
                    .child(command)
            }))
            .into_any_element()
    });
    let mut extras = Vec::new();
    if recheck_offered(bench, welcome) {
        extras.push(quiet(
            "welcome-check",
            if progress.checking {
                "Checking…".to_owned()
            } else {
                "Check again".to_owned()
            },
            Stop::Recheck,
            lit,
            room,
            this,
            &t,
        ));
    }
    if progress.shown() {
        extras.push(quiet(
            "welcome-sheet-close",
            if busy { "Cancel" } else { "Close" }.to_owned(),
            Stop::SheetClose,
            lit,
            room,
            this,
            &t,
        ));
    }
    if anyway_offered(bench) {
        extras.push(quiet(
            "welcome-anyway",
            "Continue anyway".to_owned(),
            Stop::Anyway,
            lit,
            room,
            this,
            &t,
        ));
    }
    (say, extras, primary_button(action, enabled, room, this, &t))
}

/// The bottom band, in the same place on every step: where the setup has got to, what it
/// says, and what comes next.
fn band(
    bench: &Workbench,
    welcome: &Welcome,
    lit: Option<Stop>,
    room: Room,
    this: &Entity<Workbench>,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx).clone();
    let now = match welcome.stage {
        Stage::Agents => 0,
        Stage::Projects => 1,
        Stage::Start => 2,
    };
    let back = |segment: Stateful<Div>, stage: Stage| {
        let this = this.clone();
        segment
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, keep_press)
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| show_stage(bench, stage, window, cx))
            })
    };
    let ticked = welcome.ticked().count();
    let (say, extras, primary): (Option<AnyElement>, Vec<AnyElement>, AnyElement) =
        match welcome.stage {
            Stage::Agents => agents_band(bench, welcome, lit, room, this, cx),
            Stage::Projects => {
                let empty = welcome.rows.is_empty() && !welcome.looking;
                let too_many = ticked > MAX_PROJECTS;
                let line = match (too_many, empty) {
                    (true, _) => div().text_color(t.warn).child(format!(
                        "{ticked} selected. At most {MAX_PROJECTS} can come with you."
                    )),
                    (false, true) => div()
                        .text_color(t.text_secondary)
                        .child("Add a folder to continue."),
                    (false, false) => div()
                        .text_color(t.text_secondary)
                        .child(comes_with_you(welcome)),
                };
                (
                    (!room.compact || too_many || empty).then(|| {
                        line.truncate()
                            .text_size(theme::ui_text(room.pick(16., 13.)))
                            .into_any_element()
                    }),
                    Vec::new(),
                    primary_button("Continue", can_continue_projects(welcome), room, this, &t),
                )
            }
            Stage::Start => {
                let startable = !startable_agents(bench).is_empty();
                let chars = bench.composing(SETUP).map_or(0, |setup| {
                    setup.draft.read(cx).value().trim().chars().count()
                });
                let wanted = welcome.error.is_some() || (chars > 0 && !room.compact);
                let say = (startable && wanted).then(|| {
                    let project = welcome
                        .start_project()
                        .map_or_else(String::new, |row| row.name.clone());
                    div()
                        .truncate()
                        .text_size(theme::ui_text(16.))
                        .text_color(t.text_secondary)
                        .child(match &welcome.error {
                            Some(error) => error.clone(),
                            None => format!("Starts a new chat in {project}."),
                        })
                        .into_any_element()
                });
                match startable {
                    true => (
                        say,
                        vec![quiet(
                            "welcome-just-open",
                            if welcome.opening {
                                "Opening…".to_owned()
                            } else {
                                "Just open Sikemux".to_owned()
                            },
                            Stop::JustOpen,
                            lit,
                            room,
                            this,
                            &t,
                        )],
                        primary_button(
                            if welcome.opening {
                                "Starting…"
                            } else {
                                "Start chat"
                            },
                            chars > 0 && !welcome.opening,
                            room,
                            this,
                            &t,
                        ),
                    ),
                    false => (
                        None,
                        Vec::new(),
                        primary_button("Just open Sikemux", !welcome.opening, room, this, &t),
                    ),
                }
            }
        };
    let first = match now {
        0 => segment(0, 0, false, room, &t)
            .child(div().truncate().child("Agents"))
            .into_any_element(),
        1 => back(
            segment(0, 1, lit == Some(Stop::Back), room, &t),
            Stage::Agents,
        )
        .children(agent_answer(bench, welcome, room))
        .into_any_element(),
        _ if startable_agents(bench).is_empty() => {
            back(segment(0, 2, false, room, &t), Stage::Agents)
                .text_color(t.text_tertiary)
                .font_weight(FontWeight::MEDIUM)
                .child("No agent")
                .child(
                    ui::icon(Icon::Chevron, 10.)
                        .flex_none()
                        .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                        .text_color(t.text_secondary),
                )
                .into_any_element()
        }
        _ => {
            let agents: Vec<ui::Pick> = startable_agents(bench)
                .iter()
                .map(|agent| ui::Pick::new(agent.kind.clone(), agent.label.clone()))
                .collect();
            let chosen =
                start_agent(bench, welcome).map(|agent| SharedString::from(agent.kind.clone()));
            answer_menu(
                0,
                lit == Some(Stop::AgentChip),
                agent_answer(bench, welcome, room),
                agents,
                chosen,
                ("Back to Agents", Stage::Agents),
                |welcome, key| welcome.agent = Some(key.to_owned()),
                Vec::new(),
                room,
                this,
                cx,
            )
        }
    };
    let second = match now {
        0 => segment(1, 0, false, room, &t)
            .child(div().truncate().child("Projects"))
            .into_any_element(),
        1 => segment(1, 1, false, room, &t)
            .child(div().truncate().child("Projects"))
            .into_any_element(),
        _ => {
            let projects: Vec<ui::Pick> = welcome
                .ticked()
                .map(|row| ui::Pick::new(row.path.clone(), row.name.clone()))
                .collect();
            let chosen = welcome
                .start_project()
                .map(|row| SharedString::from(row.path.clone()));
            answer_menu(
                1,
                lit == Some(Stop::ProjectChip),
                project_answer(welcome, room, &t),
                projects,
                chosen,
                ("Back to Projects", Stage::Projects),
                |welcome, key| welcome.project = Some(key.to_owned()),
                if startable_agents(bench).is_empty() {
                    Vec::new()
                } else {
                    start_tools(bench, room, this, &t)
                },
                room,
                this,
                cx,
            )
        }
    };
    div()
        .flex_none()
        .h(px(room.band()))
        .flex()
        .border_t_1()
        .border_color(t.border)
        .bg(t.surface_sunken)
        .child(first)
        .child(second)
        .child(segment(2, now, false, room, &t).child(div().truncate().child("Start")))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(room.pick(24., 8.)))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .children(say),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .pr(px(room.pad()))
                .gap(px(room.pick(14., 8.)))
                .children(extras)
                .child(primary),
        )
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

/// The first beat: the name, large, over the rising sky. When it ends it shrinks and fades
/// towards the title line.
fn opening(rise: f32, fold: f32, room: Room, cx: &App) -> Div {
    let t = Tokens::get(cx).clone();
    let (size, logo) = (room.pick(176., 72.), room.pick(72., 36.));
    let gap = room.pick(34., 14.);
    let block = logo + gap + size * 1.02 * 2.;
    let bottom = room.pick(290., 120.);
    let top = room.height - bottom - block;
    let eased = ease_out(fold);
    let settle = |from: f32, to: f32| from + (to - from) * eased;
    let width = (10. * size * 0.5).min(room.width - room.pad() * 2.);
    let sky = ui::grain::SetupSky {
        level: 0.66 * ease_out(rise),
        densest: 0.3,
        seed: 6.,
        opacity: 0.5 * (1. - eased),
        clear: [0., top, room.pad() + width, top + block],
        margin: room.pick(18., 8.),
        feather: room.pick(280., 110.),
    };
    div()
        .absolute()
        .inset_0()
        .child(
            ui::grain::setup_sky(sky, cx)
                .absolute()
                .inset_0()
                .size_full(),
        )
        .child(
            div()
                .absolute()
                .left(px(settle(room.pad(), room.pick(92., 84.))))
                .top(px(settle(top, room.pick(14., 10.))))
                .flex()
                .flex_col()
                .opacity(1. - eased)
                .child(self::logo(
                    settle(logo, room.pick(17., 14.)),
                    t.text_primary,
                ))
                .child(
                    div()
                        .mt(px(settle(gap, 4.)))
                        .text_size(theme::ui_text(settle(size, room.pick(15., 13.))))
                        .font_weight(FontWeight::SEMIBOLD)
                        .letter_spacing(-0.045)
                        .line_height(relative(1.02))
                        .text_color(t.text_primary)
                        .child("Welcome to")
                        .child(div().text_color(t.accent).child("Sikemux")),
                ),
        )
}

/// The composer that writes the first task: the chat's own, at the size of a headline.
pub fn composer(bench: &mut Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let Some(welcome) = welcome_in(bench) else {
        return div().into_any_element();
    };
    let agent = start_agent(bench, welcome);
    let (provider, label) = agent.map_or((String::new(), String::new()), |agent| {
        (agent.kind.clone(), agent.label.clone())
    });
    let chars = bench
        .composing(SETUP)
        .map_or(0, |setup| setup.draft.read(cx).value().chars().count());
    let room = welcome_in(bench).map_or(ROOMY, |welcome| welcome.room.get());
    let size = draft_size(chars, room);
    let on_send: Act = {
        let this = cx.weak_entity();
        Rc::new(move |window, cx| {
            this.update(cx, |bench, cx| bench.send(SETUP, false, window, cx))
                .ok();
        })
    };
    let Some((composer, offering)) = crate::chat::drafting(
        bench,
        SETUP,
        &provider,
        label,
        (on_send, Rc::new(|_, _| {})),
        cx,
    ) else {
        return div().into_any_element();
    };
    crate::chat::mounted(SETUP, composer.display(size), offering, cx).into_any_element()
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
    welcome.room.set(room);
    let lit = welcome
        .root
        .is_focused(window)
        .then_some(welcome.at)
        .flatten();
    let (rise, fold) = match welcome.beat {
        Beat::Holding(since) => (progress(since, RISE_MILLIS, window, cx), 0.),
        Beat::Folding(since) => (1., progress(since, FOLD_MILLIS, window, cx)),
        Beat::Done => (1., 1.),
    };
    let step_fade = progress(welcome.stage_at, STEP_MILLIS, window, cx).min(ease_out(fold));
    welcome.fade.set(step_fade);
    let screen = match welcome.stage {
        Stage::Agents => agents_screen(bench, welcome, room, this, cx),
        Stage::Projects => projects_screen(welcome, lit, room, this, cx),
        Stage::Start => start_screen(bench, welcome, room, cx),
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
        .flex_col()
        .overflow_hidden()
        .bg(t.surface_sunken)
        .occlude()
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
        .on_key_down(move |event, window, cx| {
            keys.update(cx, |bench, cx| key_down(bench, event, window, cx))
        })
        .when(fold > 0., |page| {
            page.child(
                div()
                    .flex_none()
                    .opacity(ease_out(fold))
                    .child(title_line(welcome, room, this, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .opacity(step_fade)
                    .child(screen),
            )
            .child(
                div()
                    .flex_none()
                    .opacity(ease_out(fold))
                    .child(band(bench, welcome, lit, room, this, cx)),
            )
        })
        .when(fold < 1., |page| page.child(opening(rise, fold, room, cx)))
        .into_any_element()
}

/// How a tile reads for what its agent is doing, in this screen's own words.
struct TileState {
    word: &'static str,
    ink: Hsla,
    action: Option<(&'static str, Sheet)>,
    density: Density,
    running: bool,
    works: bool,
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
        works: false,
        sentence,
    };
    match bench.agent_setup.underway(&agent.kind) {
        Some(Doing::Installing) => {
            let word = match agent.status {
                AgentStatus::Broken { .. } => "Reinstalling…",
                _ => "Installing…",
            };
            return running(word, format!("Downloading {label}…"));
        }
        Some(Doing::SigningIn) => {
            return running(
                "Waiting for sign-in…",
                "Finish in the terminal below, or the browser if one opened.".to_owned(),
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
                "Install failed",
                "The installer stopped. Its output above says why.".to_owned(),
            ),
            Doing::SigningIn => (
                "Signed out",
                "Sign-in stopped. Its output above says why.".to_owned(),
            ),
        };
        return TileState {
            word,
            ink: t.warn,
            action: next.map(|(_, sheet)| ("Try again", sheet)),
            density: DENSITY_FAILED,
            running: false,
            works: false,
            sentence,
        };
    }
    let (word, density, works, state) = match &agent.status {
        AgentStatus::Ready { account } => (
            "Ready",
            DENSITY_READY,
            true,
            match account.as_deref() {
                Some("subscription") => "Signed in with your subscription.".to_owned(),
                Some("apiKey") => "Signed in with an API key.".to_owned(),
                _ => "Signed in and ready.".to_owned(),
            },
        ),
        AgentStatus::Unknown => (
            "Installed",
            DENSITY_INSTALLED,
            true,
            "Installed. Sikemux can't tell if it's signed in.".to_owned(),
        ),
        AgentStatus::Broken { .. } => (
            "Won't start",
            DENSITY_FAILED,
            false,
            match next {
                Some(_) => format!("{label} won't start. Return reinstalls it."),
                None => format!("{label} won't start. Reinstall it yourself, then Check again."),
            },
        ),
        AgentStatus::SignedOut => (
            "Signed out",
            DENSITY_SIGNED_OUT,
            false,
            match next {
                Some(_) => {
                    format!("Signed out. Return opens {label}'s sign-in.")
                }
                None => format!("Signed out. Sign in from inside {label}, then Check again."),
            },
        ),
        AgentStatus::Missing => (
            "Not installed",
            DENSITY_MISSING,
            false,
            match next {
                Some(_) => "Not installed. Return runs its installer.".to_owned(),
                None => format!("Not installed. Install {label} yourself, then Check again."),
            },
        ),
    };
    let sentence = match (works, agent.about.is_empty()) {
        (true, false) => format!("{}. {state}", agent.about),
        _ => state,
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
        works,
        sentence,
    }
}

/// What the main button says when it runs the chosen agent's next step.
fn action_words(label: &str, agent: &str) -> String {
    match label {
        "Install" | "Reinstall" => format!("{label} {agent}"),
        "Sign in" => format!("Sign in to {agent}"),
        other => other.to_owned(),
    }
}

/// What comes along with what is ticked, as the Projects screen says it.
fn comes_with_you(welcome: &Welcome) -> String {
    let projects = welcome.ticked().count();
    let chats: u32 = welcome.ticked().map(|row| row.chats).sum();
    match (projects, chats) {
        (0, _) => "Tick what should come with you.".to_owned(),
        (1, 0) => "1 project comes with you.".to_owned(),
        (projects, 0) => format!("{projects} projects come with you."),
        (_, 1) => "1 past chat will appear under Recent.".to_owned(),
        (_, chats) => format!("{chats} past chats will appear under Recent."),
    }
}

fn can_continue_projects(welcome: &Welcome) -> bool {
    (1..=MAX_PROJECTS).contains(&welcome.ticked().count())
}
