//! The first run's welcome over the whole window: the first moves to make, and the
//! agents on this machine with an install or a sign-in for those that need one.

use std::cell::Cell;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, relative, AnyElement, App, Context, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, WindowControlArea,
};
use serde_json::json;
use sikemux_core::agent_accounts::status::AgentStatus;

use crate::agents::setup::{self, Found};
use crate::core;
use crate::experience::{self, Experience};
use crate::icons::Icon;
use crate::keymap;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_3H, SPACE_4, SPACE_6,
    TEXT_2XS, TEXT_LG, TEXT_MD, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

/// Real first moves; picking one closes the welcome and does it.
const MOVES: [(&str, &str, &str, Icon); 4] = [
    (
        "project.open",
        "Open a project",
        "Pick a folder or repository to work in",
        Icon::Folder,
    ),
    (
        "agent.new",
        "Start an agent",
        "In a project you pick, ready to type to",
        Icon::Agent,
    ),
    (
        "palette.commands",
        "Browse commands",
        "Every action and its shortcut",
        Icon::Search,
    ),
    (
        "ssh.open",
        "Connect to a host",
        "Hosts from your SSH config",
        Icon::Server,
    ),
];

pub struct Welcome {
    /// The welcome itself, which holds the keys until they choose a move.
    root: FocusHandle,
    moves: [FocusHandle; 4],
    /// Whether the welcome has taken the focus, which it does when it first draws.
    focused: Cell<bool>,
    git_missing: bool,
}

impl Welcome {
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        Self {
            root: cx.focus_handle(),
            moves: std::array::from_fn(|_| cx.focus_handle()),
            focused: Cell::new(false),
            git_missing: false,
        }
    }
}

fn welcome_of(bench: &mut Workbench) -> Option<&mut Welcome> {
    match bench.overlays.experience.as_mut()? {
        Experience::Welcome(welcome) => Some(welcome),
        _ => None,
    }
}

/// Looks for the agents here, and whether git is.
pub fn opened(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    setup::read(bench, cx);
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let tools = core::request(&asks, "hostTools", json!({})).await;
        let _ = bench.update(cx, |bench, cx| {
            if let (Some(welcome), Ok(tools)) = (welcome_of(bench), tools) {
                welcome.git_missing = tools["git"] == false;
            }
            bench.parts.redraw(&bench.parts.over, cx);
        });
    })
    .detach();
}

fn take(id: &'static str, window: &mut Window, cx: &mut Context<Workbench>, bench: &mut Workbench) {
    experience::close(bench, cx);
    keymap::dispatch(id, window, cx);
}

actions!(welcome, [Next, Previous]);

/// Tab and the arrows step between the moves, from the first or the last while none is chosen.
pub fn install(cx: &mut App) {
    let context = Some("Onboarding");
    cx.bind_keys([
        KeyBinding::new("tab", Next, context),
        KeyBinding::new("down", Next, context),
        KeyBinding::new("shift-tab", Previous, context),
        KeyBinding::new("up", Previous, context),
    ]);
}

fn step(bench: &mut Workbench, forward: bool, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    let last = MOVES.len() - 1;
    let focused = welcome
        .moves
        .iter()
        .position(|handle| handle.is_focused(window));
    let to = match (focused, forward) {
        (None, true) => 0,
        (None, false) => last,
        (Some(at), true) => (at + 1) % MOVES.len(),
        (Some(at), false) => (at + last) % MOVES.len(),
    };
    welcome.moves[to].focus(window, cx);
    bench.parts.redraw(&bench.parts.over, cx);
}

/// The welcome's keys: a move's own shortcut does it, Return takes the one in focus, and Esc
/// skips.
fn key_down(
    bench: &mut Workbench,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let keystroke = &event.keystroke;
    if let Some(&(id, ..)) = MOVES
        .iter()
        .find(|(id, ..)| keymap::pressed(id, keystroke, cx))
    {
        cx.stop_propagation();
        return take(id, window, cx, bench);
    }
    let Some(welcome) = welcome_of(bench) else {
        return;
    };
    let focused = welcome
        .moves
        .iter()
        .position(|handle| handle.is_focused(window));
    match keystroke.key.as_str() {
        "escape" => {
            cx.stop_propagation();
            experience::close(bench, cx)
        }
        "enter" => {
            let Some(at) = focused else {
                return;
            };
            cx.stop_propagation();
            take(MOVES[at].0, window, cx, bench)
        }
        _ => {}
    }
}

