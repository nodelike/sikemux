//! The Limits panel: how much of its plan each account has used, and signing one
//! in.

use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_view::prefs::TabRailPage;
use std::f32::consts::FRAC_PI_2;

use gpui_kit::{
    canvas, div, fill, point, px, size, AnyElement, App, Bounds, Context, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled,
};
use sikemux_core::agent_accounts::usage::{Usage, UsageWindow};
use sikemux_core::agent_accounts::Profile;
use sikemux_core::server::agent_accounts::AccountUsage;

use crate::core::Target;
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_0, SPACE_2, SPACE_2H, TEXT_2XS, TEXT_3XS, TEXT_3XS_H, TEXT_XS};
use crate::ui::IconButton;
use crate::workbench::Workbench;
use crate::{motion, theme, ui};

/// The account new chats here start with, and the person's other accounts with the agent, which
/// open below it while the pointer rests on the panel.
pub(crate) fn limits(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<impl IntoElement> {
    let t = Tokens::get(cx).clone();
    if bench.rails.all_harnesses {
        return None;
    }
    let launcher = bench.launcher()?;
    let compact = bench.rail_compact();
    let provider = launcher.provider.clone();
    let agent = bench.agent_name(&provider).to_owned();
    let current = bench.accounts.current(&provider);
    let seen = |profile: &Profile| {
        let status =
            crate::accounts::status(bench, profile).and_then(|status| status.as_ref().ok());
        Shown {
            id: Some(profile.id.clone()),
            profile: Some(profile.clone()),
            name: crate::accounts::name(bench, profile, status),
            usage: crate::accounts::usage(bench, profile).cloned(),
            signed_out: status.is_some_and(|status| !status.signed_in),
        }
    };
    let top = match current {
        Some(profile) => seen(profile),
        None => Shown {
            id: None,
            profile: None,
            name: agent.clone(),
            usage: bench.usage_of(&launcher.id).cloned(),
            signed_out: launcher.status.as_deref() == Some("signedOut"),
        },
    };
    let others: Vec<Shown> = bench
        .accounts
        .of(&provider)
        .into_iter()
        .filter(|profile| Some(&profile.id) != current.map(|current| &current.id))
        .map(seen)
        .collect();
    let open = bench.accounts.limits_open && !others.is_empty();
    let (top_label, top_signed_out) = (top.name.clone(), top.signed_out);
    let top = if top.signed_out {
        let title = match top.id {
            Some(_) => format!("Sign in to {}", top.name),
            None => format!("Sign in to {agent}"),
        };
        let line = match top.id {
            Some(_) => "This account is signed out",
            None => "See your 5-hour and weekly limits",
        };
        let (provider, id) = (provider.clone(), top.id.clone());
        sign_in_card(&launcher.provider, title, line, cx)
            .on_click(cx.listener(move |bench, _, window, cx| {
                crate::accounts::open_sign_in(bench, &provider, id.as_deref(), window, cx)
            }))
            .into_any_element()
    } else {
        limit_rows(&top, compact, cx)
    };
    let top_name = (!top_signed_out).then(|| {
        ui::text(top_label.clone(), TEXT_2XS, FontWeight::MEDIUM)
            .px(px(3.))
            .truncate()
            .text_color(t.text_primary)
    });
    let more = (!others.is_empty()).then(|| {
        let tall: f32 = others
            .iter()
            .map(|shown| shown.height(compact) + BLOCK_GAP)
            .sum();
        let blocks: Vec<AnyElement> = others
            .iter()
            .map(|shown| account_block(shown, &provider, compact, cx))
            .collect();
        let reveal = motion::animated(
            "limits-others",
            if open { 1. } else { 0. },
            motion::UNFOLD,
            move |share| {
                div()
                    .h(px(tall * share))
                    .overflow_hidden()
                    .opacity(share)
                    .flex()
                    .flex_col()
                    .pt(px(BLOCK_GAP))
                    .gap(px(BLOCK_GAP))
                    .children(blocks)
                    .into_any_element()
            },
        );
        let chevron = motion::turn(
            "limits-more",
            if open { -FRAC_PI_2 } else { FRAC_PI_2 },
            motion::LIST_FOLD,
            ui::icon(Icon::Chevron, 9.).text_color(t.text_tertiary),
        );
        (
            reveal,
            div()
                .h(px(14.))
                .mt(SPACE_0)
                .flex()
                .justify_center()
                .items_center()
                .child(chevron),
        )
    });
    let (reveal, chevron) = more.unzip();
    let bottom = match (chevron.is_some(), compact) {
        (true, true) => 4.,
        (true, false) => 6.,
        (false, true) => 12.,
        (false, false) => 16.,
    };
    let header = div()
        .h(px(18.))
        .mb(px(if compact { 8. } else { 9. }))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            ui::text("LIMITS", TEXT_2XS, FontWeight::MEDIUM)
                .letter_spacing(0.08)
                .text_color(t.text_tertiary),
        )
        .child(div().flex_1().h(px(1.)).bg(t.border))
        .children(top_name)
        .child({
            let provider = provider.clone();
            IconButton::new(
                "refresh-limits",
                Icon::Refresh,
                format!("Refresh {agent} plan limits"),
            )
            .small()
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.read_usage();
                crate::accounts::ask_again(bench, &provider, cx);
            }))
        })
        .child({
            let provider = provider.clone();
            IconButton::new("add-account", Icon::Plus, format!("Add a {agent} account"))
                .small()
                .on_click(cx.listener(move |bench, _, window, cx| {
                    crate::accounts::open_sign_in(bench, &provider, None, window, cx)
                }))
        });
    Some(
        div()
            .flex_none()
            .mt(px(if compact { 10. } else { 12. }))
            .mx(SPACE_2H)
            .mb(px(bottom))
            .child(header)
            .child(
                div()
                    .id("limits-content")
                    .on_hover(cx.listener(|bench, inside: &bool, _, cx| {
                        if bench.accounts.limits_open != *inside {
                            bench.accounts.limits_open = *inside;
                            cx.notify();
                        }
                    }))
                    .child(top)
                    .children(reveal)
                    .children(chevron),
            ),
    )
}

