//! The first-run setup over the whole window: an agent that works, the projects to open, then
//! the first thing to get done. On a real first run nothing leaves it but finishing, and only
//! finishing counts it done.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::Role;
use gpui_kit::{
    actions, div, px, relative, AnyElement, App, AppContext, Context, Div, Entity, FocusHandle,
    Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent,
    MouseButton, MouseDownEvent, ParentElement, PathPromptOptions, Pixels, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Subscription, Window,
    WindowControlArea,
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

use crate::agents::setup::{self, Doing, Found, Sheet, SheetStop};
use crate::chat::composer::Act;
use crate::chat::SETUP;
use crate::core::{ChatAsk, Target};
use crate::icons::Icon;
use crate::screens::experience::{self, Experience};
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_SM, TEXT_XS};
use crate::ui;
use crate::ui::path::{this_home, tilde};
use crate::workbench::Workbench;

const TERMINAL_HEIGHT: f32 = 208.;
/// How often to look again for a starting terminal that can take the keys.
const TERMINAL_WAIT: Duration = Duration::from_millis(50);
/// A row's contents start where the text of the path field and the composer does.
const ROW_START: f32 = 18.;
const ROW_END: f32 = 12.;
const TILES_ACROSS: usize = 4;
const TILE_GAP: f32 = 8.;
const TILE_LEAST: f32 = 132.;
/// How far down a tile's dither starts, unless its words reach lower.
const TILE_DITHER_FROM: f32 = 0.6;
const REASON_CHARS: usize = 160;
const DENSITY_READY: f32 = 0.55;
const DENSITY_RUNNING: f32 = 0.5;
const DENSITY_INSTALLED: f32 = 0.42;
const DENSITY_FAINT: f32 = 0.26;
const LIST_MOST: f32 = 420.;
const LIST_LEAST: f32 = 96.;
/// What the title, field and buttons around a list need, so they stay on the screen.
const AROUND_AGENTS: f32 = 180.;
const AROUND_PROJECTS: f32 = 290.;
const SIDE_SHARE: f32 = 0.38;
const SIDE_LEAST: f32 = 340.;
const SIDE_MOST: f32 = 460.;
const STEP_SHARE: f32 = 0.58;
const STEP_LEAST: f32 = 520.;
const STEP_MOST: f32 = 720.;
/// How much of a narrow pane the step may fill, whatever its least width.
const STEP_MOST_SHARE: f32 = 0.9;

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
    SheetAgain,
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
    meta: Option<String>,
    chats: u32,
    ticked: bool,
}

