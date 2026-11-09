//! The Limits panel: how much of its plan each agent's account has used, drawn as a ruler per
//! agent, with every account in a float over the rows and in the top bar's popover.

use std::cell::Cell;
use std::f32::consts::TAU;
use std::time::Duration;

use gpui_kit::base::Popover;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, fill, point, px, size, Anchor, Animation, AnimationExt, AnyElement, App, Bounds,
    Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Task, Transformation, WeakEntity, Window,
};
use sikemux_core::agent_accounts::usage::UsageWindow;
use sikemux_core::agent_accounts::Profile;
use sikemux_core::server::agent_accounts::{AccountUsage, Unpicking};
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_view::workspace::LauncherInfo;

use crate::core::Target;
use crate::icons::Icon;
use crate::rail::Edge;
use crate::theme::{
    Tokens, RADIUS_2, RADIUS_4, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, TEXT_2XS, TEXT_3XS,
    TEXT_3XS_H,
};
use crate::ui::IconButton;
use crate::workbench::Workbench;
use crate::{motion, theme, ui};

const OPEN_AFTER: Duration = Duration::from_millis(120);
const CLOSE_AFTER: Duration = Duration::from_millis(180);
const UNDO_FOR: Duration = Duration::from_secs(6);
/// A read that never answers, as when the core restarts, stops spinning after this.
const READING_AT_MOST: Duration = Duration::from_secs(60);

const ROW: f32 = 30.;
const ROW_COMPACT: f32 = 28.;
const ROW_GAP: f32 = 2.;
const HEADER: f32 = 24.;
const AGENT_HEAD: f32 = 22.;
const AGENT_GAP: f32 = 4.;
const ACCOUNT_LINE: f32 = 24.;
const FLOAT_GAP: f32 = 6.;
const FLOAT_PAD: f32 = 6.;
/// The smallest float worth scrolling, and what it leaves of the rail above it.
const FLOAT_LEAST: f32 = 120.;
const RAIL_HEAD: f32 = 41.;
const FLOAT_MARGIN: f32 = 52.;
const POPOVER_WIDTH: f32 = 288.;
const POPOVER_MARGIN: f32 = 60.;

/// What the Limits float and popover hold while they are on screen.
#[derive(Default)]
pub struct Reveal {
    open: bool,
    over_rows: bool,
    over_panel: bool,
    over_float: bool,
    settling: Option<Task<()>>,
    popover: bool,
    undo: Option<Undo>,
    reading: Option<Task<()>>,
    /// Whether the rail's last draw held the panel, which only drawing it can say.
    panel_drawn: Cell<bool>,
}

/// A pick, offered back for a few seconds with the chats it moved.
struct Undo {
    undoing: Unpicking,
    _expires: Task<()>,
}

/// One account as the Limits panel shows it.
struct Shown<'a> {
    /// The account, or none for an agent the person never added an account to.
    profile: Option<&'a Profile>,
    name: String,
    usage: Option<&'a AccountUsage>,
    signed_out: bool,
}

impl Shown<'_> {
    fn id(&self) -> Option<&str> {
        self.profile.map(|profile| profile.id.as_str())
    }

    fn key(&self, provider: &str) -> String {
        self.id().unwrap_or(provider).to_owned()
    }

    fn stale(&self, now: u64) -> bool {
        self.usage.is_some_and(|usage| usage.is_stale(now))
    }

    fn week(&self) -> Option<&UsageWindow> {
        self.usage?.usage.as_ref()?.week()
    }

    /// How long ago its numbers were read, once that is worth saying.
    fn age(&self, now: u64) -> Option<String> {
        let read = self.usage?.read_at?;
        self.stale(now).then(|| ui::time::ago(read))
    }

    /// Why there is nothing to draw for a signed-in account.
    fn nothing(&self) -> String {
        let Some(usage) = self.usage else {
            return "Not read yet".into();
        };
        match (&usage.problem, &usage.usage) {
            (Some(_), _) => "Could not read".into(),
            (None, Some(_)) => "No plan limits".into(),
            (None, None) => "Not read yet".into(),
        }
    }

    /// The core's own explanation of an account that reports no limits, too long for the row.
    fn no_limits_reason(&self) -> Option<String> {
        let usage = self.usage?;
        (usage.problem.is_none())
            .then(|| usage.usage.as_ref()?.unavailable_reason.clone())
            .flatten()
    }
}

/// An agent that reports plan limits, with its accounts in the order they were added.
struct Agent<'a> {
    provider: &'a str,
    name: &'a str,
    accounts: Vec<Shown<'a>>,
    in_use: usize,
}