pub(crate) const BLOCK_GAP: f32 = 10.;

pub(crate) const BLOCK_PAD_TOP: f32 = 7.;

pub(crate) const BLOCK_PAD_BOTTOM: f32 = 9.;

pub(crate) const NAME_LINE: f32 = 16.;

pub(crate) const NAME_GAP: f32 = 6.;

pub(crate) const LIMIT_ROW: f32 = 18.;

pub(crate) const SIGNED_OUT_LINE: f32 = 30.;

pub(crate) fn row_gap(compact: bool) -> f32 {
    if compact {
        2.
    } else {
        3.
    }
}

/// One account as the Limits panel shows it.
pub(crate) struct Shown {
    /// The account, or none for an agent the person never added an account to.
    id: Option<String>,
    profile: Option<Profile>,
    name: String,
    usage: Option<AccountUsage>,
    signed_out: bool,
}

impl Shown {
    fn reading(&self) -> Option<&Usage> {
        self.usage.as_ref()?.usage.as_ref()
    }

    fn windows(&self) -> &[UsageWindow] {
        self.reading()
            .map_or(&[][..], |usage| usage.windows.as_slice())
    }

    /// How tall its block draws, so the panel can open onto every account at its full height.
    fn height(&self, compact: bool) -> f32 {
        if self.signed_out {
            return SIGNED_OUT_LINE;
        }
        let rows = self.windows().len().max(1) as f32;
        BLOCK_PAD_TOP
            + NAME_LINE
            + NAME_GAP
            + rows * LIMIT_ROW
            + (rows - 1.) * row_gap(compact)
            + BLOCK_PAD_BOTTOM
    }

    /// When a used-up window of the account's plan is back.
    fn back(&self) -> Option<String> {
        let full = self
            .windows()
            .iter()
            .find(|window| window.used_percent >= 100.)?;
        let label = &full.label;
        let back = reset_at(full).map(day_and_time)?;
        Some(format!("{label} full · back {back}"))
    }
}