pub fn render(
    bench: &Workbench,
    this: &Entity<Workbench>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let Some(Experience::Welcome(welcome)) = &bench.overlays.experience else {
        return div().into_any_element();
    };
    if !welcome.focused.replace(true) {
        let root = welcome.root.clone();
        window.defer(cx, move |window, cx| root.focus(window, cx));
    }
    let t = Tokens::get(cx).clone();
    let moves = MOVES
        .iter()
        .zip(&welcome.moves)
        .map(|(&(id, label, detail, icon), focus)| {
            let focused = focus.is_focused(window);
            let ink = if focused { t.accent } else { t.text_tertiary };
            let this = this.clone();
            let keys = keymap::hint(id, cx);
            div()
                .id(SharedString::from(format!("welcome-{id}")))
                .track_focus(focus)
                .flex()
                .items_center()
                .gap(SPACE_3H)
                .px(SPACE_4)
                .py(SPACE_3H)
                .border_1()
                .border_color(if focused {
                    mix(t.accent, t.border, 0.55)
                } else {
                    t.border
                })
                .rounded(theme::RADIUS_2)
                .bg(if focused {
                    mix(t.accent, t.surface_raised, 0.08)
                } else {
                    Hsla {
                        a: 0.8,
                        ..t.surface
                    }
                })
                .cursor_pointer()
                .when(!focused, |row| row.hover(|row| row.bg(t.surface_raised)))
                .on_mouse_down(MouseButton::Left, keep_press)
                .on_click(move |_, window, cx| {
                    this.update(cx, |bench, cx| take(id, window, cx, bench))
                })
                .child(
                    div()
                        .w(px(18.))
                        .flex_none()
                        .child(ui::icon(icon, 16.).text_color(ink)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            ui::text(label, TEXT_MD, FontWeight::MEDIUM).text_color(t.text_primary),
                        )
                        .child(
                            ui::text(detail, TEXT_XS, FontWeight::NORMAL)
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(t.text_tertiary),
                        ),
                )
                .children(keys.map(|keys| {
                    div()
                        .px(px(7.))
                        .py(px(3.))
                        .border_1()
                        .border_color(if focused {
                            mix(t.accent, t.border, 0.55)
                        } else {
                            t.border
                        })
                        .rounded(theme::RADIUS_1)
                        .bg(t.surface_sunken)
                        .child(
                            ui::kbd(keys, cx)
                                .text_size(crate::theme::ui_text(TEXT_XS))
                                .when(focused, |keys| keys.text_color(t.text_primary)),
                        )
                }))
        });
    let body = div()
        .id("welcome-body")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .justify_center()
        .child(
            div()
                .w(px(520.))
                .max_w(relative(0.9))
                .mx_auto()
                .pt(px(48.))
                .pb(SPACE_6)
                .flex()
                .flex_col()
                .child(ui::icon(Icon::Logo, 64.).ml(px(-15.)).text_color(t.accent))
                .child(
                    div()
                        .mt(px(22.))
                        .flex()
                        .gap(px(11.))
                        .text_size(crate::theme::ui_text(44.))
                        .font_weight(FontWeight::BOLD)
                        .letter_spacing(-0.05)
                        .line_height(relative(1.05))
                        .text_color(t.text_primary)
                        .child("Welcome to")
                        .child(div().text_color(t.accent).child("Sikemux")),
                )
                .child(
                    div()
                        .mt(SPACE_3)
                        .mb(px(32.))
                        .text_size(crate::theme::ui_text(TEXT_LG))
                        .line_height(relative(1.5))
                        .text_color(t.text_secondary)
                        .child(
                            "Projects, terminals and coding agents in one keyboard-first window.",
                        ),
                )
                .child(div().flex().flex_col().gap(SPACE_1).children(moves))
                .child(found(bench, this, cx)),
        );
    let skip = {
        let this = this.clone();
        div()
            .id("welcome-skip")
            .ml_auto()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .cursor_pointer()
            .hover(|skip| skip.text_color(t.text_primary))
            .on_mouse_down(MouseButton::Left, keep_press)
            .on_click(move |_, _, cx| this.update(cx, experience::close))
            .child(
                div()
                    .px(px(5.))
                    .py(SPACE_0)
                    .border_1()
                    .border_color(t.border)
                    .rounded(theme::RADIUS_1)
                    .child("esc"),
            )
            .child("skip")
    };
    let this = this.clone();
    div()
        .id("welcome")
        .key_context("Modal Onboarding")
        .track_focus(&welcome.root)
        .on_action({
            let this = this.clone();
            move |_: &Next, window, cx| this.update(cx, |bench, cx| step(bench, true, window, cx))
        })
        .on_action({
            let this = this.clone();
            move |_: &Previous, window, cx| {
                this.update(cx, |bench, cx| step(bench, false, window, cx))
            }
        })
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(Hsla {
            a: 0.94,
            ..t.surface_sunken
        })
        .backdrop_blur(px(28.))
        .occlude()
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
        .on_key_down(move |event, window, cx| {
            this.update(cx, |bench, cx| key_down(bench, event, window, cx))
        })
        .child(
            crate::ui::grain::sky(0., 0.4, cx)
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(relative(0.32)),
        )
        .child(body)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(22.))
                .py(SPACE_3H)
                .font_family(theme::KBD)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .when(welcome.git_missing, |foot| {
                    foot.child(
                        div()
                            .text_color(t.warn)
                            .child("git not found: the Git view needs it"),
                    )
                })
                .child(skip),
        )
        .children(setup::sheet(bench, &cx.entity(), window, cx))
        .into_any_element()
}

