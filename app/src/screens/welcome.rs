//! The first-run setup over the whole window: an agent that works, then the projects to open.
//! On a real first run nothing leaves it but finishing, and only finishing counts it done.

use std::path::Path;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, relative, AnyElement, App, AppContext, Context, Div, Entity, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement, PathPromptOptions, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Window, WindowControlArea,
};
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_core::server::project_roots::{ResolveParams, Resolved};
use sikemux_core::sessions::found_projects::FoundProject;

use crate::agents::setup::{self, Found};
use crate::core::Target;
use crate::screens::experience::{self, Experience};
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_3, SPACE_3H, TEXT_2XS, TEXT_SM};
use crate::ui;
use crate::ui::path::{this_home, tilde};
use crate::workbench::Workbench;
use sikemux_core::agent_accounts::status::AgentStatus;

const TERMINAL_HEIGHT: f32 = 208.;

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
}

struct Candidate {
    name: String,
    path: String,
    shown: String,
    meta: Option<String>,
    ticked: bool,
}

impl Candidate {
    fn found(project: FoundProject, home: Option<&str>) -> Self {
        let chats = match project.chats {
            1 => "1 chat".to_owned(),
            chats => format!("{chats} chats"),
        };
        Self {
            shown: tilde(&project.path, home),
            meta: Some(format!("{chats} · {}", ui::time::ago(project.active_ms))),
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
    stage: Stage,
    at: Option<Stop>,
    git_missing: bool,
    rows: Vec<Candidate>,
    looking: bool,
    partial: bool,
    path: Option<Entity<InputState>>,
    path_error: Option<String>,
    opening: bool,
    scroll: ScrollHandle,
    _typing: Option<Subscription>,
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
            path_error: None,
            opening: false,
            scroll: ScrollHandle::new(),
            _typing: None,
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
}

/// Looks for the agents here, the folders their history names, and whether git is.
pub fn opened(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.agent_setup.failed = false;
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
        let found = Target::here(&asks)
            .request_typed::<call::FoundProjects>(NoParams {})
            .await
            .unwrap_or_default();
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
            welcome.looking = false;
            redraw(bench, cx);
        });
    })
    .detach();
}

/// Makes the path field once a window can hold it, and keeps the keys in the setup.
pub fn take_keys(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
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
                            welcome.path_error = None;
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
    if !welcome.root.contains_focused(window, cx) {
        welcome.root.focus(window, cx);
    }
}

actions!(welcome, [Next, Previous]);

pub fn install(cx: &mut App) {
    for context in ["Onboarding && !Terminal", "Onboarding > Input"] {
        let context = Some(context);
        cx.bind_keys([
            KeyBinding::new("tab", Next, context),
            KeyBinding::new("down", Next, context),
            KeyBinding::new("shift-tab", Previous, context),
            KeyBinding::new("up", Previous, context),
        ]);
    }
}

fn anyway_offered(bench: &Workbench) -> bool {
    bench.agent_setup.failed && ready_agents(bench) == 0
}

fn ready_agents(bench: &Workbench) -> usize {
    bench.agent_setup.found.as_ref().map_or(0, |found| {
        found
            .iter()
            .filter(|agent| setup::usable(&agent.status))
            .count()
    })
}