/// What an account's plan reports: a row for each window, or why there are none.
pub(crate) fn limit_rows(shown: &Shown, compact: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let problem = shown
        .usage
        .as_ref()
        .and_then(|usage| usage.problem.as_ref());
    let windows = shown.windows();
    let quiet = |text: String| {
        ui::text(text, TEXT_3XS_H, FontWeight::NORMAL)
            .h(px(LIMIT_ROW))
            .px(SPACE_2)
            .truncate()
            .italic()
            .text_color(t.text_tertiary)
    };
    if shown.reading().is_none() && problem.is_none() {
        quiet("Refresh to read plan limits.".into()).into_any_element()
    } else if problem.is_some() {
        quiet("Could not read plan limits. Refresh to try again.".into()).into_any_element()
    } else if windows.is_empty() {
        let reason = (shown.reading())
            .and_then(|usage| usage.unavailable_reason.as_deref())
            .unwrap_or("This account did not report plan limits.");
        quiet(reason.to_owned()).into_any_element()
    } else {
        let key = shown.id.clone().unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap(px(row_gap(compact)))
            .children(windows.iter().map(|window| limit_row(window, &key, cx)))
            .into_any_element()
    }
}

/// Another of the person's accounts: its name, when a full window is back, and its rows; a click
/// starts new chats on it.
pub(crate) fn account_block(
    shown: &Shown,
    provider: &str,
    compact: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let key = shown.id.clone().unwrap_or_else(|| provider.to_owned());
    let group = SharedString::from(format!("limits-account-{key}"));
    let id = SharedString::from(format!("limits-block-{key}"));
    if shown.signed_out {
        let (provider, account) = (provider.to_owned(), shown.id.clone());
        return div()
            .id(id)
            .h(px(SIGNED_OUT_LINE))
            .px(SPACE_2)
            .flex()
            .items_center()
            .child(
                ui::text(shown.name.clone(), TEXT_2XS, FontWeight::MEDIUM)
                    .flex_1()
                    .truncate()
                    .text_color(t.text_tertiary),
            )
            .child(
                ui::text("Sign in", TEXT_3XS_H, FontWeight::NORMAL)
                    .id(SharedString::from(format!("limits-sign-in-{key}")))
                    .cursor_pointer()
                    .text_color(t.text_secondary)
                    .underline()
                    .hover(|link| link.text_color(t.text_primary))
                    .on_click(cx.listener(move |bench, _, window, cx| {
                        crate::accounts::open_sign_in(
                            bench,
                            &provider,
                            account.as_deref(),
                            window,
                            cx,
                        )
                    })),
            )
            .into_any_element();
    }
    let line = div()
        .h(px(NAME_LINE))
        .mb(px(NAME_GAP))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            ui::text(shown.name.clone(), TEXT_2XS, FontWeight::MEDIUM)
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .child(
            div()
                .relative()
                .flex_none()
                .children(shown.back().map(|back| {
                    ui::text(back, TEXT_3XS, FontWeight::NORMAL)
                        .font_features(theme::tabular())
                        .text_color(t.danger)
                        .group_hover(group.clone(), |note| note.opacity(0.))
                }))
                .child(
                    ui::text("Use for new chats", TEXT_3XS, FontWeight::NORMAL)
                        .absolute()
                        .top_0()
                        .right_0()
                        .whitespace_nowrap()
                        .opacity(0.)
                        .text_color(t.text_primary)
                        .group_hover(group.clone(), |text| text.opacity(1.)),
                ),
        );
    let profile = shown.profile.clone();
    div()
        .id(id)
        .group(group.clone())
        .h(px(shown.height(compact)))
        .overflow_hidden()
        .pt(px(BLOCK_PAD_TOP))
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .hover(|block| block.bg(t.surface_hover))
        .on_click(cx.listener(move |bench, _, _, cx| {
            if let Some(profile) = profile.as_ref() {
                crate::accounts::select(bench, profile, cx);
            }
        }))
        .child(line)
        .child(limit_rows(shown, compact, cx))
        .into_any_element()
}