impl Candidate {
    fn found(project: FoundProject, home: Option<&str>) -> Self {
        let ago = ui::time::ago(project.active_ms);
        let meta = match project.chats {
            0 => ago,
            1 => format!("1 chat · {ago}"),
            chats => format!("{chats} chats · {ago}"),
        };
        Self {
            shown: tilde(&project.path, home),
            meta: Some(meta),
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
            meta: None,
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
            cx.new(|cx| InputState::new(window, cx).placeholder("Or type a path, like ~/code/app"));
        let typing =
            cx.subscribe_in(
                &path,
                window,
                |bench, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        if let Some(welcome) = welcome_of(bench) {
                            welcome.error = None;
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
                .chain(progress.sheet_failed().then_some(Stop::SheetAgain))
                .chain(progress.shown().then_some(Stop::SheetClose));
            (0..listed_agents(bench).len())
                .map(Stop::Row)
                .chain(sheet)
                .chain([Stop::Recheck])
                .chain(anyway_offered(bench).then_some(Stop::Anyway))
                .collect()
        }
        Stage::Projects => (0..welcome.rows.len())
            .map(Stop::Row)
            .chain([Stop::Field, Stop::Browse, Stop::Back])
            .collect(),
        Stage::Start if startable_agents(bench).is_empty() => vec![Stop::JustOpen, Stop::Back],
        Stage::Start => vec![
            Stop::Composer,
            Stop::ProjectChip,
            Stop::AgentChip,
            Stop::Back,
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
        welcome.scroll.scroll_to_item(row);
    }
    redraw(bench, cx);
}

fn step(bench: &mut Workbench, forward: bool, window: &mut Window, cx: &mut Context<Workbench>) {
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
    welcome.error = None;
    welcome.scroll = ScrollHandle::new();
    let first = match stage {
        Stage::Agents => Stop::Row(tile),
        Stage::Projects if welcome.rows.is_empty() => Stop::Field,
        Stage::Projects => Stop::Row(0),
        Stage::Start if startable => Stop::Composer,
        Stage::Start => Stop::JustOpen,
    };
    go(bench, first, window, cx);
}

fn back(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    match welcome_in(bench).map(|welcome| welcome.stage) {
        Some(Stage::Projects) => show_stage(bench, Stage::Agents, window, cx),
        Some(Stage::Start) => show_stage(bench, Stage::Projects, window, cx),
        _ => {}
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
        (_, Some(Stop::SheetAgain)) => {
            setup::again(bench, cx);
            hand_terminal_the_keys(bench);
        }
        (_, Some(Stop::SheetClose)) => {
            let tile = chosen_index(bench, welcome);
            setup::close(bench, cx);
            go(bench, Stop::Row(tile), window, cx);
        }
        (_, Some(Stop::Browse)) => browse(window, cx),
        (_, Some(Stop::Back)) => back(bench, window, cx),
        (_, Some(Stop::JustOpen)) => finish(bench, false, window, cx),
        (_, Some(stop @ (Stop::ProjectChip | Stop::AgentChip))) if space => cycle(bench, stop, cx),
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
        None => Some(format!("Nothing at {typed}")),
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

/// The column the setup's name, its steps and the sky stand in.
fn side(stage: Stage, cx: &App) -> Div {
    let t = Tokens::get(cx).clone();
    let at = match stage {
        Stage::Agents => 0,
        Stage::Projects => 1,
        Stage::Start => 2,
    };
    div()
        .relative()
        .flex_none()
        .w(relative(SIDE_SHARE))
        .min_w(px(SIDE_LEAST))
        .max_w(px(SIDE_MOST))
        .h_full()
        .overflow_hidden()
        .border_r_1()
        .border_color(t.border)
        .child(ui::grain::rising(cx).absolute().inset_0().size_full())
        .child(
            div()
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .justify_center()
                .items_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(ui::icon(Icon::Logo, 34.).ml(px(-8.)).text_color(t.accent))
                        .child(
                            div()
                                .mt(px(12.))
                                .mb(px(20.))
                                .text_size(theme::ui_text(44.))
                                .font_weight(FontWeight::BOLD)
                                .letter_spacing(-0.05)
                                .line_height(relative(1.05))
                                .text_color(t.text_primary)
                                .child("Welcome to")
                                .child(div().text_color(t.accent).child("Sikemux")),
                        )
                        .children(["Agents", "Projects", "Start"].into_iter().enumerate().map(
                            |(index, name)| {
                                div()
                                    .py(px(3.))
                                    .text_size(theme::ui_text(TEXT_SM))
                                    .map(|step| match index == at {
                                        true => step
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(t.text_primary),
                                        false => step.text_color(t.text_tertiary),
                                    })
                                    .child(format!("{}  {name}", index + 1))
                            },
                        )),
                ),
        )
}

fn heading(title: &'static str, sub: String, t: &Tokens) -> [Div; 2] {
    [
        div()
            .flex_none()
            .text_size(theme::ui_text(24.))
            .font_weight(FontWeight::BOLD)
            .letter_spacing(-0.025)
            .line_height(relative(1.12))
            .text_color(t.text_primary)
            .child(title),
        div()
            .flex_none()
            .mt(px(6.))
            .mb(px(18.))
            .text_size(theme::ui_text(TEXT_SM))
            .text_color(t.text_secondary)
            .child(sub),
    ]
}

fn row(id: String, lit: bool, last: bool, t: &Tokens) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .flex_none()
        .h(px(42.))
        .flex()
        .items_center()
        .gap(px(10.))
        .pl(px(ROW_START))
        .pr(px(ROW_END))
        .when(!last, |row| row.border_b_1().border_color(t.border))
        .map(|row| match lit {
            true => row.bg(t.surface_chosen),
            false => row.hover(|row| row.bg(t.surface_hover)),
        })
        .on_mouse_down(MouseButton::Left, keep_press)
}

fn note_row(note: AnyElement, t: &Tokens) -> AnyElement {
    div()
        .flex_none()
        .h(px(42.))
        .flex()
        .items_center()
        .pl(px(ROW_START))
        .pr(px(ROW_END))
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary)
        .child(note)
        .into_any_element()
}

/// The step's width in what the side column leaves of the window.
fn step_width(window: &Window) -> Pixels {
    let width = f32::from(window.viewport_size().width);
    let pane = width - (width * SIDE_SHARE).clamp(SIDE_LEAST, SIDE_MOST);
    px((pane * STEP_SHARE)
        .clamp(STEP_LEAST, STEP_MOST)
        .min(pane * STEP_MOST_SHARE))
}

fn list_height(window: &Window, around: f32) -> Pixels {
    (window.viewport_size().height - px(around))
        .min(px(LIST_MOST))
        .max(px(LIST_LEAST))
}

fn listing(
    scroll: &ScrollHandle,
    rows: Vec<AnyElement>,
    height: Pixels,
    under: Option<AnyElement>,
    t: &Tokens,
) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .overflow_hidden()
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_2)
        .bg(t.surface)
        .child(
            div()
                .id("welcome-rows")
                .track_scroll(scroll)
                .max_h(height)
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .children(rows),
        )
        .children(under)
}