impl<'a> Agent<'a> {
    fn read(bench: &'a Workbench, launcher: &'a LauncherInfo) -> Self {
        let provider = launcher.provider.as_str();
        let name = bench.agent_name(provider);
        let profiles = bench.accounts.of(provider);
        let accounts: Vec<Shown<'a>> = if profiles.is_empty() {
            vec![Shown {
                profile: None,
                name: name.to_owned(),
                usage: bench.usage_of(&launcher.id),
                signed_out: launcher.status.as_deref() == Some("signedOut"),
            }]
        } else {
            (profiles.into_iter())
                .map(|profile| {
                    let status = crate::accounts::status(bench, profile)
                        .and_then(|status| status.as_ref().ok());
                    Shown {
                        profile: Some(profile),
                        name: crate::accounts::name(bench, profile, status),
                        usage: crate::accounts::usage(bench, profile),
                        signed_out: status.is_some_and(|status| !status.signed_in),
                    }
                })
                .collect()
        };
        let current = bench.accounts.current(provider).map(|profile| &profile.id);
        let in_use = (accounts.iter())
            .position(|shown| shown.profile.map(|profile| &profile.id) == current)
            .unwrap_or(0);
        Self {
            provider,
            name,
            accounts,
            in_use,
        }
    }

    fn current(&self) -> &Shown<'a> {
        &self.accounts[self.in_use]
    }

    /// Whether the account goes unnamed, because it is only the agent's own.
    fn unnamed(&self, shown: &Shown) -> bool {
        shown.profile.is_none() || shown.name == self.name
    }
}

fn reports_limits(bench: &Workbench, provider: &str) -> bool {
    sikemux_core::agent_catalog::agent(provider).is_some_and(|agent| agent.usage)
        && bench.has_agent(provider)
}

/// The agents the panel has a row for, in the rail's own order: every one that reports limits,
/// or the agent the rail is showing.
fn launchers(bench: &Workbench, every: bool) -> Vec<&LauncherInfo> {
    if !every {
        return (bench.launcher().into_iter())
            .filter(|launcher| reports_limits(bench, &launcher.provider))
            .collect();
    }
    let mut found: Vec<&LauncherInfo> = Vec::new();
    for launcher in bench.launchers() {
        if reports_limits(bench, &launcher.provider)
            && found.iter().all(|seen| seen.provider != launcher.provider)
        {
            found.push(
                bench
                    .launcher_of_kind(&launcher.provider)
                    .unwrap_or(launcher),
            );
        }
    }
    found
}

fn agents(bench: &Workbench, every: bool) -> Vec<Agent<'_>> {
    (launchers(bench, every).into_iter())
        .map(|launcher| Agent::read(bench, launcher))
        .collect()
}

/// What every piece of the panel draws from.
struct View {
    this: WeakEntity<Workbench>,
    now: u64,
    solo: bool,
    compact: bool,
    reading: bool,
    /// The account a pick came from, with Undo on its row.
    undo: Option<String>,
    /// How tall the float's list may grow before it scrolls.
    room: f32,
}

impl View {
    fn on<E>(
        &self,
        act: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let this = self.this.clone();
        move |_, window, cx| {
            let _ = this.update(cx, |bench, cx| act(bench, window, cx));
        }
    }
}

