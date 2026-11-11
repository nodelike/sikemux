//! The Limits panel: how much of its plan each agent's account has used, drawn as a ruler per
//! agent, with every account in a float over the rows and in the top bar's popover.

use std::cell::Cell;
use std::f32::consts::TAU;
use std::time::Duration;

use gpui_kit::base::Popover;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, canvas, deferred, div, fill, img, point, px, size, Anchor, AnchoredPositionMode,
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Div, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Task, Transformation, WeakEntity, Window,
};
use sikemux_core::agent_accounts::usage::{ResetKind, ResetOutcome, UsageWindow};
use sikemux_core::agent_accounts::Profile;
use sikemux_core::server::agent_accounts::{AccountUsage, Unpicking, UsedReset, UsingReset};
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_view::workspace::LauncherInfo;

use crate::core::Target;
use crate::icons::Icon;
use crate::rail::Edge;
use crate::theme::{
    Tokens, RADIUS_1, RADIUS_2, RADIUS_3, RADIUS_4, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, TEXT_2XS,
    TEXT_2XS_H, TEXT_3XS, TEXT_3XS_H,
};
use crate::ui::IconButton;
use crate::workbench::Workbench;
use crate::{motion, theme, toast, ui};

const OPEN_AFTER: Duration = Duration::from_millis(120);
const CLOSE_AFTER: Duration = Duration::from_millis(180);
const UNDO_FOR: Duration = Duration::from_secs(6);
/// A read that never answers, as when the core restarts, stops spinning after this.
const READING_AT_MOST: Duration = Duration::from_secs(60);
/// How old a reading may be when limits come into view, as claude.ai reads its own on every look.
const FRESH_FOR: Duration = Duration::from_secs(20);