fn hint(text: String, t: &Tokens) -> Div {
    div()
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary)
        .child(text)
}

/// How a tile reads for what its agent is doing, in this screen's own words.
struct TileState {
    word: &'static str,
    ink: Hsla,
    action: Option<(&'static str, Sheet)>,
    density: f32,
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
            return running("Installing…", format!("Downloading {label}…"));
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
                "install failed",
                format!("{label} didn't install. Check your connection and try again."),
            ),
            Doing::SigningIn => (
                "signed out",
                format!("{label} didn't sign in. Check your connection and try again."),
            ),
        };
        return TileState {
            word,
            ink: t.danger,
            action: next.map(|(_, sheet)| ("Try again", sheet)),
            density: DENSITY_FAINT,
            running: false,
            works: false,
            sentence,
        };
    }
    let (word, density, works, state) = match &agent.status {
        AgentStatus::Ready { account } => (
            "ready",
            DENSITY_READY,
            true,
            match account.as_deref() {
                Some("subscription") => "Signed in with your subscription.".to_owned(),
                Some("apiKey") => "Signed in with an API key.".to_owned(),
                _ => "Signed in and ready.".to_owned(),
            },
        ),
        AgentStatus::Unknown => (
            "installed",
            DENSITY_INSTALLED,
            true,
            "Installed. Sikemux can't tell if it's signed in.".to_owned(),
        ),
        AgentStatus::Broken { reason } => (
            "won't start",
            DENSITY_FAINT,
            false,
            format!(
                "{label} failed its check: {}",
                reason.chars().take(REASON_CHARS).collect::<String>()
            ),
        ),
        AgentStatus::SignedOut => (
            "signed out",
            DENSITY_FAINT,
            false,
            match next {
                Some(_) => {
                    format!("Signed out. Sikemux opens {label}'s own sign-in in a terminal.")
                }
                None => format!("Signed out. Sign in from inside {label}, then come back."),
            },
        ),
        AgentStatus::Missing => (
            "not installed",
            DENSITY_FAINT,
            false,
            match next {
                Some(_) => "Not installed. Sikemux runs its own installer.".to_owned(),
                None => format!("Not installed. Install {label} yourself, then come back."),
            },
        ),
    };
    let sentence = match agent.about.is_empty() {
        true => state,
        false => format!("{}. {state}", agent.about),
    };
    TileState {
        word,
        ink: t.text_secondary,
        action: next,
        density,
        running: false,
        works,
        sentence,
    }
}

fn tile_size(window: &Window) -> (f32, f32) {
    let across = TILES_ACROSS as f32;
    let width = (f32::from(step_width(window)) - TILE_GAP * (across - 1.)) / across;
    (width, width.max(TILE_LEAST))
}