/// The panel at the foot of the rail: the rows, and the float while the pointer rests on them.
pub(crate) fn limits(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<impl IntoElement> {
    let every = bench.rails.all_harnesses;
    let agents = agents(bench, every);
    if agents.is_empty() {
        return None;
    }
    bench.accounts.limits.panel_drawn.set(true);
    let compact = bench.rail_compact();
    let rows = rest_slots(&agents, !every);
    let row = if compact { ROW_COMPACT } else { ROW };
    let tall = HEADER + rows as f32 * row + (rows as f32 - 1.) * ROW_GAP;
    let room = (bench.rails.frame.get()[3] - RAIL_HEAD - tall - FLOAT_MARGIN).max(FLOAT_LEAST);
    let view = view(bench, !every, compact, room, cx.weak_entity());
    let open = bench.accounts.limits.open;
    let hovered = |this: WeakEntity<Workbench>, over: fn(&mut Reveal) -> &mut bool| {
        move |inside: &bool, _: &mut Window, cx: &mut App| {
            let inside = *inside;
            let _ = this.update(cx, |bench, cx| {
                *over(&mut bench.accounts.limits) = inside;
                settle_hover(bench, cx);
            });
        }
    };
    Some(
        div()
            .id("limits")
            .relative()
            .flex_none()
            .mt(px(if compact { 10. } else { 12. }))
            .mx(SPACE_2H)
            .mb(px(if compact { 8. } else { 12. }))
            .on_hover(hovered(view.this.clone(), |reveal| &mut reveal.over_panel))
            .child(header(&view, &agents, cx))
            .child(
                div()
                    .id("limits-rows")
                    .flex()
                    .flex_col()
                    .gap(px(ROW_GAP))
                    .on_hover(hovered(view.this.clone(), |reveal| &mut reveal.over_rows))
                    .children(rest_rows(&view, &agents, cx)),
            )
            .when(open, |panel| {
                panel.child(
                    div()
                        .id("limits-float")
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_full()
                        .pb(px(FLOAT_GAP))
                        .occlude()
                        .on_hover(hovered(view.this.clone(), |reveal| &mut reveal.over_float))
                        .child(
                            floating(cx)
                                .p(px(FLOAT_PAD))
                                .child(every_account(&view, &agents, cx))
                                .with_animation(
                                    "limits-float-in",
                                    motion::entrance(motion::POP_UP),
                                    |float, shown| {
                                        float.opacity(shown).relative().top(px(4. * (1. - shown)))
                                    },
                                ),
                        ),
                )
            }),
    )
}

fn view(
    bench: &Workbench,
    solo: bool,
    compact: bool,
    room: f32,
    this: WeakEntity<Workbench>,
) -> View {
    let reveal = &bench.accounts.limits;
    View {
        this,
        now: ui::time::now_ms(),
        solo,
        compact,
        reading: reveal.reading.is_some(),
        undo: reveal
            .undo
            .as_ref()
            .map(|undo| undo.undoing.back_to.clone()),
        room,
    }
}

/// How many rows the panel holds: one per agent shown.
fn rest_slots(agents: &[Agent], solo: bool) -> usize {
    if solo {
        1
    } else {
        agents.len()
    }
}

fn floating(cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .rounded(RADIUS_4)
        .shadow(t.shadow_float())
}

fn small_button(id: impl Into<gpui_kit::ElementId>, icon: Icon, label: String) -> IconButton {
    IconButton::new(id, icon, label)
        .small()
        .width(20.)
        .height(20.)
        .glyph(13.)
}

fn header(view: &View, agents: &[Agent], cx: &App) -> Div {
    let t = Tokens::get(cx);
    let add = (view.solo).then(|| add_button(view, &agents[0]));
    div()
        .h(px(20.))
        .mb(SPACE_1)
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            ui::text("LIMITS", TEXT_2XS, FontWeight::MEDIUM)
                .letter_spacing(0.08)
                .text_color(t.text_tertiary),
        )
        .child(div().flex_1().h(px(1.)).bg(t.border))
        .child(refresh_button(view, agents, cx))
        .children(add)
}

fn add_button(view: &View, agent: &Agent) -> IconButton {
    let provider = agent.provider.to_owned();
    small_button(
        SharedString::from(format!("add-account-{provider}")),
        Icon::Plus,
        format!("Add a {} account", agent.name),
    )
    .on_click(view.on(move |bench, window, cx| {
        crate::accounts::open_sign_in(bench, &provider, None, window, cx);
    }))
}

fn refresh_button(view: &View, agents: &[Agent], cx: &App) -> AnyElement {
    let providers: Vec<String> = agents.iter().map(|a| a.provider.to_owned()).collect();
    let button = small_button(
        "refresh-limits",
        Icon::Refresh,
        "Refresh plan limits".into(),
    )
    .on_click(view.on(move |bench, _, cx| {
        if bench.accounts.limits.reading.is_none() {
            start_reading(bench, cx);
            for provider in &providers {
                crate::accounts::ask_again(bench, provider, cx);
            }
        }
    }));
    if !view.reading || cx.reduce_motion() {
        return button.into_any_element();
    }
    let t = Tokens::get(cx);
    div()
        .size(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            ui::icon(Icon::Refresh, 13.)
                .text_color(t.text_primary)
                .with_animation(
                    "limits-reading",
                    Animation::new(Duration::from_millis(800))
                        .repeat()
                        .with_max_fps(30.),
                    |glyph, turned| {
                        glyph.with_transformation(Transformation::rotate(gpui_kit::Radians(
                            turned * TAU,
                        )))
                    },
                ),
        )
        .into_any_element()
}

/// Reads every account's limits, with the refresh button turning until the core answers.
fn start_reading(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if bench.accounts.limits.reading.is_some() {
        return;
    }
    bench.read_usage();
    bench.accounts.limits.reading = Some(cx.spawn(async move |bench, cx| {
        cx.background_executor().timer(READING_AT_MOST).await;
        let _ = bench.update(cx, |bench, cx| usage_read(bench, cx));
    }));
    redraw(bench, cx);
}

