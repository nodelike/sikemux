//! What agents did across every project: the totals, the calendar, and the
//! share each model, agent and project had, priced at API rates.

use chrono::TimeZone;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, AppContext, Context, Entity, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use serde_json::{json, Map, Value};
use sikemux_core::server::activity_log::ActivityModel;
use sikemux_core::server::activity_log::ActivityPriceParams;
use sikemux_core::server::activity_log::ActivityShare;
use sikemux_core::server::activity_log::ActivitySummary;
use sikemux_core::server::activity_log::ActivitySummaryParams;
use sikemux_core::server::activity_log::PriceOverride;
use sikemux_core::server::call;

use crate::activity_view::{self, Metric};
use crate::core::{utc_offset_minutes, Target};
use crate::icons::Icon;
use crate::reads::Read;
use crate::settings::index::Page::Usage;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::settings::Settings;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS,
    TEXT_2XS_H, TEXT_SM,
};
use crate::ui;
use crate::ui::numbers::{compact, money, whole};
use crate::ui::time::total;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(Usage, "Overview", "activity stats statistics analytics dashboard profile usage totals sessions tokens commits hours cost spend dollars money"),
    section(Usage, "Calendar", "heatmap contributions streak days year"),
    section(Usage, "By model", "cost price pricing spend opus sonnet gpt models"),
    section(Usage, "By agent", "claude codex share breakdown"),
    section(Usage, "By project", "repositories share breakdown"),
];

/// The price being entered for a model, in USD per million tokens.
pub struct Fields {
    model: Option<String>,
    prices: Vec<Entity<InputState>>,
}

/// Each price the form asks for, and whether it may be left empty.
const PRICES: [(&str, &str, bool); 4] = [
    ("input", n!("Input"), false),
    ("output", n!("Output"), false),
    ("cacheRead", n!("Cache read"), true),
    ("cacheWrite", n!("Cache write"), true),
];

pub(super) fn make(bench: &mut Workbench, _: &mut Window, cx: &mut Context<Workbench>) {
    Target::here(&bench.asks).watch::<call::ActivitySummary>(
        Read::ActivityEverywhere,
        ActivitySummaryParams {
            utc_offset_minutes: i64::from(utc_offset_minutes()),
            project: None,
        },
    );
    let fields = Fields {
        model: None,
        prices: Vec::new(),
    };
    super::change_pages(bench, cx, |pages, _| pages.activity = Some(fields));
}

/// Changes the price form, and draws the pages again.
fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.activity.as_mut(), change)
}

/// Opens the price form for `model`, filled with the price the person gave it.
fn edit(
    bench: &mut Workbench,
    model: String,
    price: Option<PriceOverride>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let prices = PRICES
        .iter()
        .enumerate()
        .map(|(at, (_, _, optional))| {
            let value = price
                .and_then(|price| {
                    [
                        Some(price.input),
                        Some(price.output),
                        price.cache_read,
                        price.cache_write,
                    ][at]
                })
                .map(|value| value.to_string())
                .unwrap_or_default();
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(if *optional {
                    t!("same as input")
                } else {
                    "0.00"
                });
                input.set_value(value, window, cx);
                input
            })
        })
        .collect();
    change_fields(bench, cx, |fields, _| {
        fields.model = Some(model);
        fields.prices = prices;
    });
}

fn close_form(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| {
        fields.model = None;
        fields.prices.clear();
    });
}

fn set_price(bench: &Workbench, model: &str, price: Option<PriceOverride>) {
    Target::here(&bench.asks).call_typed::<call::SetActivityPrice>(ActivityPriceParams {
        model: model.to_owned(),
        price,
    });
}

/// The prices typed, or none while one is missing or not a price.
fn typed_price(prices: &[Entity<InputState>], cx: &App) -> Option<PriceOverride> {
    let mut price = Map::new();
    for ((key, _, optional), input) in PRICES.iter().zip(prices) {
        let text = input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            match optional {
                true => continue,
                false => return None,
            }
        }
        let value: f64 = text
            .parse()
            .ok()
            .filter(|value: &f64| value.is_finite() && *value >= 0.)?;
        price.insert((*key).to_owned(), json!(value));
    }
    serde_json::from_value(Value::Object(price)).ok()
}