/// One of a plan's rolling windows: how much of it is used, and when it starts again.
pub(crate) fn limit_row(window: &UsageWindow, key: &str, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let label = window.label.clone();
    let percent = window.used_percent.clamp(0., 100.);
    let signal = signal(percent, &t);
    let reset = reset_at(window);
    let note = match reset {
        Some(at) => format!(
            "Resets {}",
            at.with_timezone(&chrono::Local)
                .format("%-m/%-d/%Y, %-I:%M:%S %p")
        ),
        None => "Reset time unavailable".into(),
    };
    div()
        .id(SharedString::from(format!("limit-{key}-{label}")))
        .h(px(LIMIT_ROW))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .tooltip(ui::tooltip(format!(
            "{label}: {}% used. {note}",
            percent.round()
        )))
        .child(
            ui::meta(label, cx)
                .w(px(58.))
                .flex_none()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap(),
        )
        .child(ticks(percent, signal, &t).flex_1().h(px(10.)))
        .child(
            ui::text(
                format!("{}%", percent.round()),
                TEXT_2XS,
                FontWeight::NORMAL,
            )
            .w(px(28.))
            .flex_none()
            .text_right()
            .font_features(theme::tabular())
            .text_color(if percent < 70. {
                t.text_primary
            } else {
                signal
            }),
        )
        .child(
            ui::text(ui::time::countdown(reset), TEXT_3XS, FontWeight::NORMAL)
                .w(px(42.))
                .flex_none()
                .text_right()
                .font_features(theme::tabular())
                .text_color(t.text_tertiary),
        )
}

pub(crate) fn ticks(percent: f64, signal: Hsla, t: &Tokens) -> gpui_kit::Canvas<()> {
    let unlit = Hsla {
        a: 0.10,
        ..t.text_primary
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let lit = bounds.size.width * (percent / 100.) as f32;
            let mut x = px(0.);
            while x < bounds.size.width {
                let tick = Bounds::new(
                    point(bounds.origin.x + x, bounds.origin.y),
                    size(px(1.5), bounds.size.height),
                );
                window.paint_quad(fill(tick, if x < lit { signal } else { unlit }));
                x += px(4.);
            }
        },
    )
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

/// The call to sign in, in the agent's own colours: its mark, what signing in shows, and a button.
pub(crate) fn sign_in_card(
    provider: &str,
    title: String,
    line: &'static str,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    let brand = theme::brand(provider);
    let tint = |a: f32| Hsla { a, ..brand };
    div()
        .id("limits-sign-in")
        .mx(SPACE_0)
        .py(SPACE_2H)
        .px(px(11.))
        .flex()
        .items_center()
        .gap(px(11.))
        .rounded(theme::RADIUS_3)
        .border_1()
        .border_color(tint(0.22))
        .bg(gpui_kit::linear_gradient(
            100.,
            gpui_kit::linear_color_stop(tint(0.13), 0.),
            gpui_kit::linear_color_stop(
                Hsla {
                    a: 0.02,
                    ..t.text_primary
                },
                0.7,
            ),
        ))
        .cursor_pointer()
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_2)
                .bg(tint(0.16))
                .child(ui::mark(provider, 18.)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    ui::text(title, TEXT_XS, FontWeight::SEMIBOLD)
                        .truncate()
                        .text_color(t.text_primary),
                )
                .child(
                    ui::text(line, TEXT_3XS_H, FontWeight::NORMAL)
                        .truncate()
                        .text_color(t.text_tertiary),
                ),
        )
        .child(
            ui::text("Sign in", TEXT_2XS, FontWeight::SEMIBOLD)
                .flex_none()
                .h(px(24.))
                .px(px(11.))
                .flex()
                .items_center()
                .rounded(theme::RADIUS_2)
                .bg(brand)
                .text_color(if brand.l > 0.7 {
                    gpui_kit::black()
                } else {
                    gpui_kit::white()
                }),
        )
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
    /// Whether the Limits panel is on screen: the tab rail open on its agents page, for one
    /// agent.
    pub(crate) fn limits_shown(&self) -> bool {
        self.rails.tab_open
            && self.rails.page == TabRailPage::Agents
            && !self.rails.all_harnesses
            && self.launcher().is_some()
    }

    /// Reads the plan limits again: each lookup reaches the agent's service on the person's
    /// sign-in, so it runs as the app starts, every few minutes while the Limits panel shows,
    /// and when the person asks.
    pub fn read_usage(&self) {
        Target::here(&self.asks).call_typed::<call::ReadAgentUsages>(NoParams {});
    }
}