/// Whether a signed-in account's reading is older than the reads made while limits show.
fn reading_is_old(bench: &Workbench, now: u64) -> bool {
    let every = crate::workbench::LIMITS_EVERY.as_millis() as u64;
    (bench.reads.usage.iter())
        .filter(|account| account.signed_in)
        .any(|account| {
            account
                .read_at
                .is_none_or(|read| now.saturating_sub(read) >= every)
        })
}

/// An agent's week, added up over the accounts that have read one.
struct Week {
    /// Points left, out of 100 for each account counted.
    left: f64,
    accounts: usize,
}

impl Week {
    fn of(agent: &Agent, now: u64) -> Option<Self> {
        let left: Vec<f64> = (agent.accounts.iter())
            .filter(|shown| !shown.signed_out)
            .filter_map(|shown| Some(100. - shown.week()?.used_at(now)))
            .collect();
        (!left.is_empty()).then(|| Self {
            left: left.iter().sum(),
            accounts: left.len(),
        })
    }

    /// How much of the whole is used, as one window would say it.
    fn used(&self) -> f64 {
        100. - self.left / self.accounts as f64
    }
}

fn rest_rows(view: &View, agents: &[Agent], cx: &App) -> Vec<AnyElement> {
    let shown = if view.solo { &agents[..1] } else { agents };
    (shown.iter())
        .map(|agent| match Week::of(agent, view.now) {
            Some(week) => rest_row(view, agent, &week, cx),
            None if agent.current().signed_out => sign_in_row(view, agent, cx),
            None => nothing_row(view, agent, cx),
        })
        .collect()
}

fn row_height(view: &View) -> f32 {
    if view.compact {
        ROW_COMPACT
    } else {
        ROW
    }
}

/// A resting row: the agent's mark, a ruler and what ends the line.
fn rest_frame(view: &View, agent: &Agent, used: Option<f64>, end: Div, cx: &App) -> Div {
    div()
        .h(px(row_height(view)))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .child(ui::mark(agent.provider, 18.))
        .child(ruler(used, Tokens::get(cx)).flex_1().min_w_0().h(px(14.)))
        .child(end)
}

/// The float's first column, an account's name, so the rulers line up.
fn label_column(view: &View) -> f32 {
    if view.compact {
        64.
    } else {
        74.
    }
}