fn stops(bench: &Workbench, welcome: &Welcome) -> Vec<Stop> {
    match welcome.stage {
        Stage::Agents => {
            let agents = bench.agent_setup.found.as_ref().map_or(0, Vec::len);
            (0..agents)
                .map(Stop::Row)
                .chain(anyway_offered(bench).then_some(Stop::Anyway))
                .collect()
        }
        Stage::Projects => (0..welcome.rows.len())
            .map(Stop::Row)
            .chain([Stop::Field, Stop::Browse, Stop::Back])
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
    let stops = stops(bench, welcome);
    let Some(last) = stops.len().checked_sub(1) else {
        return;
    };
    let from = welcome
        .at
        .and_then(|at| stops.iter().position(|stop| *stop == at));
    let to = match (from, forward) {
        (None, true) => 0,
        (None, false) => last,
        (Some(at), true) => (at + 1) % stops.len(),
        (Some(at), false) => (at + last) % stops.len(),
    };
    go(bench, stops[to], window, cx);
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
    welcome.scroll = ScrollHandle::new();
    let first = match (stage, welcome.rows.is_empty()) {
        (Stage::Projects, true) => Stop::Field,
        _ => Stop::Row(0),
    };
    go(bench, first, window, cx);
}

fn run_step(bench: &mut Workbench, row: usize, cx: &mut Context<Workbench>) {
    if bench.agent_setup.busy() {
        return;
    }
    let next = (bench.agent_setup.found.as_ref())
        .and_then(|found| found.get(row))
        .and_then(|agent| setup::next_step(bench, agent));
    if let Some((_, sheet)) = next {
        setup::run(bench, sheet, cx);
    }
}

fn toggle(bench: &mut Workbench, row: usize, cx: &mut Context<Workbench>) {
    if let Some(found) = welcome_of(bench).and_then(|welcome| welcome.rows.get_mut(row)) {
        found.ticked = !found.ticked;
    }
    redraw(bench, cx);
}

fn primary(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    match welcome_in(bench).map(|welcome| welcome.stage) {
        Some(Stage::Agents) if ready_agents(bench) > 0 => {
            show_stage(bench, Stage::Projects, window, cx)
        }
        Some(Stage::Projects) => finish(bench, window, cx),
        _ => {}
    }
}

/// Space does what a row or button offers; Return presses a button or moves the setup on.
fn activate(bench: &mut Workbench, space: bool, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_in(bench) else {
        return;
    };
    match (welcome.stage, welcome.at) {
        (_, Some(Stop::Anyway)) => show_stage(bench, Stage::Projects, window, cx),
        (_, Some(Stop::Browse)) => browse(window, cx),
        (_, Some(Stop::Back)) => show_stage(bench, Stage::Agents, window, cx),
        (Stage::Agents, Some(Stop::Row(row))) if space => run_step(bench, row, cx),
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
    experience::close(bench, cx);
}

fn path_problem(typed: &str, resolved: &Resolved) -> Option<String> {
    if !Path::new(&resolved.path).is_absolute() {
        return Some("Type the whole path, starting with / or ~".to_owned());
    }
    match resolved.kind.as_deref() {
        Some("dir") => None,
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
                (Some(problem), _) => welcome.path_error = Some(problem),
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

/// The launcher a new chat should start on: the one it would use anyway when that agent works,
/// else the first agent that does.
fn ready_launcher(bench: &Workbench) -> Option<String> {
    let found = bench.agent_setup.found.as_ref()?;
    let works = |kind: &str| {
        found
            .iter()
            .any(|agent| agent.kind == kind && setup::usable(&agent.status))
    };
    bench
        .launcher()
        .filter(|launcher| works(&launcher.provider))
        .or_else(|| {
            found
                .iter()
                .filter(|agent| setup::usable(&agent.status))
                .find_map(|agent| bench.launcher_of_kind(&agent.kind))
        })
        .map(|launcher| launcher.id.clone())
}

/// Adds what is ticked, counts the setup done and opens a chat in the first project added.
fn finish(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    let paths: Vec<String> = welcome.ticked().map(|row| row.path.clone()).collect();
    if welcome.opening || paths.is_empty() {
        return;
    }
    welcome.opening = true;
    let adding: Vec<_> = paths
        .iter()
        .map(|path| bench.add_project(path, cx))
        .collect();
    redraw(bench, cx);
    cx.spawn_in(window, async move |bench, cx| {
        let mut first = None;
        for added in adding {
            let added = added.await;
            first = first.or(added);
        }
        let _ = bench.update_in(cx, |bench, window, cx| {
            let Some(project) = first else {
                if let Some(welcome) = welcome_of(bench) {
                    welcome.opening = false;
                }
                return redraw(bench, cx);
            };
            let launcher = ready_launcher(bench);
            let any_ready = ready_agents(bench) > 0;
            leave(bench, cx);
            bench.activate_project(&project);
            match launcher {
                Some(launcher) => bench.target().start_chat(launcher, project, None, None),
                None if any_ready => bench.start_chat_in(&project, window, cx),
                None => {}
            }
        });
    })
    .detach();
}

fn keep_press(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

fn stages_bar(stage: Stage, t: &Tokens) -> Div {
    let at = match stage {
        Stage::Agents => 0,
        Stage::Projects => 1,
    };
    div()
        .flex_none()
        .h(px(38.))
        .flex()
        .items_center()
        .justify_center()
        .gap(SPACE_2)
        .text_size(theme::ui_text(TEXT_2XS))
        .children(
            ["Agents", "Projects", "Start"]
                .into_iter()
                .enumerate()
                .map(|(index, name)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .when(index > 0, |step| {
                            step.child(div().w(px(22.)).h(px(1.)).bg(t.border))
                        })
                        .child(
                            div()
                                .text_color(match index {
                                    index if index == at => t.text_primary,
                                    index if index < at => t.text_secondary,
                                    _ => t.text_tertiary,
                                })
                                .when(index == at, |name| name.font_weight(FontWeight::MEDIUM))
                                .child(name),
                        )
                }),
        )
}

fn heading(title: &'static str, sub: &'static str, t: &Tokens) -> [Div; 2] {
    [
        div()
            .flex_none()
            .text_size(theme::ui_text(24.))
            .font_weight(FontWeight::BOLD)
            .letter_spacing(-0.02)
            .text_color(t.text_primary)
            .child(title),
        div()
            .flex_none()
            .mt(px(5.))
            .mb(px(16.))
            .text_size(theme::ui_text(TEXT_SM))
            .text_color(t.text_secondary)
            .child(sub),
    ]
}

fn row(id: String, lit: bool, last: bool, t: &Tokens) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .flex_none()
        .min_h(px(44.))
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(11.))
        .py(px(8.))
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
        .min_h(px(44.))
        .flex()
        .items_center()
        .px(px(11.))
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary)
        .child(note)
        .into_any_element()
}

fn listing(
    scroll: &ScrollHandle,
    rows: Vec<AnyElement>,
    under: Option<AnyElement>,
    t: &Tokens,
) -> Div {
    div()
        .min_h_0()
        .flex_shrink(1.)
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
                .min_h_0()
                .flex_shrink(1.)
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

fn agent_row(
    bench: &Workbench,
    agent: &Found,
    at: usize,
    last: bool,
    lit: Option<Stop>,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let underway = bench.agent_setup.underway(&agent.kind);
    let (status, ink) = match (underway, &agent.status) {
        (Some(underway), _) => (underway, t.text_secondary),
        (None, AgentStatus::Ready { .. }) => (setup::status_label(&agent.status), t.live),
        (None, AgentStatus::SignedOut) => (setup::status_label(&agent.status), t.warn),
        (None, AgentStatus::Broken { .. }) => (setup::status_label(&agent.status), t.danger),
        (None, status) => (setup::status_label(status), t.text_secondary),
    };
    let next = underway
        .is_none()
        .then(|| setup::next_step(bench, agent))
        .flatten();
    let busy = bench.agent_setup.busy();
    let chosen = this.clone();
    row(
        format!("welcome-agent-{}", agent.kind),
        lit == Some(Stop::Row(at)),
        last,
        &t,
    )
    .on_click(move |_, window, cx| {
        chosen.update(cx, |bench, cx| go(bench, Stop::Row(at), window, cx))
    })
    .child(ui::mark(&agent.kind, 18.))
    .child(
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                ui::text(agent.label.clone(), TEXT_SM, FontWeight::MEDIUM)
                    .text_color(t.text_primary),
            )
            .child(
                div()
                    .text_size(theme::ui_text(TEXT_2XS))
                    .text_color(ink)
                    .child(status),
            ),
    )
    .children(next.map(|(label, sheet)| {
        let this = this.clone();
        ui::Button::new(
            SharedString::from(format!("welcome-step-{}", agent.kind)),
            label,
            ui::Variant::Outline,
        )
        .disabled(busy)
        .on_click(move |_, _, cx| {
            let sheet = sheet.clone();
            this.update(cx, |bench, cx| {
                if !bench.agent_setup.busy() {
                    setup::run(bench, sheet, cx);
                }
            })
        })
    }))
    .into_any_element()
}

fn primary_button(
    label: &'static str,
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

fn foot() -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(10.))
        .pt(SPACE_3H)
        .pb(px(16.))
}