const ROW: f32 = 30.;
const ROW_COMPACT: f32 = 28.;
const ROW_GAP: f32 = 2.;
const HEADER: f32 = 24.;
const AGENT_HEAD: f32 = 34.;
const ACCOUNT_LINE: f32 = 24.;
const NAME_LINE: f32 = 20.;
const NAME_GAP: f32 = 5.;
const WINDOW_LINE: f32 = 17.;
const ACCOUNT_PAD: f32 = 8.;
const RESETS_HEAD: f32 = 22.;
const RESET_ROW: f32 = 24.;
const RESET_ASKING: f32 = 84.;
const LABEL_WIDTH: f32 = 44.;
const AVATAR: f32 = 18.;
const FLOAT_GAP: f32 = 6.;
/// The smallest float worth scrolling, and what it leaves of the rail above it.
const FLOAT_LEAST: f32 = 120.;
const RAIL_HEAD: f32 = 41.;
const FLOAT_MARGIN: f32 = 52.;
const POPOVER_WIDTH: f32 = 300.;
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
    /// The reset pressed once, by account and kind, waiting for the press that spends it.
    arming: Option<String>,
    /// The reset being spent, by account and kind.
    resetting: Option<String>,
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

    fn session(&self) -> Option<&UsageWindow> {
        self.usage?.usage.as_ref()?.session()
    }

    /// The windows an account draws, labelled: the session's, the week's, then each model's.
    fn windows(&self) -> Vec<(String, &UsageWindow)> {
        let Some(usage) = self.usage.and_then(|usage| usage.usage.as_ref()) else {
            return Vec::new();
        };
        let plan = [("Session", usage.session()), ("Week", usage.week())]
            .into_iter()
            .filter_map(|(label, window)| Some((label.to_owned(), window?)));
        let models =
            (usage.windows.iter()).filter_map(|window| Some((window.scope.clone()?, window)));
        plan.chain(models).collect()
    }

    fn tier(&self) -> Option<&str> {
        self.usage?.usage.as_ref()?.tier.as_deref()
    }

    fn resets(&self) -> &[ResetKind] {
        (self.usage)
            .and_then(|usage| usage.usage.as_ref()?.resets.as_ref())
            .map_or(&[], |resets| resets.kinds.as_slice())
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
        let next = accounts
            .iter()
            .position(|shown| shown.usage.is_some_and(|usage| usage.next));
        let in_use = next
            .or_else(|| {
                (accounts.iter())
                    .position(|shown| shown.profile.map(|profile| &profile.id) == current)
            })
            .unwrap_or(0);
        Self {
            provider,
            name,
            accounts,
            in_use,
        }
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
    arming: Option<String>,
    resetting: Option<String>,
    /// The account a pick came from, with Undo on its row.
    undo: Option<String>,
    /// How tall the float's list may grow before it scrolls.
    room: f32,
    /// Whether smart accounts places new chats, so a pick pins them rather than moving any.
    smart: bool,
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
    let rows = rest_accounts(&agents, !every).len();
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
                let float = div()
                    .id("limits-float")
                    .w(px(POPOVER_WIDTH))
                    .pb(px(FLOAT_GAP))
                    .occlude()
                    .on_hover(hovered(view.this.clone(), |reveal| &mut reveal.over_float))
                    .child(
                        floating(cx)
                            .child(every_account(&view, &agents, cx))
                            .with_animation(
                                "limits-float-in",
                                motion::entrance(motion::POP_UP),
                                |float, shown| {
                                    float.opacity(shown).relative().top(px(4. * (1. - shown)))
                                },
                            ),
                    );
                panel.child(
                    div().absolute().top_0().right_0().child(
                        deferred(
                            anchored()
                                .position_mode(AnchoredPositionMode::Local)
                                .anchor(Anchor::BottomRight)
                                .snap_to_window_with_margin(px(8.))
                                .child(float),
                        )
                        .with_priority(1),
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
        arming: reveal.arming.clone(),
        resetting: reveal.resetting.clone(),
        undo: reveal
            .undo
            .as_ref()
            .map(|undo| undo.undoing.back_to.clone()),
        room,
        smart: bench.accounts.smart(),
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
        .children(resets_badge(view, agents, cx))
        .child(refresh_button(view, agents, cx))
        .children(add)
}

/// How many limit resets the accounts shown hold between them, when any.
fn resets_badge(view: &View, agents: &[Agent], cx: &App) -> Option<Stateful<Div>> {
    let left: u32 = (rest_accounts(agents, view.solo).into_iter())
        .flat_map(|(agent, place)| agent.accounts[place].resets())
        .map(|kind| kind.left)
        .sum();
    if left == 0 {
        return None;
    }
    let t = Tokens::get(cx);
    let plural = if left == 1 { "" } else { "s" };
    Some(
        div()
            .id("limits-resets")
            .h(px(18.))
            .px(px(5.))
            .flex()
            .items_center()
            .gap(px(3.))
            .rounded(RADIUS_1)
            .bg(t.surface_hover)
            .text_color(t.text_secondary)
            .tooltip(ui::tooltip(format!(
                "{left} limit reset{plural} across these accounts"
            )))
            .child(ui::icon(Icon::Refresh, 10.).text_color(t.text_secondary))
            .child(ui::text(left.to_string(), TEXT_3XS_H, FontWeight::MEDIUM)),
    )
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
    let every = FRESH_FOR.as_millis() as u64;
    (bench.reads.usage.iter())
        .filter(|account| account.signed_in)
        .any(|account| {
            account
                .read_at
                .is_none_or(|read| now.saturating_sub(read) >= every)
        })
}

/// One of an agent's windows, added up over the accounts that have read it.
struct Total {
    /// Points left, out of 100 for each account counted.
    left: f64,
    accounts: usize,
}

type Pick = for<'b, 'c> fn(&'b Shown<'c>) -> Option<&'b UsageWindow>;

impl Total {
    fn of(agent: &Agent, now: u64, window: Pick) -> Option<Self> {
        let left: Vec<f64> = (agent.accounts.iter())
            .filter(|shown| !shown.signed_out)
            .filter_map(|shown| Some(100. - window(shown)?.used_at(now)))
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

/// The accounts the rail lists, each by its agent and its place among that agent's accounts.
fn rest_accounts<'b, 'a>(agents: &'b [Agent<'a>], solo: bool) -> Vec<(&'b Agent<'a>, usize)> {
    let shown = if solo { &agents[..1] } else { agents };
    (shown.iter())
        .flat_map(|agent| (0..agent.accounts.len()).map(move |place| (agent, place)))
        .collect()
}

/// What an account's rail row draws: its session over its week, else its week alone. A model's
/// own week, such as Fable's, is left to the card.
fn rail_limits<'s>(shown: &'s Shown) -> Vec<(&'static str, &'s UsageWindow)> {
    [("Session", shown.session()), ("Week", shown.week())]
        .into_iter()
        .filter_map(|(name, window)| Some((name, window?)))
        .collect()
}

fn rest_rows(view: &View, agents: &[Agent], cx: &App) -> Vec<AnyElement> {
    let accounts = rest_accounts(agents, view.solo);
    let two =
        (accounts.iter()).any(|(agent, place)| rail_limits(&agent.accounts[*place]).len() > 1);
    let numbers = if two { 66. } else { 36. };
    (accounts.into_iter())
        .map(|(agent, place)| rest_row(view, agent, place, numbers, cx))
        .collect()
}

/// An account's avatar with its agent's mark on its corner.
fn face(provider: &str, name: &str, place: usize, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .relative()
        .flex_none()
        .size(px(16.))
        .child(img(super::marble::avatar(name, place)).size(px(16.)))
        .child(
            div()
                .absolute()
                .right(px(-3.))
                .bottom(px(-3.))
                .size(px(11.))
                .rounded_full()
                .bg(t.surface)
                .flex()
                .items_center()
                .justify_center()
                .child(ui::mark(provider, 8.)),
        )
}

/// How low a limit is, in the colour of its number: quiet until it nears its end.
fn left_ink(used: f64, t: &Tokens, quiet: Hsla) -> Hsla {
    match used {
        used if used < 70. => quiet,
        used => signal(used, t),
    }
}

/// A resting row: an account's face and name, its session over its week, and what is left of
/// each, every ruler as long as the others so the accounts compare at a glance.
fn rest_row(view: &View, agent: &Agent, place: usize, numbers: f32, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let shown = &agent.accounts[place];
    let limits = rail_limits(shown);
    let id = SharedString::from(format!(
        "limit-{}-{}",
        agent.provider,
        shown.key(agent.provider)
    ));
    let in_use = place == agent.in_use && agent.accounts.len() > 1;
    let name = ui::text(shown.name.clone(), TEXT_2XS, FontWeight::NORMAL)
        .flex_none()
        .w(px(76.))
        .truncate()
        .text_color(t.text_secondary);
    let row = div()
        .id(id)
        .h(px(row_height(view)))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .when(in_use, |row| row.bg(t.surface_chosen))
        .child(face(agent.provider, &shown.name, place, cx))
        .child(name);
    if shown.signed_out {
        return row
            .cursor_pointer()
            .hover(|row| row.bg(t.surface_hover))
            .on_click(sign_in(view, agent.provider, shown))
            .child(div().flex_1())
            .child(go("Sign in", cx))
            .into_any_element();
    }
    let Some((_, main)) = limits.first() else {
        return row
            .tooltip(ui::tooltip(
                shown.no_limits_reason().unwrap_or_else(|| shown.nothing()),
            ))
            .child(ruler(None, t).flex_1().min_w_0().h(px(8.)))
            .child(
                div()
                    .flex_none()
                    .w(px(numbers))
                    .flex()
                    .justify_end()
                    .child(note("–".into(), cx)),
            )
            .into_any_element();
    };
    let second = limits.get(1).map(|(_, window)| *window);
    let tip = (limits.iter())
        .map(|(name, window)| format!("{name} {}", left(window.used_at(view.now))))
        .collect::<Vec<_>>()
        .join(" · ");
    let rulers = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(ruler(Some(main.used_at(view.now)), t).h(px(8.)))
        .children(second.map(|window| ruler(Some(window.used_at(view.now)), t).h(px(3.))));
    let percent = |window: &UsageWindow| format!("{}%", (100. - window.used_at(view.now)).round());
    let figures = div()
        .flex_none()
        .w(px(numbers))
        .flex()
        .justify_end()
        .items_baseline()
        .gap(px(4.))
        .font_features(theme::tabular())
        .child(
            ui::text(percent(main), TEXT_2XS, FontWeight::MEDIUM).text_color(left_ink(
                main.used_at(view.now),
                t,
                t.text_primary,
            )),
        )
        .children(second.map(|window| {
            ui::text(percent(window), TEXT_2XS, FontWeight::NORMAL).text_color(left_ink(
                window.used_at(view.now),
                t,
                t.text_tertiary,
            ))
        }));
    row.tooltip(ui::tooltip(tip))
        .child(rulers)
        .child(figures)
        .into_any_element()
}

fn row_height(view: &View) -> f32 {
    if view.compact {
        ROW_COMPACT
    } else {
        ROW
    }
}

/// A window's short name, such as Week, at the start of its line.
fn window_label(label: impl Into<SharedString>, width: f32, cx: &App) -> Div {
    ui::text(label.into(), TEXT_3XS, FontWeight::MEDIUM)
        .flex_none()
        .w(px(width))
        .text_color(Tokens::get(cx).text_tertiary)
}

fn percent_cell(window: &UsageWindow, now: u64, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let used = window.used_at(now);
    ui::text(left(used), TEXT_2XS, FontWeight::NORMAL)
        .flex_none()
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
        (_, None, _) => String::new(),
        (false, Some(reset), _) => ui::time::countdown(Some(reset)),
    };
    ui::text(text, TEXT_3XS, FontWeight::NORMAL)
        .flex_none()
        .text_color(if full {
            t.text_primary
        } else {
            t.text_tertiary
        })
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

/// Every account of every agent, each across the float's full width between hairlines, the one
/// in use lifted.
fn every_account(view: &View, agents: &[Agent], cx: &App) -> Stateful<Div> {
    let t = Tokens::get(cx);
    let hairline = || div().h(px(1.)).flex_none().bg(t.border);
    let mut heights = Vec::new();
    let mut items: Vec<AnyElement> = Vec::new();
    for (at, agent) in agents.iter().enumerate() {
        if at > 0 {
            heights.push(1.);
            items.push(hairline().into_any_element());
        }
        heights.push(AGENT_HEAD + 1.);
        items.push(agent_head(view, agent, cx).into_any_element());
        items.push(hairline().into_any_element());
        for (index, shown) in agent.accounts.iter().enumerate() {
            if index > 0 {
                heights.push(1.);
                items.push(hairline().into_any_element());
            }
            let last = at + 1 == agents.len() && index + 1 == agent.accounts.len();
            heights.push(account_height(view, shown));
            items.push(account_line(view, agent, shown, index, last, cx));
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

fn agent_head(view: &View, agent: &Agent, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .h(px(AGENT_HEAD))
        .px(SPACE_2H)
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

/// Whether an account draws as one short line rather than its name over its windows.
fn one_line(view: &View, shown: &Shown) -> bool {
    shown.signed_out || (shown.id().is_some() && view.undo.as_deref() == shown.id())
}

fn account_height(view: &View, shown: &Shown) -> f32 {
    if one_line(view, shown) {
        return ACCOUNT_LINE + ACCOUNT_PAD;
    }
    let resets = shown.resets();
    let asking =
        (resets.iter()).any(|kind| view.arming.as_deref() == Some(reset_key(shown, kind).as_str()));
    let resets_height = match resets.is_empty() {
        true => 0.,
        false => {
            RESETS_HEAD
                + resets.len() as f32 * RESET_ROW
                + if asking { RESET_ASKING - RESET_ROW } else { 0. }
        }
    };
    NAME_LINE
        + NAME_GAP
        + shown.windows().len().max(1) as f32 * WINDOW_LINE
        + resets_height
        + ACCOUNT_PAD * 2.
}

/// An account: its avatar, name and plan, a ruler for each of its limits, then the limit resets
/// it holds. Clicking one not in use moves the agent's chats onto it; while smart accounts is on,
/// clicking one pins new chats to it, or unpins it.
fn account_line(
    view: &View,
    agent: &Agent,
    shown: &Shown,
    place: usize,
    last: bool,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let in_use = place == agent.in_use;
    let key = shown.key(agent.provider);
    let id = SharedString::from(format!("limits-account-{}-{key}", agent.provider));
    let group = SharedString::from(format!("limits-line-{key}"));
    let line = |height: f32| div().h(px(height)).flex().items_center().gap(SPACE_2);
    let name = |text: String| {
        ui::text(text, TEXT_2XS_H, FontWeight::SEMIBOLD)
            .min_w_0()
            .truncate()
            .text_color(t.text_primary)
    };
    let meta = |text: String| {
        ui::text(text, TEXT_3XS, FontWeight::NORMAL)
            .flex_none()
            .text_color(t.text_tertiary)
    };
    let frame = || {
        div()
            .id(id.clone())
            .px(SPACE_2H)
            .py(px(ACCOUNT_PAD))
            .flex()
            .flex_col()
            .when(last, |account| account.rounded_b(RADIUS_4))
    };
    if one_line(view, shown) {
        let short = line(ACCOUNT_LINE);
        if shown.signed_out {
            let names = match agent.unnamed(shown) && in_use {
                true => short.child(name("Signed out".into()).flex_1()),
                false => short
                    .child(name(shown.name.clone()))
                    .child(meta("Signed out".into()).flex_1()),
            };
            return frame()
                .py(px(ACCOUNT_PAD / 2.))
                .cursor_pointer()
                .hover(|account| account.bg(t.surface_hover))
                .on_click(sign_in(view, agent.provider, shown))
                .child(names.child(go("Sign in", cx)))
                .into_any_element();
        }
        return frame()
            .py(px(ACCOUNT_PAD / 2.))
            .child(
                short.child(name(shown.name.clone()).flex_1()).child(
                    go("Undo", cx)
                        .id(SharedString::from(format!("limits-undo-{key}")))
                        .cursor_pointer()
                        .hover(|link| link.text_color(t.text_primary))
                        .on_click(view.on(move |bench, _, cx| undo_pick(bench, cx))),
                ),
            )
            .into_any_element();
    }
    let windows = shown.windows();
    let stale = shown.stale(view.now);
    let age = shown.age(view.now);
    let tip = match windows.is_empty() {
        true => shown.no_limits_reason(),
        false => Some(
            (windows.iter())
                .map(|(_, window)| tip(window, view.now, stale))
                .collect::<Vec<_>>()
                .join(" "),
        ),
    };
    let tip = match (status(view, shown).filter(|_| !windows.is_empty()), tip) {
        (Some(status), Some(tip)) => Some(format!("{status}. {tip}")),
        (status, tip) => tip.or(status),
    };
    let window_line = |label: Option<&str>, window: Option<&UsageWindow>| {
        line(WINDOW_LINE)
            .font_features(theme::tabular())
            .when_some(label, |line, label| {
                line.child(window_label(label.to_owned(), LABEL_WIDTH, cx))
            })
            .child(
                ruler(window.map(|window| window.used_at(view.now)), t)
                    .flex_1()
                    .min_w_0()
                    .h(px(8.)),
            )
            .map(|line| match window {
                Some(window) => line
                    .child(
                        percent_cell(window, view.now, cx)
                            .min_w(px(50.))
                            .text_right(),
                    )
                    .child(
                        when_cell(window, view.now, age.as_deref(), cx)
                            .min_w(px(40.))
                            .text_right(),
                    ),
                None => line.child(note(shown.nothing(), cx)),
            })
    };
    let lines: Vec<Div> = match windows.is_empty() {
        true => vec![window_line(None, None)],
        false => (windows.iter())
            .map(|(label, window)| window_line(Some(label.as_str()), Some(*window)))
            .collect(),
    };
    let smart = view.smart && agent.accounts.len() > 1;
    let pinned = shown.usage.is_some_and(|usage| usage.pinned);
    let tip = match (smart && in_use, tip) {
        (true, Some(tip)) => Some(format!("New chats start here. {tip}")),
        (true, None) => Some("New chats start here".to_owned()),
        (false, tip) => tip,
    };
    let target = shown.profile.filter(|_| smart || !in_use).cloned();
    let pickable = target.is_some();
    let offer = match (smart, pinned) {
        (true, true) => "Unpin",
        (true, false) => "Pin new chats here",
        (false, _) => "Move chats here",
    };
    let provider = agent.provider.to_owned();
    let names = line(NAME_LINE)
        .mb(px(NAME_GAP))
        .child(
            img(super::marble::avatar(&shown.name, place))
                .size(px(AVATAR))
                .flex_none(),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(name(shown.name.clone()))
                .children(shown.tier().map(|tier| tier_pill(tier, cx)))
                .when(smart && pinned, |names| names.child(meta("Pinned".into()))),
        )
        .when(pickable, |names| {
            names.child(
                ui::text(offer, TEXT_3XS, FontWeight::NORMAL)
                    .flex_none()
                    .text_color(t.text_primary)
                    .opacity(0.)
                    .group_hover(group.clone(), |offer| offer.opacity(1.)),
            )
        });
    frame()
        .group(group.clone())
        .when(in_use && agent.accounts.len() > 1, |account| {
            account.bg(t.surface_chosen)
        })
        .when_some(tip, |account, tip| account.tooltip(ui::tooltip(tip)))
        .when_some(target, |account, target| {
            account
                .cursor_pointer()
                .hover(|account| account.bg(t.surface_hover))
                .on_click(view.on(move |bench, _, cx| match pinned {
                    true => crate::accounts::unpin(bench, &provider, cx),
                    false => pick(bench, &target, cx),
                }))
        })
        .child(names)
        .children(lines)
        .children(resets_list(view, shown, &windows, cx))
        .into_any_element()
}

/// The plan an account is on, as its provider names it.
fn tier_pill(tier: &str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    ui::text(tier.to_owned(), TEXT_3XS, FontWeight::MEDIUM)
        .flex_none()
        .h(px(16.))
        .px(px(5.))
        .flex()
        .items_center()
        .rounded(RADIUS_1)
        .border_1()
        .border_color(t.border)
        .bg(t.surface_hover)
        .text_color(t.text_secondary)
}

/// The account's limit resets under its rulers: one row for each kind, with how many it holds,
/// until when, and Use.
fn resets_list(
    view: &View,
    shown: &Shown,
    windows: &[(String, &UsageWindow)],
    cx: &App,
) -> Option<Div> {
    let resets = shown.resets();
    if resets.is_empty() {
        return None;
    }
    let t = Tokens::get(cx);
    let left: u32 = resets.iter().map(|kind| kind.left).sum();
    let head = div()
        .h(px(RESETS_HEAD))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            ui::text("RESETS", TEXT_3XS, FontWeight::MEDIUM)
                .letter_spacing(0.08)
                .text_color(t.text_tertiary),
        )
        .child(div().flex_1().h(px(1.)).bg(t.border))
        .child(
            ui::text(format!("{left} left"), TEXT_3XS, FontWeight::NORMAL)
                .text_color(t.text_tertiary),
        );
    Some(
        div().flex().flex_col().child(head).children(
            resets
                .iter()
                .map(|kind| reset_row(view, shown, kind, windows, cx)),
        ),
    )
}

fn reset_key(shown: &Shown, kind: &ResetKind) -> String {
    let launcher = shown.usage.map_or("", |usage| usage.launcher.as_str());
    format!("{launcher}/{}", kind.label)
}

/// A reset kind as the person says it: Weekly, Fable, Full.
fn short_kind(kind: &ResetKind) -> &str {
    match kind.label.strip_suffix(" reset") {
        Some("5-hour") => "Session",
        Some(short) => short,
        None => &kind.label,
    }
}

/// The colour of a reset's last day: red within a week, amber within a month, else quiet.
fn expiry_ink(use_by: u64, now: u64, t: &Tokens) -> Hsla {
    const DAY: u64 = 86_400_000;
    match use_by.saturating_sub(now) / DAY {
        ..7 => t.danger,
        ..31 => t.warn,
        _ => t.text_tertiary,
    }
}

fn until(use_by: Option<u64>, now: u64, cx: &App) -> Option<Div> {
    let t = Tokens::get(cx);
    let at = chrono::DateTime::from_timestamp_millis(i64::try_from(use_by?).ok()?)?;
    let day = at.with_timezone(&chrono::Local).format("%b %-d");
    Some(
        ui::text(format!("until {day}"), TEXT_3XS_H, FontWeight::NORMAL)
            .flex_none()
            .text_color(expiry_ink(use_by?, now, t)),
    )
}

/// What spending one reset of `kind` does, said before it is spent, and what its button says.
fn reset_says(kind: &ResetKind, windows: &[(String, &UsageWindow)], now: u64) -> (String, String) {
    let refilled = (kind.window.as_deref())
        .and_then(|label| windows.iter().find(|(_, window)| window.label == label))
        .map(|(name, window)| (name.as_str(), *window));
    let after = match kind.left {
        1 => format!(
            "No {} resets will be left.",
            short_kind(kind).to_lowercase()
        ),
        left => format!("{} will be left.", left - 1),
    };
    let (does, button) = match refilled {
        Some((name, window)) if window.scope.is_some() => (
            format!("Refills your {name} allowance and gives back what {name} used of your week."),
            format!("Reset {name}"),
        ),
        Some((name, window)) => {
            let left = (100. - window.used_at(now)).round();
            let what = name.to_lowercase();
            (
                format!("Refills your whole {what} now, {left}% → 100% left."),
                format!("Reset {what}"),
            )
        }
        None => (
            "Starts every used-up limit over.".to_owned(),
            "Reset all".to_owned(),
        ),
    };
    (format!("{does} {after}"), button)
}

/// One kind of reset: its name and count, until when, and Use. Use opens the row to say what
/// spending one does, with Cancel and the button that spends it.
fn reset_row(
    view: &View,
    shown: &Shown,
    kind: &ResetKind,
    windows: &[(String, &UsageWindow)],
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let key = reset_key(shown, kind);
    let asking = view.arming.as_deref() == Some(key.as_str());
    let busy = view.resetting.as_deref() == Some(key.as_str());
    let spend = Spend {
        launcher: shown
            .usage
            .map_or_else(String::new, |usage| usage.launcher.clone()),
        kind: kind.id.clone(),
        account: shown.name.clone(),
        what: kind.label.clone(),
    };
    let title = div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_baseline()
        .child(
            ui::text(short_kind(kind).to_owned(), TEXT_2XS, FontWeight::MEDIUM)
                .text_color(t.text_primary),
        )
        .child(
            ui::text(format!(" ×{}", kind.left), TEXT_2XS, FontWeight::NORMAL)
                .text_color(t.text_tertiary),
        );
    let this = view.this.clone();
    let tap = move |act: fn(&mut Workbench, String, Spend, &mut Context<Workbench>)| {
        let (this, key, spend) = (this.clone(), key.clone(), spend.clone());
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            let (key, spend) = (key.clone(), spend.clone());
            let _ = this.update(cx, |bench, cx| act(bench, key, spend, cx));
        }
    };
    if !asking {
        let action = match busy {
            true => ui::text("Resetting…", TEXT_2XS, FontWeight::MEDIUM)
                .text_color(t.text_tertiary)
                .into_any_element(),
            false => ui::text("Use", TEXT_2XS, FontWeight::MEDIUM)
                .id(SharedString::from(format!(
                    "limits-use-{}",
                    reset_key(shown, kind)
                )))
                .px(px(2.))
                .text_color(t.accent)
                .cursor_pointer()
                .on_click(tap(|bench, key, _, cx| {
                    bench.accounts.limits.arming = Some(key);
                    redraw(bench, cx);
                }))
                .into_any_element(),
        };
        return div()
            .h(px(RESET_ROW))
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(title)
            .children(until(kind.use_by, view.now, cx))
            .child(action)
            .into_any_element();
    }
    let (says, button) = reset_says(kind, windows, view.now);
    div()
        .my(px(2.))
        .mx(px(-6.))
        .p(SPACE_2)
        .flex()
        .flex_col()
        .gap(px(4.))
        .rounded(RADIUS_3)
        .bg(Hsla {
            a: 0.08,
            ..t.accent
        })
        .border_1()
        .border_color(Hsla {
            a: 0.25,
            ..t.accent
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(title)
                .children(until(kind.use_by, view.now, cx)),
        )
        .child(
            ui::text(says, TEXT_3XS_H, FontWeight::NORMAL)
                .whitespace_normal()
                .line_height(px(15.))
                .text_color(t.text_secondary),
        )
        .child(
            div()
                .mt(px(4.))
                .flex()
                .justify_end()
                .items_center()
                .gap(SPACE_2)
                .child(
                    ui::text("Cancel", TEXT_2XS, FontWeight::NORMAL)
                        .id(SharedString::from(format!(
                            "limits-cancel-{}",
                            reset_key(shown, kind)
                        )))
                        .px(px(4.))
                        .text_color(t.text_secondary)
                        .cursor_pointer()
                        .hover(|cancel| cancel.text_color(t.text_primary))
                        .on_click(tap(|bench, _, _, cx| {
                            bench.accounts.limits.arming = None;
                            redraw(bench, cx);
                        })),
                )
                .child(
                    ui::Button::new(
                        SharedString::from(format!("limits-spend-{}", reset_key(shown, kind))),
                        button,
                        ui::Variant::Primary,
                    )
                    .sized(22., 9., TEXT_2XS)
                    .on_click(tap(use_reset)),
                ),
        )
        .into_any_element()
}

/// A reset to spend, and the names a toast says it by.
#[derive(Clone)]
struct Spend {
    launcher: String,
    kind: Option<String>,
    account: String,
    what: String,
}

/// Spends one of the account's resets and says how it went. The core reads its limits again.
fn use_reset(bench: &mut Workbench, key: String, spend: Spend, cx: &mut Context<Workbench>) {
    let limits = &mut bench.accounts.limits;
    limits.arming = None;
    limits.resetting = Some(key);
    redraw(bench, cx);
    let asked = Target::here(&bench.asks).request_typed::<call::UseAgentReset>(UsingReset {
        launcher: spend.launcher,
        kind: spend.kind,
    });
    let Spend { account, what, .. } = spend;
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| {
            bench.accounts.limits.resetting = None;
            let note = match answer {
                Ok(UsedReset {
                    outcome: ResetOutcome::Reset,
                    left,
                }) => toast::success(match left {
                    Some(left) => format!("{what} used on {account} · {left} left"),
                    None => format!("{what} used on {account}"),
                }),
                Ok(UsedReset {
                    outcome: ResetOutcome::NothingToReset,
                    ..
                }) => toast::info(format!(
                    "Nothing to reset on {account} yet, so none was used"
                )),
                Ok(UsedReset {
                    outcome: ResetOutcome::NoReset,
                    ..
                }) => toast::info(format!("{account} has no {what}s left")),
                Err(error) => toast::error(format!("Couldn't reset {account}: {error}")),
            };
            bench.notify(note, cx);
            redraw(bench, cx);
        });
    })
    .detach();
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
            if !wanted {
                reveal.arming = None;
            }
            if wanted && reading_is_old(bench, ui::time::now_ms()) {
                start_reading(bench, cx);
            }
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
        self.arming = None;
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

/// Reads the limits again when the window comes to the front with them on screen and the last
/// reading is a little old, so they are current whenever the person looks.
pub(crate) fn window_came_forward(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if bench.limits_shown() && reading_is_old(bench, ui::time::now_ms()) {
        start_reading(bench, cx);
    }
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
    let week: Pick = |shown| shown.week();
    let pairs: Vec<(String, Option<Total>)> = (agents.iter())
        .map(|agent| (agent.provider.to_owned(), Total::of(agent, now, week)))
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
                    bench.accounts.limits.arming = None;
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
                            .mx(-SPACE_1H)
                            .mb(-SPACE_1H)
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