/// `claude-opus-5-5` reads as Opus 5.5 and `gpt-5.6-sol` as GPT-5.6 Sol; anything else keeps its id.
fn model_name(model: &str) -> String {
    let bare = model.rsplit('/').next().unwrap_or(model);
    let bare = bare.split('[').next().unwrap_or(bare);
    let capital = |word: &str| {
        let mut letters = word.chars();
        letters
            .next()
            .map(|first| first.to_uppercase().chain(letters).collect())
            .unwrap_or_default()
    };
    if let Some(rest) = bare.strip_prefix("claude-") {
        let mut parts: Vec<&str> = rest.split('-').collect();
        if parts
            .last()
            .is_some_and(|last| last.len() == 8 && last.chars().all(|c| c.is_ascii_digit()))
        {
            parts.pop();
        }
        let digits = |part: &&str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
        match parts.as_slice() {
            [family, major] if family.chars().all(|c| c.is_ascii_lowercase()) && digits(major) => {
                return format!("{} {major}", capital(family));
            }
            [family, major, minor]
                if family.chars().all(|c| c.is_ascii_lowercase())
                    && digits(major)
                    && digits(minor)
                    && minor.len() <= 2 =>
            {
                return format!("{} {major}.{minor}", capital(family));
            }
            _ => {}
        }
    }
    if let Some(rest) = bare.strip_prefix("gpt-") {
        let (version, variant) = rest.split_once('-').unwrap_or((rest, ""));
        if !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.') {
            let variant: Vec<String> = variant
                .split('-')
                .filter(|word| !word.is_empty())
                .map(capital)
                .collect();
            return match variant.is_empty() {
                true => format!("GPT-{version}"),
                false => format!("GPT-{version} {}", variant.join(" ")),
            };
        }
    }
    bare.to_owned()
}

fn percent(part: f64, whole: f64) -> i64 {
    match whole > 0. {
        true => (part / whole * 100.).round() as i64,
        false => 0,
    }
}

/// A share's value under the calendar's measure.
fn share_value(share: &ActivityShare, measure: Metric) -> String {
    let value = measure.of_share(share);
    if value <= 0. {
        return "—".into();
    }
    match measure {
        Metric::Cost => money(value),
        Metric::Tokens => compact(value),
        Metric::AgentTime => total(value),
        Metric::Commits | Metric::Sessions => whole(value),
    }
}

/// A line in a list of shares: its mark, its name with a quieter line under
/// it, what else it did, and its value.
fn share_row(
    mark: AnyElement,
    title: String,
    under: Option<String>,
    details: String,
    value: AnyElement,
    t: &Tokens,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .px(SPACE_4)
        .py(px(9.))
        .border_b_1()
        .border_color(t.line_soft)
        .child(ui::mark_box(20., mark))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child(title),
                )
                .children(under.map(|under| {
                    div()
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(under)
                })),
        )
        .child(
            div()
                .flex_none()
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(details),
        )
        .child(
            div()
                .flex_none()
                .min_w(px(64.))
                .flex()
                .justify_end()
                .font_features(theme::tabular())
                .text_size(crate::theme::ui_text(TEXT_SM))
                .text_color(t.text_primary)
                .child(value),
        )
}

fn shares(
    bench: &Workbench,
    shares: &[ActivityShare],
    agents: bool,
    measure: Metric,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    if shares.is_empty() {
        return rows::note(t!("Nothing recorded yet."), cx);
    }
    div()
        .children(shares.iter().map(|share| {
            let name = share.name.as_str();
            let details: Vec<String> = [
                Metric::Sessions,
                Metric::Tokens,
                Metric::Commits,
                Metric::AgentTime,
            ]
            .into_iter()
            .map(|other| (other, other.of_share(share)))
            .filter(|(other, value)| *other != measure && *value > 0.)
            .map(|(other, value)| match other {
                Metric::Sessions if value == 1. => t!("1 session").to_owned(),
                Metric::Sessions => t!("{count} sessions", count = whole(value)),
                Metric::Tokens => t!("{count} tokens", count = compact(value)),
                Metric::Commits if value == 1. => t!("1 commit").to_owned(),
                Metric::Commits => t!("{count} commits", count = whole(value)),
                _ => total(value),
            })
            .collect();
            let (mark, title, under) = match agents {
                true => (ui::mark(name, 20.), bench.agent_name(name).to_owned(), None),
                false => (
                    ui::icon(Icon::Folder, 16.)
                        .text_color(t.text_tertiary)
                        .into_any_element(),
                    std::path::Path::new(name)
                        .file_name()
                        .map(|base| base.to_string_lossy().into_owned())
                        .unwrap_or_else(|| name.to_owned()),
                    Some(crate::ui::path::tilde(
                        name,
                        crate::ui::path::this_home().as_deref(),
                    )),
                ),
            };
            share_row(
                mark,
                title,
                under,
                details.join(" · "),
                div().child(share_value(share, measure)).into_any_element(),
                &t,
            )
        }))
        .into_any_element()
}