#[allow(clippy::too_many_arguments)]
fn agent_tile(
    bench: &Workbench,
    agent: &Found,
    at: usize,
    chosen: bool,
    (width, height): (f32, f32),
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let brand = theme::brand(&agent.kind);
    let state = tile_state(bench, agent, &t);
    let busy = bench.agent_setup.busy();
    let top = (height * 0.125).max(14.);
    let mark = (height * 0.21).clamp(26., 40.);
    let gap = (height * 0.055).clamp(7., 10.);
    let words = 30.;
    let from = ((top + mark + gap + words + 4.) / height).max(TILE_DITHER_FROM);
    let rest = t.surface_raised;
    let ground = if chosen {
        rest.blend(t.surface_chosen)
    } else {
        rest
    };
    let hovered = rest.blend(t.surface_hover);
    let pill_ground = ground.blend(t.surface_chosen);
    let pill_hovered = pill_ground.blend(t.surface_hover);
    let screen_reader = match &state.action {
        Some((label, _)) => format!("{}, {}, {label}", agent.label, state.word),
        None => format!("{}, {}", agent.label, state.word),
    };
    let action = state.action.map(|(label, _)| {
        let this = this.clone();
        div().mt(px(gap)).relative().child(
            ui::Button::new(
                SharedString::from(format!("welcome-step-{}", agent.kind)),
                label,
                ui::Variant::Outline,
            )
            .solid(pill_ground, pill_hovered)
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
        .flex_none()
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .rounded(theme::RADIUS_3)
        .bg(ground)
        .when(!chosen, |tile| tile.hover(move |tile| tile.bg(hovered)))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, keep_press)
        .on_click(move |_, window, cx| {
            picked.update(cx, |bench, cx| go(bench, Stop::Row(at), window, cx))
        })
        .child(
            ui::grain::tile(brand, state.density, from, state.running, cx)
                .absolute()
                .inset_0()
                .size_full(),
        )
        .child(
            div()
                .relative()
                .size_full()
                .pt(px(top))
                .flex()
                .flex_col()
                .items_center()
                .child(ui::mark(&agent.kind, mark))
                .child(
                    ui::text(agent.label.clone(), TEXT_SM, FontWeight::SEMIBOLD)
                        .mt(px(gap))
                        .text_color(t.text_primary),
                )
                .child(
                    div()
                        .mt(px(1.))
                        .text_size(theme::ui_text(TEXT_2XS))
                        .text_color(state.ink)
                        .child(state.word),
                )
                .children(action),
        )
        .into_any_element()
}

fn primary_button(
    label: impl Into<SharedString>,
    enabled: bool,
    this: &Entity<Workbench>,
) -> impl IntoElement {
    let this = this.clone();
    ui::Button::new(
        "welcome-primary",
        label,
        match enabled {
            true => ui::Variant::Primary,
            false => ui::Variant::Outline,
        },
    )
    .disabled(!enabled)
    .on_click(move |_, window, cx| this.update(cx, |bench, cx| primary(bench, window, cx)))
}

fn stop_button(
    id: &'static str,
    label: &'static str,
    stop: Stop,
    lit: Option<Stop>,
    this: &Entity<Workbench>,
) -> impl IntoElement {
    let this = this.clone();
    ui::Button::new(id, label, ui::Variant::Outline)
        .pressed(lit == Some(stop))
        .on_click(move |_, window, cx| {
            this.update(cx, |bench, cx| {
                go(bench, stop, window, cx);
                activate(bench, false, window, cx);
            })
        })
}

/// A way forward that is only words, until it is under the keys or the pointer.
fn quiet(
    id: &'static str,
    label: String,
    stop: Stop,
    lit: Option<Stop>,
    this: &Entity<Workbench>,
    t: &Tokens,
) -> Stateful<Div> {
    let this = this.clone();
    let ink = t.text_primary;
    div()
        .id(id)
        .flex_none()
        .cursor_pointer()
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(if lit == Some(stop) {
            t.text_primary
        } else {
            t.text_tertiary
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
}

fn foot() -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(10.))
        .mt(px(16.))
}

/// One line that keeps its place, so an error coming and going moves nothing else.
fn message_line(error: Option<String>, note: Option<&str>, t: &Tokens) -> AnyElement {
    let line = div()
        .flex_none()
        .mt(SPACE_1H)
        .h(px(14.))
        .truncate()
        .text_size(theme::ui_text(TEXT_2XS));
    match (error, note) {
        (Some(error), _) => line.text_color(t.danger).child(error),
        (None, Some(note)) => line.text_color(t.text_tertiary).child(note.to_owned()),
        _ => line,
    }
    .into_any_element()
}

/// What the main button says when it runs the chosen agent's next step.
fn action_words(label: &str, agent: &str) -> String {
    match label {
        "Install" | "Reinstall" => format!("{label} {agent}"),
        "Sign in" => format!("Sign in to {agent}"),
        other => other.to_owned(),
    }
}

fn agents_screen(
    bench: &Workbench,
    welcome: &Welcome,
    lit: Option<Stop>,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let listed = listed_agents(bench);
    let chosen = chosen_agent(bench, welcome);
    let size = tile_size(window);
    let tiles: Vec<AnyElement> = match &bench.agent_setup.found {
        None => vec![ui::shimmer::shimmer(
            "welcome-agents-looking",
            "Looking on your PATH…",
        )],
        Some(found) if found.is_empty() => vec![
            "None found. Install Claude Code, Codex or another supported CLI.".into_any_element(),
        ],
        Some(_) => listed
            .iter()
            .enumerate()
            .map(|(at, agent)| {
                let is_chosen = chosen.is_some_and(|chosen| chosen.kind == agent.kind);
                agent_tile(bench, agent, at, is_chosen, size, this, cx)
            })
            .collect(),
    };
    let sheet_lit = match lit {
        Some(Stop::SheetAgain) => Some(SheetStop::Again),
        Some(Stop::SheetClose) => Some(SheetStop::Close),
        _ => None,
    };
    let terminal = setup::inline(bench, this, px(TERMINAL_HEIGHT), sheet_lit, cx).map(|terminal| {
        div()
            .flex_none()
            .mt(px(TILE_GAP))
            .overflow_hidden()
            .rounded(theme::RADIUS_3)
            .bg(t.surface_raised)
            .on_mouse_down(MouseButton::Left, keep_press)
            .child(terminal)
            .into_any_element()
    });
    let around = AROUND_AGENTS + terminal.as_ref().map_or(0., |_| TERMINAL_HEIGHT + TILE_GAP);
    let [title, sub] = heading(
        "Choose who does the work",
        "Pick one that works. It starts your first chat.".to_owned(),
        &t,
    );
    let chosen_state = chosen.map(|agent| (agent, tile_state(bench, agent, &t)));
    let sentence = chosen_state
        .as_ref()
        .map(|(_, state)| state.sentence.clone())
        .unwrap_or_default();
    let command = chosen_state.as_ref().and_then(|(_, state)| {
        let (_, sheet) = state.action.as_ref()?;
        Some(format!("Return runs: {}", sheet.shown_command()))
    });
    let busy = bench.agent_setup.busy();
    let (action, enabled) = match &chosen_state {
        Some((agent, state)) if state.works => (format!("Continue with {}", agent.label), true),
        Some((agent, state)) => match &state.action {
            Some((label, _)) => (action_words(label, &agent.label), !busy),
            None => ("Continue".to_owned(), false),
        },
        None => ("Continue".to_owned(), false),
    };
    let checking = bench.agent_setup.checking;
    let grid = div()
        .id("welcome-rows")
        .track_scroll(&welcome.scroll)
        .flex_none()
        .max_h(list_height(window, around))
        .flex()
        .flex_wrap()
        .gap(px(TILE_GAP))
        .overflow_y_scroll()
        .children(tiles);
    [
        title.into_any_element(),
        sub.into_any_element(),
        grid.into_any_element(),
    ]
    .into_iter()
    .chain(terminal)
    .chain([foot()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(sentence)
                .children(command.map(|command| {
                    div()
                        .font_family(theme::MONO)
                        .text_size(theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(command)
                })),
        )
        .child(quiet(
            "welcome-check",
            if checking {
                "Checking…".to_owned()
            } else {
                "Check again".to_owned()
            },
            Stop::Recheck,
            lit,
            this,
            &t,
        ))
        .when(anyway_offered(bench), |foot| {
            foot.child(stop_button(
                "welcome-anyway",
                "Continue anyway",
                Stop::Anyway,
                lit,
                this,
            ))
        })
        .child(primary_button(action, enabled, this))
        .into_any_element()])
    .collect()
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

fn projects_screen(
    welcome: &Welcome,
    lit: Option<Stop>,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let count = welcome.rows.len();
    let name_shared = |name: &str| welcome.rows.iter().filter(|row| row.name == name).count() > 1;
    let mut rows: Vec<AnyElement> = welcome
        .rows
        .iter()
        .enumerate()
        .map(|(at, candidate)| {
            let this = this.clone();
            row(
                format!("welcome-project-{at}"),
                lit == Some(Stop::Row(at)),
                at + 1 == count && !welcome.looking,
                &t,
            )
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| {
                    go(bench, Stop::Row(at), window, cx);
                    toggle(bench, at, cx);
                })
            })
            .child(
                Checkbox::new(SharedString::from(format!("welcome-tick-{at}")))
                    .accessibility_label(candidate.name.clone())
                    .checked(candidate.ticked),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(SPACE_2)
                    .child(
                        ui::text(candidate.name.clone(), TEXT_SM, FontWeight::MEDIUM)
                            .flex_none()
                            .text_color(t.text_primary),
                    )
                    .when(name_shared(&candidate.name), |line| {
                        line.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(theme::MONO)
                                .text_size(theme::ui_text(TEXT_2XS))
                                .text_color(t.text_tertiary)
                                .child(candidate.shown.clone()),
                        )
                    }),
            )
            .children(candidate.meta.clone().map(|meta| {
                div()
                    .flex_none()
                    .text_size(theme::ui_text(TEXT_2XS))
                    .text_color(t.text_secondary)
                    .child(meta)
            }))
            .into_any_element()
        })
        .collect();
    if welcome.looking {
        rows.push(note_row(
            ui::shimmer::shimmer("welcome-projects-looking", "Reading your agents' history…"),
            &t,
        ));
    } else if rows.is_empty() {
        let note = match welcome.scan_failed {
            true => "Couldn't read your agents' history. Add a folder below.",
            false => "No saved chats yet. Choose or create a folder.",
        };
        rows.push(note_row(note.into_any_element(), &t));
    }
    let ticked = welcome.ticked().count();
    let [title, sub] = heading("Bring your projects", comes_with_you(welcome), &t);
    let field = welcome.path.as_ref().map(|path| {
        div()
            .flex_1()
            .min_w_0()
            .on_mouse_down(MouseButton::Left, keep_press)
            .child(ui::Field::new(path).mono().small())
    });
    vec![
        title.into_any_element(),
        sub.into_any_element(),
        listing(
            &welcome.scroll,
            rows,
            list_height(window, AROUND_PROJECTS),
            None,
            &t,
        )
        .into_any_element(),
        div()
            .flex_none()
            .mt(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .children(field)
            .child(stop_button(
                "welcome-browse",
                "Add folder…",
                Stop::Browse,
                lit,
                this,
            ))
            .into_any_element(),
        message_line(
            welcome.error.clone(),
            welcome.partial.then_some(
                "Only your newest history was read, so some older folders may be missing.",
            ),
            &t,
        ),
        foot()
            .child(stop_button("welcome-back", "Back", Stop::Back, lit, this))
            .child(hint(
                match ticked {
                    0 => "Tick or add a project to continue".to_owned(),
                    ticked if ticked > MAX_PROJECTS => {
                        format!("{ticked} selected. At most {MAX_PROJECTS} can come with you")
                    }
                    ticked => format!("{ticked} selected"),
                },
                &t,
            ))
            .child(div().flex_1())
            .child(primary_button(
                "Continue",
                can_continue_projects(welcome),
                this,
            ))
            .into_any_element(),
    ]
}