fn rest_row(view: &View, agent: &Agent, week: &Week, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let used = week.used();
    let sum = div()
        .flex_none()
        .flex()
        .items_baseline()
        .gap(px(4.))
        .font_features(theme::tabular())
        .child(
            ui::text(
                format!("{}%", week.left.round()),
                TEXT_2XS,
                FontWeight::MEDIUM,
            )
            .text_color(match used {
                used if used < 70. => t.text_primary,
                used => signal(used, t),
            }),
        )
        .child(
            ui::text(
                format!("/ {}%", week.accounts * 100),
                TEXT_3XS,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        );
    div()
        .id(SharedString::from(format!("limit-{}", agent.provider)))
        .child(rest_frame(view, agent, Some(used), sum, cx))
        .into_any_element()
}

fn percent_cell(window: &UsageWindow, now: u64, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let used = window.used_at(now);
    ui::text(left(used), TEXT_2XS, FontWeight::NORMAL)
        .min_w(px(48.))
        .text_right()
        .text_color(match used {
            used if used < 70. => t.text_primary,
            used => signal(used, t),
        })
}

/// How much of a window is left, as every limit says it.
fn left(used: f64) -> String {
    match used >= 100. {
        true => "used up".to_owned(),
        false => format!("{}% left", (100. - used).round()),
    }
}

/// When the window is back. Past its reset with reads failing it says how old the reading is,
/// as "resetting now" would stay true for as long as the reads keep failing.
fn when_cell(window: &UsageWindow, now: u64, age: Option<&str>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let reset = reset_at(window);
    let full = window.used_at(now) >= 100.;
    let text = match (full, reset, age.filter(|_| window.is_past_reset(now))) {
        (_, _, Some(age)) => format!("read {age}"),
        (true, Some(reset), _) => format!("back {}", time_of_day(reset)),
        (true, None, _) => String::new(),
        (false, reset, _) => ui::time::countdown(reset),
    };
    ui::text(text, TEXT_3XS, FontWeight::NORMAL)
        .min_w(px(38.))
        .text_right()
        .text_color(if full {
            t.text_primary
        } else {
            t.text_tertiary
        })
}

fn nothing_row(view: &View, agent: &Agent, cx: &App) -> AnyElement {
    let shown = agent.current();
    div()
        .id(SharedString::from(format!("limit-{}-none", agent.provider)))
        .when_some(shown.no_limits_reason(), |row, reason| {
            row.tooltip(ui::tooltip(reason))
        })
        .child(rest_frame(view, agent, None, note(shown.nothing(), cx), cx))
        .into_any_element()
}

fn note(text: String, cx: &App) -> Div {
    ui::text(text, TEXT_3XS_H, FontWeight::NORMAL)
        .flex_none()
        .text_color(Tokens::get(cx).text_tertiary)
}

fn go(text: &'static str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    ui::text(text, TEXT_3XS_H, FontWeight::NORMAL)
        .flex_none()
        .underline()
        .text_color(t.text_secondary)
}

fn sign_in(
    view: &View,
    provider: &str,
    shown: &Shown,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
    let provider = provider.to_owned();
    let id = shown.id().map(str::to_owned);
    view.on(move |bench, window, cx| {
        crate::accounts::open_sign_in(bench, &provider, id.as_deref(), window, cx);
    })
}

fn sign_in_row(view: &View, agent: &Agent, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let shown = agent.current();
    div()
        .id(SharedString::from(format!(
            "limits-sign-in-{}",
            shown.key(agent.provider)
        )))
        .rounded(RADIUS_2)
        .cursor_pointer()
        .hover(|row| row.bg(t.surface_hover))
        .on_click(sign_in(view, agent.provider, shown))
        .child(rest_frame(view, agent, None, go("Sign in", cx), cx))
        .into_any_element()
}

/// Every account of every agent, one line each, the one in use lifted.
fn every_account(view: &View, agents: &[Agent], cx: &App) -> Stateful<Div> {
    let mut heights = Vec::new();
    let mut items: Vec<AnyElement> = Vec::new();
    for (at, agent) in agents.iter().enumerate() {
        if !view.solo {
            let gap = if at == 0 { 0. } else { AGENT_GAP };
            heights.push(AGENT_HEAD + gap);
            items.push(agent_head(view, agent, gap, cx).into_any_element());
        }
        for (index, shown) in agent.accounts.iter().enumerate() {
            heights.push(ACCOUNT_LINE);
            items.push(account_line(view, agent, shown, index == agent.in_use, cx));
        }
    }
    let total: f32 = heights.iter().sum();
    let mut fits = 0.;
    let mut down = 0.;
    for height in heights {
        down += height;
        if down <= view.room {
            fits = down;
        }
    }
    let shown_height = match (total > view.room, fits > 0.) {
        (true, true) => Some(fits),
        (true, false) => Some(view.room),
        (false, _) => None,
    };
    div()
        .id("limits-accounts")
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .when_some(shown_height, |list, height| list.h(px(height)))
        .children(items)
}

fn agent_head(view: &View, agent: &Agent, gap: f32, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .h(px(AGENT_HEAD))
        .mt(px(gap))
        .pl(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(ui::mark(agent.provider, 13.))
        .child(
            ui::text(agent.name.to_owned(), TEXT_2XS, FontWeight::MEDIUM)
                .flex_1()
                .text_color(t.text_primary),
        )
        .child(add_button(view, agent))
}

/// What an account's name line says about how far to trust its numbers.
fn status(view: &View, shown: &Shown) -> Option<String> {
    let usage = shown.usage?;
    let age = shown.age(view.now);
    match (&usage.problem, age) {
        (Some(_), Some(age)) => Some(format!("Could not read, from {age}")),
        (Some(_), None) => Some("Could not read".into()),
        (None, Some(age)) => Some(format!("Read {age}")),
        (None, None) => None,
    }
}

/// An account as one line: its name, then its week's ruler. Clicking one not in use moves the
/// agent's chats onto it.
fn account_line(view: &View, agent: &Agent, shown: &Shown, in_use: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let key = shown.key(agent.provider);
    let id = SharedString::from(format!("limits-account-{}-{key}", agent.provider));
    let group = SharedString::from(format!("limits-line-{key}"));
    let frame = || {
        div()
            .relative()
            .h(px(ACCOUNT_LINE))
            .px(SPACE_1H)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .rounded(RADIUS_2)
    };
    let name = |text: String| {
        ui::text(text, TEXT_2XS, FontWeight::MEDIUM)
            .min_w_0()
            .truncate()
            .text_color(t.text_primary)
    };
    let side = || div().flex_1().min_w_0().flex().items_baseline().gap(px(5.));
    let meta = |text: String| {
        ui::text(text, TEXT_3XS, FontWeight::NORMAL)
            .flex_none()
            .text_color(t.text_tertiary)
    };
    if shown.signed_out {
        let names = match agent.unnamed(shown) && in_use {
            true => side().child(name("Signed out".into())),
            false => side()
                .child(name(shown.name.clone()))
                .child(meta("Signed out".into())),
        };
        return frame()
            .id(id)
            .cursor_pointer()
            .hover(|line| line.bg(t.surface_hover))
            .on_click(sign_in(view, agent.provider, shown))
            .child(names)
            .child(go("Sign in", cx))
            .into_any_element();
    }
    let undoing = shown.id().is_some() && view.undo.as_deref() == shown.id();
    if undoing {
        return frame()
            .id(id)
            .child(side().child(name(shown.name.clone())))
            .child(
                go("Undo", cx)
                    .id(SharedString::from(format!("limits-undo-{key}")))
                    .cursor_pointer()
                    .hover(|link| link.text_color(t.text_primary))
                    .on_click(view.on(move |bench, _, cx| undo_pick(bench, cx))),
            )
            .into_any_element();
    }
    let window = shown.week();
    let stale = shown.stale(view.now);
    let age = shown.age(view.now);
    let tip = window
        .map(|window| tip(window, view.now, stale))
        .or_else(|| shown.no_limits_reason());
    let tip = match (status(view, shown).filter(|_| window.is_some()), tip) {
        (Some(status), Some(tip)) => Some(format!("{status}. {tip}")),
        (status, tip) => tip.or(status),
    };
    let end = div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .font_features(theme::tabular())
        .child(
            ruler(window.map(|window| window.used_at(view.now)), t)
                .flex_1()
                .min_w_0()
                .h(px(10.)),
        )
        .map(|end| match window {
            Some(window) => end
                .child(percent_cell(window, view.now, cx))
                .child(when_cell(window, view.now, age.as_deref(), cx)),
            None => end.child(note(shown.nothing(), cx)),
        });
    let target = shown.profile.filter(|_| !in_use).cloned();
    let pickable = target.is_some();
    frame()
        .id(id)
        .group(group.clone())
        .when(in_use && agent.accounts.len() > 1, |line| {
            line.bg(t.surface_chosen)
        })
        .when_some(tip, |line, tip| line.tooltip(ui::tooltip(tip)))
        .when_some(target, |line, target| {
            line.cursor_pointer()
                .hover(|line| line.bg(t.surface_hover))
                .on_click(view.on(move |bench, _, cx| pick(bench, &target, cx)))
        })
        .child(
            div()
                .flex_none()
                .w(px(label_column(view)))
                .flex()
                .child(name(shown.name.clone())),
        )
        .child(end.when(pickable, |end| {
            end.group_hover(group.clone(), |end| end.opacity(0.))
        }))
        .when(pickable, |line| {
            line.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(SPACE_1H)
                    .flex()
                    .items_center()
                    .opacity(0.)
                    .group_hover(group.clone(), |action| action.opacity(1.))
                    .child(
                        ui::text("Move chats here", TEXT_3XS, FontWeight::NORMAL)
                            .text_color(t.text_primary),
                    ),
            )
        })
        .into_any_element()
}

/// Puts the agent on `profile`, which moves its open chats, and offers the way back.
fn pick(bench: &mut Workbench, profile: &Profile, cx: &mut Context<Workbench>) {
    crate::accounts::pick(bench, profile, cx, |bench, undoing, cx| {
        bench.accounts.limits.undo = Some(Undo {
            undoing,
            _expires: cx.spawn(async move |bench, cx| {
                cx.background_executor().timer(UNDO_FOR).await;
                let _ = bench.update(cx, |bench, cx| {
                    bench.accounts.limits.undo = None;
                    redraw(bench, cx);
                });
            }),
        });
        redraw(bench, cx);
    });
}

fn undo_pick(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(undo) = bench.accounts.limits.undo.take() else {
        return;
    };
    crate::accounts::unpick(bench, undo.undoing, cx);
    redraw(bench, cx);
}

/// Opens or closes the float a moment after the pointer settles, so that crossing the gap, the
/// header or the rows on the way to it never closes it.
fn settle_hover(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let reveal = &mut bench.accounts.limits;
    let wanted = reveal.over_rows || (reveal.open && (reveal.over_panel || reveal.over_float));
    if wanted == reveal.open {
        reveal.settling = None;
        return;
    }
    if reveal.settling.is_some() {
        return;
    }
    let delay = if wanted { OPEN_AFTER } else { CLOSE_AFTER };
    reveal.settling = Some(cx.spawn(async move |bench, cx| {
        cx.background_executor().timer(delay).await;
        let _ = bench.update(cx, |bench, cx| {
            let reveal = &mut bench.accounts.limits;
            reveal.settling = None;
            reveal.open = wanted;
            bench.parts.redraw(&bench.parts.tabs, cx);
        });
    }));
}

impl Reveal {
    pub(crate) fn drawing_rail(&mut self) {
        *self.panel_drawn.get_mut() = false;
    }

    /// Forgets the pointer: a panel that stops being drawn never hears it leave.
    fn pointer_gone(&mut self) {
        self.open = false;
        self.over_rows = false;
        self.over_panel = false;
        self.over_float = false;
        self.settling = None;
    }
}

/// Draws what shows the plan limits again: the rail's panel and the top bar's popover.
pub(crate) fn redraw(bench: &Workbench, cx: &mut App) {
    bench.parts.redraw(&bench.parts.tabs, cx);
    bench.parts.redraw(&bench.parts.top, cx);
}

/// Stops the refresh spinning: the core has answered, or never will.
pub(crate) fn usage_read(bench: &mut Workbench, cx: &mut App) {
    if bench.accounts.limits.reading.take().is_some() {
        redraw(bench, cx);
    }
}

/// The top bar's chip: each agent's last reading, opening the same rows and accounts in a
/// popover. It only draws what was read already.
pub(crate) fn chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<impl IntoElement> {
    let agents = agents(bench, true);
    if agents.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let now = ui::time::now_ms();
    let this = cx.weak_entity();
    let open_this = this.clone();
    let pairs: Vec<(String, Option<Week>)> = (agents.iter())
        .map(|agent| (agent.provider.to_owned(), Week::of(agent, now)))
        .collect();
    let spoken: Vec<String> = (agents.iter().zip(&pairs))
        .map(|(agent, (_, week))| match week {
            Some(week) => format!(
                "{} 7d {}% of {}% left",
                agent.name,
                week.left.round(),
                week.accounts * 100
            ),
            None => agent.name.to_owned(),
        })
        .collect();
    let tip = format!("Plan limits: {}", spoken.join(", "));
    drop(agents);
    Some(
        Popover::new("limits-chip")
            .anchor(Anchor::TopRight)
            .offset(SPACE_1H)
            .trigger_with(move |open, _, _| {
                let pairs = pairs.into_iter().map(|(provider, week)| {
                    let (text, ink) = match &week {
                        Some(week) => {
                            let used = week.used();
                            let text = format!("{}%", week.left.round());
                            let ink = if used < 70. {
                                t.text_secondary
                            } else {
                                signal(used, &t)
                            };
                            (text, ink)
                        }
                        None => ("–".to_owned(), t.text_tertiary),
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .child(ui::mark(&provider, 12.))
                        .child(
                            ui::text(text, TEXT_2XS, FontWeight::NORMAL)
                                .font_features(theme::tabular())
                                .text_color(ink),
                        )
                });
                div()
                    .id("limits-chip-button")
                    .h(px(26.))
                    .px(px(7.))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .rounded(RADIUS_2)
                    .cursor_pointer()
                    .when(open, |chip| chip.bg(t.surface_chosen))
                    .when(!open, |chip| chip.hover(|chip| chip.bg(t.surface_hover)))
                    .when(!open, |chip| chip.tooltip(ui::tooltip(tip)))
                    .children(pairs)
                    .into_any_element()
            })
            .on_open_change(move |open: &bool, _: &mut Window, cx: &mut App| {
                let open = *open;
                let _ = open_this.update(cx, |bench, cx| {
                    bench.accounts.limits.popover = open;
                    if open && reading_is_old(bench, ui::time::now_ms()) {
                        start_reading(bench, cx);
                    }
                    redraw(bench, cx);
                });
            })
            .content(move |_, window, cx| {
                let Some(bench) = this.upgrade() else {
                    return div().into_any_element();
                };
                let bench = bench.read(cx);
                let agents = self::agents(bench, true);
                let room = f32::from(window.viewport_size().height) - POPOVER_MARGIN;
                let view = self::view(bench, false, false, room.max(FLOAT_LEAST), this.clone());
                let t = Tokens::get(cx);
                floating(cx)
                    .w(px(POPOVER_WIDTH))
                    .pt(SPACE_2)
                    .px(SPACE_1H)
                    .pb(SPACE_1H)
                    .child(header(&view, &agents, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(ROW_GAP))
                            .children(rest_rows(&view, &agents, cx)),
                    )
                    .child(
                        div()
                            .mt(SPACE_1H)
                            .pt(SPACE_1)
                            .border_t_1()
                            .border_color(t.border)
                            .child(every_account(&view, &agents, cx)),
                    )
                    .into_any_element()
            }),
    )
}

/// One window's ruler: a tick for each 4 points, lit for as much as is left, or none lit when
/// nothing was read.
fn ruler(used: Option<f64>, t: &Tokens) -> gpui_kit::Canvas<()> {
    let color = signal(used.unwrap_or(0.), t);
    let unlit = Hsla {
        a: 0.10,
        ..t.text_primary
    };
    let left = used.map_or(0., |used| (100. - used).clamp(0., 100.));
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let lit = f32::from(bounds.size.width) * (left / 100.) as f32;
            let mut x = 0.;
            while x < f32::from(bounds.size.width) - 1. {
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.origin.x + px(x), bounds.origin.y),
                        size(px(1.5), bounds.size.height),
                    ),
                    if x < lit { color } else { unlit },
                ));
                x += 4.;
            }
        },
    )
}