fn price_form(
    model: &str,
    fields: &Fields,
    own: bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let valid = typed_price(&fields.prices, cx).is_some();
    let (save, revert, cancel) = (this.clone(), this.clone(), this.clone());
    let (model_save, model_revert) = (model.to_owned(), model.to_owned());
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .px(SPACE_4)
        .py(SPACE_3)
        .border_b_1()
        .border_color(t.line_soft)
        .bg(t.surface_sunken)
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(t!(
                    "USD per million tokens. Cache prices default to the input price."
                )),
        )
        .child(
            div()
                .flex()
                .gap(SPACE_2)
                .children(
                    PRICES
                        .iter()
                        .zip(&fields.prices)
                        .map(|((_, label, _), input)| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(SPACE_1)
                                .child(
                                    div()
                                        .text_size(crate::theme::ui_text(TEXT_2XS))
                                        .text_color(t.text_tertiary)
                                        .child(crate::i18n::tr(label)),
                                )
                                .child(ui::Field::new(input))
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(SPACE_2)
                .when(own, |actions| {
                    actions.child(
                        ui::Button::new(
                            "price-published",
                            t!("Use published price"),
                            ui::Variant::Outline,
                        )
                        .on_click(move |_, _, cx| {
                            revert.update(cx, |bench, cx| {
                                set_price(bench, &model_revert, None);
                                close_form(bench, cx);
                            })
                        }),
                    )
                })
                .child(
                    ui::Button::new("price-cancel", t!("Cancel"), ui::Variant::Outline)
                        .on_click(move |_, _, cx| cancel.update(cx, close_form)),
                )
                .child(
                    ui::Button::new("price-save", t!("Save"), ui::Variant::Primary)
                        .disabled(!valid)
                        .on_click(move |_, _, cx| {
                            save.update(cx, |bench, cx| {
                                let price = super::state(bench, cx)
                                    .and_then(|pages| pages.activity.as_ref())
                                    .and_then(|fields| typed_price(&fields.prices, cx));
                                if let Some(price) = price {
                                    set_price(bench, &model_save, Some(price));
                                    close_form(bench, cx);
                                }
                            })
                        }),
                ),
        )
        .into_any_element()
}

fn models(
    summary: &ActivitySummary,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let models = &summary.models;
    if models.is_empty() {
        return rows::note(t!("Nothing recorded yet."), cx);
    }
    let prices = &summary.pricing.overrides;
    div()
        .children(models.iter().map(|model| {
            let id = model.model.clone();
            let name = model_name(&id);
            let own = prices.get(&id).copied();
            let cost = model.cost_usd;
            let details: Vec<String> = [
                Some(t!(
                    "{count} tokens",
                    count = compact((model.input + model.output + model.cache_write) as f64)
                )),
                (model.cache_write > 0)
                    .then(|| t!("{count} written", count = compact(model.cache_write as f64))),
                (model.fast_cost_usd > 0.).then(|| {
                    t!(
                        "{percent}% fast",
                        percent = percent(model.fast_cost_usd, cost)
                    )
                }),
            ]
            .into_iter()
            .flatten()
            .collect();
            let opener = |label: &'static str, tag: &'static str| {
                let (this, id) = (this.clone(), id.clone());
                div()
                    .id(SharedString::from(format!("{tag}-{id}")))
                    .cursor_pointer()
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                    .text_color(t.text_secondary)
                    .hover(|style| style.text_color(t.text_primary))
                    .child(label)
                    .on_click(move |_, window, cx| {
                        let id = id.clone();
                        this.update(cx, |bench, cx| edit(bench, id, own, window, cx))
                    })
            };
            let value = match model.priced {
                true => div().child(money(cost)).into_any_element(),
                false => opener(t!("Set price"), "price-set").into_any_element(),
            };
            let row = share_row(
                ui::mark(&model.agent, 20.),
                name.clone(),
                (name != id).then(|| id.clone()),
                details.join(" · "),
                value,
                &t,
            );
            let row = match own.is_some() {
                true => row.child(opener(t!("your price"), "price-own")),
                false => row,
            };
            let split = (model.priced && cost > 0.).then(|| cost_split(model, &t));
            div()
                .border_b_1()
                .border_color(t.line_soft)
                .child(row.border_b_0())
                .children(split)
                .when(fields.model.as_deref() == Some(id.as_str()), |model| {
                    model.child(price_form(&id, fields, own.is_some(), this, cx))
                })
        }))
        .into_any_element()
}

/// What a model's cost went on, by kind of token: cache writes first, as where a cold resume
/// or a rotation spends.
fn cost_split(model: &ActivityModel, t: &Tokens) -> AnyElement {
    let costs = &model.costs;
    let parts = [
        (t!("Cache write"), costs.cache_write, t.warn),
        (t!("Output"), costs.output, theme::brand(&model.agent)),
        (t!("Cache read"), costs.cache_read, t.accent),
        (t!("Input"), costs.input, t.text_tertiary),
    ];
    let total = costs.total().max(f64::EPSILON);
    div()
        .pl(SPACE_4 + px(20.) + SPACE_3)
        .pr(SPACE_4)
        .pb(SPACE_2H)
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(
            div()
                .flex()
                .gap(px(1.))
                .h(px(6.))
                .rounded(px(3.))
                .overflow_hidden()
                .children(parts.iter().filter(|part| part.1 > 0.).map(|part| {
                    div()
                        .h_full()
                        .flex_basis(relative((part.1 / total) as f32))
                        .bg(part.2)
                })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(SPACE_3)
                .gap_y(SPACE_1)
                .children(parts.iter().filter(|part| part.1 > 0.).map(|part| {
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
                        .text_color(t.text_tertiary)
                        .child(div().size(px(7.)).rounded(px(2.)).bg(part.2))
                        .child(part.0)
                        .child(
                            div()
                                .font_features(theme::tabular())
                                .text_color(t.text_primary)
                                .child(money(part.1)),
                        )
                })),
        )
        .into_any_element()
}

fn pricing_note(summary: &ActivitySummary, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let unpriced = summary.totals.unpriced_tokens as f64;
    let fetched = (summary.pricing.fetched_at_ms)
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single());
    let source = match fetched {
        Some(at) => t!("Prices from models.dev, {date}", date = at.format("%b %-d")),
        None => t!("Published prices could not be loaded").to_owned(),
    };
    let line = match unpriced > 0. {
        true => {
            let unpriced = t!("{count} tokens have no price", count = compact(unpriced));
            format!("{unpriced} · {source}")
        }
        false => source,
    };
    div()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .px(SPACE_4)
        .py(SPACE_2H)
        .text_size(crate::theme::ui_text(TEXT_2XS_H))
        .text_color(t.text_tertiary)
        .child(t!(
            "What these tokens would cost at API prices. Subscription plans bill separately."
        ))
        .child(line)
        .into_any_element()
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.activity else {
        return Vec::new();
    };
    let read = bench.reads.activity_everywhere.as_ref();
    let failed = read.is_some_and(Option::is_none);
    let summary = read.and_then(Option::as_ref);
    let measure = bench.calendar.metric;
    let totals = summary.map(|summary| &summary.totals);
    let since = totals
        .and_then(|totals| totals.first_at_ms)
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .map(|at| SharedString::from(t!("since {date}", date = at.format("%b %-d, %Y"))));
    let overview = match totals {
        _ if failed => rows::note(t!("Activity could not be read."), cx),
        Some(totals) if activity_view::is_empty(totals) => rows::note(
            t!("Nothing yet. Start an agent and its sessions, turns, tokens and commits collect here."),
            cx,
        ),
        Some(totals) => div()
            .p(SPACE_4)
            .child(activity_view::overview(totals, cx))
            .into_any_element(),
        None => rows::note(t!("Reading activity…"), cx),
    };
    let calendar = match summary {
        Some(summary) => div()
            .p(SPACE_4)
            .child(activity_view::calendar(summary, bench, this, None, cx))
            .into_any_element(),
        None => div().into_any_element(),
    };
    let by_model = summary
        .map(|summary| models(summary, fields, this, cx))
        .unwrap_or_else(|| div().into_any_element());
    let by_agent = summary
        .map(|summary| shares(bench, &summary.agents, true, measure, cx))
        .unwrap_or_else(|| div().into_any_element());
    let by_project = div()
        .children(summary.map(|summary| {
            let shown = &summary.projects[..summary.projects.len().min(8)];
            shares(bench, shown, false, measure, cx)
        }))
        .children(summary.map(|summary| pricing_note(summary, cx)));
    vec![
        rows::section(
            n!("Overview"),
            since,
            Some(t!("Counted from agents run and commits made inside Sikemux. Work done elsewhere is never read.").into()),
            lit("Overview"),
            overview,
            cx,
        ),
        rows::section(n!("Calendar"), None, None, lit("Calendar"), calendar, cx),
        rows::section(n!("By model"), None, None, lit("By model"), by_model, cx),
        rows::section(n!("By agent"), None, None, lit("By agent"), by_agent, cx),
        rows::section(n!("By project"), None, None, lit("By project"), by_project, cx),
    ]
}