/// Keeps a press on a button from dragging the window, as a press anywhere else on the welcome does.
fn keep_press(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

/// `a` mixed into `b`, as CSS's `color-mix(a amount, b)`.
fn mix(a: Hsla, b: Hsla, amount: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let channel = |a: f32, b: f32| a * amount + b * (1. - amount);
    gpui_kit::Rgba {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: channel(a.a, b.a),
    }
    .into()
}

/// The agents the core found, each with what it needs next.
fn found(bench: &Workbench, this: &Entity<Workbench>, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let muted = |text: &'static str| {
        div()
            .mt(SPACE_3)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_tertiary)
            .child(text)
    };
    let list: AnyElement = match &bench.agent_setup.found {
        None => muted("Looking on your PATH…").into_any_element(),
        Some(found) if found.is_empty() => {
            muted("None found. Install Claude Code, Codex or another supported CLI.")
                .into_any_element()
        }
        Some(found) => div()
            .mt(SPACE_3)
            .flex()
            .flex_wrap()
            .gap(SPACE_1H)
            .children(found.iter().map(|agent| chip(bench, agent, this, cx)))
            .into_any_element(),
    };
    div()
        .mt(px(32.))
        .child(
            ui::text("AGENTS ON THIS MACHINE", 9., FontWeight::SEMIBOLD)
                .font_family(theme::MONO)
                .letter_spacing(0.08)
                .text_color(t.text_tertiary),
        )
        .child(list)
}

fn chip(bench: &Workbench, agent: &Found, this: &Entity<Workbench>, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let missing = matches!(agent.status, AgentStatus::Missing);
    let broken = matches!(agent.status, AgentStatus::Broken { .. });
    let signed_out = matches!(agent.status, AgentStatus::SignedOut);
    let status =
        (!matches!(agent.status, AgentStatus::Unknown)).then(|| setup::status_label(&agent.status));
    let folder = bench
        .accounts
        .current(&agent.kind)
        .and_then(|profile| profile.config_path.clone());
    let sheet = match (missing || broken, signed_out) {
        (true, _) => setup::installing(agent)
            .map(|sheet| (if broken { "Reinstall" } else { "Install" }, sheet)),
        (_, true) => setup::signing_in(agent, folder.as_deref()).map(|sheet| ("Sign in", sheet)),
        _ => None,
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .pl(px(9.))
        .pr(px(11.))
        .py(px(7.))
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .map(|chip| match missing {
            true => chip.border_dashed().text_color(t.text_tertiary),
            false => chip
                .bg(Hsla {
                    a: 0.8,
                    ..t.surface
                })
                .text_color(t.text_primary),
        })
        .child(
            div()
                .when(missing, |mark| mark.opacity(0.4))
                .child(ui::mark(&agent.kind, 18.)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .line_height(relative(1.2))
                .child(agent.label.clone())
                .children(status.map(|status| {
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(if signed_out { t.warn } else { t.text_tertiary })
                        .child(status)
                })),
        )
        .children(sheet.map(|(label, sheet)| {
            let this = this.clone();
            div()
                .id(SharedString::from(format!("welcome-agent-{}", agent.kind)))
                .h(px(22.))
                .ml(SPACE_1)
                .px(SPACE_2)
                .flex()
                .items_center()
                .border_1()
                .border_color(t.border)
                .rounded(theme::RADIUS_2)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_primary)
                .cursor_pointer()
                .hover(|button| button.bg(t.surface_hover))
                .on_mouse_down(MouseButton::Left, keep_press)
                .on_click(move |_, _, cx| {
                    let sheet = sheet.clone();
                    this.update(cx, |bench, cx| setup::run(bench, sheet, cx))
                })
                .child(label)
        }))
}