/// How far through its window the clock is, as a share: nothing when a stale reading would
/// make the mark drift, or the window never said how long it is or when it resets.
fn time_gone(window: &UsageWindow, now: u64, stale: bool) -> Option<f32> {
    if stale || window.is_past_reset(now) {
        return None;
    }
    let length = window.window_minutes.filter(|minutes| *minutes > 0)?;
    let reset = window.resets_at.as_ref()?.unix_ms()?;
    let left = reset.saturating_sub(now) as f64 / 60_000.;
    Some(((length as f64 - left) / length as f64).clamp(0., 1.) as f32)
}

fn tip(window: &UsageWindow, now: u64, stale: bool) -> String {
    let mut parts = vec![format!("{}: {}.", window.label, left(window.used_at(now)))];
    if let Some(gone) = time_gone(window, now, stale) {
        parts.push(format!("{}% of the time has gone.", (gone * 100.).round()));
    }
    parts.push(match reset_at(window) {
        Some(reset) => format!("Resets {}.", time_of_day(reset)),
        None => "Reset time unavailable.".to_owned(),
    });
    parts.join(" ")
}

/// A reset as the panel says it: the time today, else the day and the time.
fn time_of_day(at: chrono::DateTime<chrono::Utc>) -> String {
    let at = at.with_timezone(&chrono::Local);
    let days = (at.date_naive() - chrono::Local::now().date_naive()).num_days();
    let time = at.format("%-I:%M %p");
    match days {
        ..=0 => time.to_string(),
        1..=6 => format!("{} {time}", at.format("%a")),
        _ => format!("{}, {time}", at.format("%b %-d")),
    }
}