fn start_screen(
    bench: &Workbench,
    welcome: &Welcome,
    lit: Option<Stop>,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let composer = (!startable_agents(bench).is_empty())
        .then(|| bench.composing(SETUP).map(|setup| setup.view.clone()))
        .flatten();
    let sub = match composer {
        Some(_) => "Your agent starts on it the moment you press Return.",
        None => "No agent is ready yet. You can set one up later in Settings.",
    };
    let [title, sub] = heading("What should get done first?", sub.to_owned(), &t);
    let opening = if welcome.opening {
        "Opening…"
    } else {
        "Just open Sikemux"
    };
    let foot = foot().child(stop_button("welcome-back", "Back", Stop::Back, lit, this));
    let foot = match composer.is_some() {
        true => foot.child(div().flex_1()).child(quiet(
            "welcome-just-open",
            opening.to_owned(),
            Stop::JustOpen,
            lit,
            this,
            &t,
        )),
        false => foot.child(div().flex_1()).child(primary_button(
            "Just open Sikemux",
            !welcome.opening,
            this,
        )),
    };
    [title.into_any_element(), sub.into_any_element()]
        .into_iter()
        .chain(composer.map(|view| {
            div()
                .flex_none()
                .mx(px(-16.))
                .mb(px(-16.))
                .on_mouse_down(MouseButton::Left, keep_press)
                .child(view)
                .into_any_element()
        }))
        .chain([
            message_line(welcome.error.clone(), None, &t),
            foot.into_any_element(),
        ])
        .collect()
}