fn agents_screen(
    bench: &Workbench,
    welcome: &Welcome,
    lit: Option<Stop>,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let rows: Vec<AnyElement> = match &bench.agent_setup.found {
        None => vec![note_row(
            ui::shimmer::shimmer("welcome-agents-looking", "Looking on your PATH…"),
            &t,
        )],
        Some(found) if found.is_empty() => vec![note_row(
            "None found. Install Claude Code, Codex or another supported CLI.".into_any_element(),
            &t,
        )],
        Some(found) => found
            .iter()
            .enumerate()
            .map(|(at, agent)| agent_row(bench, agent, at, at + 1 == found.len(), lit, this, cx))
            .collect(),
    };
    let terminal = setup::inline(bench, this, px(TERMINAL_HEIGHT), cx).map(|terminal| {
        div()
            .flex_none()
            .border_t_1()
            .border_color(t.border)
            .bg(t.surface_sunken)
            .on_mouse_down(MouseButton::Left, keep_press)
            .child(terminal)
            .into_any_element()
    });
    let ready = ready_agents(bench);
    let signed_in = bench.agent_setup.found.as_ref().map_or(0, |found| {
        found
            .iter()
            .filter(|agent| matches!(agent.status, AgentStatus::Ready { .. }))
            .count()
    });
    let [title, sub] = heading(
        "Connect an agent",
        "You need at least one working agent. Sikemux runs each one's own installer and sign-in.",
        &t,
    );
    vec![
        title.into_any_element(),
        sub.into_any_element(),
        listing(&welcome.scroll, rows, terminal, &t).into_any_element(),
        foot()
            .child(hint(
                match (signed_in, ready) {
                    (0, 0) => "Get one agent ready to continue".to_owned(),
                    (0, found) => format!("{found} found"),
                    (signed_in, _) => format!("{signed_in} ready"),
                },
                &t,
            ))
            .child(div().flex_1())
            .when(anyway_offered(bench), |foot| {
                foot.child(stop_button(
                    "welcome-anyway",
                    "Continue anyway",
                    Stop::Anyway,
                    lit,
                    this,
                ))
            })
            .child(primary_button("Continue", ready > 0, this))
            .into_any_element(),
    ]
}