/// When a window resets, as a person would say it: today, tomorrow, a weekday or a date.
pub(crate) fn day_and_time(at: chrono::DateTime<chrono::Utc>) -> String {
    let at = at.with_timezone(&chrono::Local);
    let days = (at.date_naive() - chrono::Local::now().date_naive()).num_days();
    let time = at.format("%-I:%M %p");
    match days {
        ..=0 => format!("today {time}"),
        1 => format!("tomorrow {time}"),
        2..=6 => format!("{} {time}", at.format("%a")),
        _ => format!("{}, {time}", at.format("%b %-d")),
    }
}

/// The colour of a window used `percent` full: quiet until it nears its limit.
pub(crate) fn signal(percent: f64, t: &Tokens) -> Hsla {
    if percent >= 90. {
        t.danger
    } else if percent >= 70. {
        t.warn
    } else {
        Hsla {
            a: 0.62,
            ..t.text_primary
        }
    }
}

/// When a window starts again: Unix seconds, as a number or digits, or an ISO time.
pub(crate) fn reset_at(window: &UsageWindow) -> Option<chrono::DateTime<chrono::Utc>> {
    let at = window.resets_at.as_ref()?.unix_ms()?;
    chrono::DateTime::from_timestamp_millis(i64::try_from(at).ok()?)
}