/// A chip under the first task's text, grey-lifted while the keys stand on it.
#[allow(clippy::too_many_arguments)]
fn chip(
    id: &'static str,
    lit: bool,
    lead: Option<AnyElement>,
    shown: String,
    picks: Vec<ui::Pick>,
    chosen: Option<SharedString>,
    on_pick: impl Fn(&mut Welcome, &str) + 'static,
    this: &Entity<Workbench>,
    t: &Tokens,
    cx: &App,
) -> Div {
    let this = this.clone();
    div()
        .min_w_0()
        .rounded(theme::RADIUS_2)
        .when(lit, |chip| chip.bg(t.surface_chosen))
        .child(ui::select_led(
            id,
            lead,
            shown,
            picks,
            chosen,
            false,
            move |key, _, cx| {
                this.update(cx, |bench, cx| {
                    if let Some(welcome) = welcome_of(bench) {
                        on_pick(welcome, key);
                    }
                    redraw(bench, cx);
                })
            },
            cx,
        ))
}

/// The chat's own composer, starting on the agent and in the project its chips name.
pub fn composer(bench: &mut Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let this = cx.entity();
    let t = Tokens::get(cx).clone();
    let Some(welcome) = welcome_in(bench) else {
        return div().into_any_element();
    };
    let agent = start_agent(bench, welcome);
    let (provider, label) = agent.map_or((String::new(), String::new()), |agent| {
        (agent.kind.clone(), agent.label.clone())
    });
    let projects: Vec<ui::Pick> = welcome
        .ticked()
        .map(|row| ui::Pick::new(row.path.clone(), row.name.clone()))
        .collect();
    let agents: Vec<ui::Pick> = startable_agents(bench)
        .iter()
        .map(|agent| ui::Pick::new(agent.kind.clone(), agent.label.clone()))
        .collect();
    let project = welcome.start_project();
    let project_chip = chip(
        "welcome-project-chip",
        welcome.at == Some(Stop::ProjectChip),
        None,
        project.map_or_else(String::new, |row| row.name.clone()),
        projects,
        project.map(|row| row.path.clone().into()),
        |welcome, key| welcome.project = Some(key.to_owned()),
        &this,
        &t,
        cx,
    );
    let agent_chip = chip(
        "welcome-agent-chip",
        welcome.at == Some(Stop::AgentChip),
        Some(ui::mark(&provider, 13.).into_any_element()),
        label.clone(),
        agents,
        Some(provider.clone().into()),
        |welcome, key| welcome.agent = Some(key.to_owned()),
        &this,
        &t,
        cx,
    );
    let opening = welcome.opening;
    let on_send: Act = {
        let this = this.downgrade();
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
    let composer = composer
        .leading(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(project_chip)
                .child(agent_chip)
                .into_any_element(),
        )
        .send_label(if opening { "Starting…" } else { "Start" });
    crate::chat::mounted(SETUP, composer, offering, cx).into_any_element()
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
    let lit = welcome
        .root
        .is_focused(window)
        .then_some(welcome.at)
        .flatten();
    let screen = match welcome.stage {
        Stage::Agents => agents_screen(bench, welcome, lit, this, window, cx),
        Stage::Projects => projects_screen(welcome, lit, this, window, cx),
        Stage::Start => start_screen(bench, welcome, lit, this, cx),
    };
    let close = (!welcome.required).then(|| {
        let this = this.clone();
        div()
            .id("welcome-close")
            .ml_auto()
            .cursor_pointer()
            .hover(|close| close.text_color(t.text_primary))
            .on_mouse_down(MouseButton::Left, keep_press)
            .on_click(move |_, _, cx| this.update(cx, leave))
            .child("esc to close")
    });
    let foot_line = (welcome.git_missing || close.is_some()).then(|| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .px(px(22.))
            .pb(SPACE_3)
            .font_family(theme::KBD)
            .text_size(theme::ui_text(TEXT_2XS))
            .text_color(t.text_tertiary)
            .when(welcome.git_missing, |line| {
                line.child(
                    div()
                        .text_color(t.warn)
                        .child(if cfg!(target_os = "macos") {
                            "Git isn't set up. Run xcode-select --install."
                        } else {
                            "Git isn't installed. The Git view needs it."
                        }),
                )
            })
            .children(close)
    });
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
        .child(side(welcome.stage, cx))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .justify_center()
                .items_center()
                .child(
                    div()
                        .w(step_width(window))
                        .flex()
                        .flex_col()
                        .children(screen),
                )
                .children(foot_line),
        )
        .into_any_element()
}