fn projects_screen(
    welcome: &Welcome,
    lit: Option<Stop>,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let count = welcome.rows.len();
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
                    .flex_col()
                    .child(
                        ui::text(candidate.name.clone(), TEXT_SM, FontWeight::MEDIUM)
                            .truncate()
                            .text_color(t.text_primary),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(theme::MONO)
                            .text_size(theme::ui_text(TEXT_2XS))
                            .text_color(t.text_tertiary)
                            .child(candidate.shown.clone()),
                    ),
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
        rows.push(note_row(
            "No saved chats here yet. Add the folders you work in below.".into_any_element(),
            &t,
        ));
    }
    let ticked = welcome.ticked().count();
    let [title, sub] = heading(
        "Add your projects",
        "Found in your agents' history. Newest first; active ones are ticked.",
        &t,
    );
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
        listing(&welcome.scroll, rows, None, &t).into_any_element(),
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
    ]
    .into_iter()
    .chain(welcome.path_error.clone().map(|error| {
        div()
            .flex_none()
            .mt(SPACE_1H)
            .text_size(theme::ui_text(TEXT_2XS))
            .text_color(t.danger)
            .child(error)
            .into_any_element()
    }))
    .chain(welcome.partial.then(|| {
        hint(
            "Only your newest history was read, so some older folders may be missing.".to_owned(),
            &t,
        )
        .flex_none()
        .mt(SPACE_1H)
        .into_any_element()
    }))
    .chain([foot()
        .child(stop_button("welcome-back", "Back", Stop::Back, lit, this))
        .child(hint(
            match ticked {
                0 => "Tick or add a project to continue".to_owned(),
                ticked => format!("{ticked} selected"),
            },
            &t,
        ))
        .child(div().flex_1())
        .child(primary_button(
            if welcome.opening {
                "Opening…"
            } else {
                "Open Sikemux"
            },
            ticked > 0 && !welcome.opening,
            this,
        ))
        .into_any_element()])
    .collect()
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
        Stage::Agents => agents_screen(bench, welcome, lit, this, cx),
        Stage::Projects => projects_screen(welcome, lit, this, cx),
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
            .flex_none()
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
                        .child("git not found: the Git view needs it"),
                )
            })
            .children(close)
    });
    let keys = this.clone();
    let next = this.clone();
    let previous = this.clone();
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
        .child(stages_bar(welcome.stage, &t))
        .child(
            div().flex_1().min_h_0().child(
                div()
                    .w(px(520.))
                    .max_w(relative(0.92))
                    .h_full()
                    .mx_auto()
                    .pt(px(22.))
                    .flex()
                    .flex_col()
                    .children(screen),
            ),
        )
        .children(foot_line)
        .into_any_element()
}