impl Workbench {
    /// Whether the rail is up and its last draw held the panel. On an SSH host or in Goals the
    /// rail draws something else, and a peeking rail draws it with the rail kept closed.
    fn limits_panel_shown(&self) -> bool {
        (self.rails.tab_open || self.rails.peeking == Some(Edge::End))
            && self.accounts.limits.panel_drawn.get()
    }

    /// Whether plan limits are on screen: the panel or the top bar's popover. The chip alone
    /// never counts, since each read boots the agent's CLI.
    pub(crate) fn limits_shown(&self) -> bool {
        self.limits_panel_shown() || self.accounts.limits.popover
    }

    /// Lets go of what a panel or chip no longer on screen was holding open.
    pub(crate) fn settle_limits(&mut self) {
        if !self.limits_panel_shown() {
            self.accounts.limits.pointer_gone();
        }
        if self.accounts.limits.popover && launchers(self, true).is_empty() {
            self.accounts.limits.popover = false;
        }
    }

    /// Reads the plan limits again: each lookup reaches the agent's service on the person's
    /// sign-in, so it runs as the app starts, every few minutes while they show, and when the
    /// person asks.
    pub fn read_usage(&self) {
        Target::here(&self.asks).call_typed::<call::ReadAgentUsages>(NoParams {});
    }
}
