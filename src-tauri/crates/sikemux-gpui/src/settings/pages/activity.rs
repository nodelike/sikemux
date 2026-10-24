//! What agents did across every project: the totals, the calendar, and the
//! share each model, agent and project had, priced at API rates.

use chrono::TimeZone;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use serde_json::{json, Map, Value};

use crate::core::{utc_offset_minutes, Ask, Watch};
use crate::home::{self, compact, duration, money, number, plural, whole};
use crate::icons::Icon;
use crate::settings::{rows, ACTIVITY};
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

pub const SECTIONS: &[&str] = &["Overview", "Calendar", "By model", "By agent", "By project"];

/// The price being entered for a model, in USD per million tokens.
pub struct Fields {
    model: Option<String>,
    prices: Vec<Entity<InputState>>,
}

/// Each price the form asks for, and whether it may be left empty.
const PRICES: [(&str, &str, bool); 4] = [
    ("input", "Input", false),
    ("output", "Output", false),
    ("cacheRead", "Cache read", true),
    ("cacheWrite", "Cache write", true),
];

fn make(bench: &mut Workbench, _: &mut Window, cx: &mut Context<Workbench>) {
    let _ = bench.asks.send(Ask::Watch(
        ACTIVITY,
        Watch::new(
            "activitySummary",
            json!({ "utcOffsetMinutes": utc_offset_minutes(), "project": null }),
            &["activity"],
        ),
    ));
    if let Some(settings) = bench.settings.as_mut() {
        settings.pages.activity = Some(Fields {
            model: None,
            prices: Vec::new(),
        });
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

fn fields_of(bench: &mut Workbench) -> Option<&mut Fields> {
    bench.settings.as_mut()?.pages.activity.as_mut()
}

/// Opens the price form for `model`, filled with the price the person gave it.
fn edit(
    bench: &mut Workbench,
    model: String,
    price: Option<Value>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let prices = PRICES
        .iter()
        .map(|(key, _, optional)| {
            let value = price
                .as_ref()
                .and_then(|price| price[key].as_f64())
                .map(|value| value.to_string())
                .unwrap_or_default();
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(if *optional {
                    "same as input"
                } else {
                    "0.00"
                });
                input.set_value(value, window, cx);
                input
            })
        })
        .collect();
    if let Some(fields) = fields_of(bench) {
        fields.model = Some(model);
        fields.prices = prices;
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

fn close_form(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if let Some(fields) = fields_of(bench) {
        fields.model = None;
        fields.prices.clear();
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

fn set_price(bench: &Workbench, model: &str, price: Value) {
    let _ = bench.asks.send(Ask::Call {
        method: "setActivityPrice",
        params: json!({ "model": model, "price": price }),
    });
}

/// The prices typed, or none while one is missing or not a price.
fn typed_price(prices: &[Entity<InputState>], cx: &App) -> Option<Value> {
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
    Some(Value::Object(price))
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
fn share_value(share: &Value, key: &str) -> String {
    let value = number(&share[key]);
    if value <= 0. {
        return "—".into();
    }
    match key {
        "costUsd" => money(value),
        "tokens" => compact(value),
        "agentMs" => duration(value),
        _ => whole(value),
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
        .gap(px(12.))
        .px(px(14.))
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
                        .text_size(crate::theme::ui_text(13.))
                        .text_color(t.text_primary)
                        .child(title),
                )
                .children(under.map(|under| {
                    div()
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(11.))
                        .text_color(t.text_tertiary)
                        .child(under)
                })),
        )
        .child(
            div()
                .flex_none()
                .text_size(crate::theme::ui_text(11.5))
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
                .text_size(crate::theme::ui_text(13.))
                .text_color(t.text_primary)
                .child(value),
        )
}

fn agent_label(bench: &Workbench, agent: &str) -> String {
    bench
        .read("catalog")
        .and_then(Value::as_array)
        .and_then(|kinds| kinds.iter().find(|kind| kind["type"] == agent))
        .and_then(|kind| kind["label"].as_str())
        .unwrap_or(agent)
        .to_owned()
}

fn pretty(path: &str) -> String {
    let home = sikemux_process::paths::home()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

fn shares(bench: &Workbench, shares: &[Value], agents: bool, key: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    if shares.is_empty() {
        return rows::note("Nothing recorded yet.", cx);
    }
    div()
        .children(shares.iter().map(|share| {
            let name = share["name"].as_str().unwrap_or_default();
            let details: Vec<String> = [
                ("sessions", number(&share["sessions"])),
                ("tokens", number(&share["tokens"])),
                ("commits", number(&share["commits"])),
                ("agentMs", number(&share["agentMs"])),
            ]
            .into_iter()
            .filter(|(measure, value)| *measure != key && *value > 0.)
            .map(|(measure, value)| match measure {
                "sessions" => plural(value, "session"),
                "tokens" => format!("{} tokens", compact(value)),
                "commits" => plural(value, "commit"),
                _ => duration(value),
            })
            .collect();
            let (mark, title, under) = match agents {
                true => (ui::mark(name, 20.), agent_label(bench, name), None),
                false => (
                    ui::icon(Icon::Folder, 16.)
                        .text_color(t.text_tertiary)
                        .into_any_element(),
                    std::path::Path::new(name)
                        .file_name()
                        .map(|base| base.to_string_lossy().into_owned())
                        .unwrap_or_else(|| name.to_owned()),
                    Some(pretty(name)),
                ),
            };
            share_row(
                mark,
                title,
                under,
                details.join(" · "),
                div().child(share_value(share, key)).into_any_element(),
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
        .gap(px(10.))
        .px(px(14.))
        .py(px(12.))
        .border_b_1()
        .border_color(t.line_soft)
        .bg(t.surface_sunken)
        .child(
            div()
                .text_size(crate::theme::ui_text(11.5))
                .text_color(t.text_tertiary)
                .child("USD per million tokens. Cache prices default to the input price."),
        )
        .child(
            div()
                .flex()
                .gap(px(8.))
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
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(crate::theme::ui_text(11.))
                                        .text_color(t.text_tertiary)
                                        .child(*label),
                                )
                                .child(ui::Field::new(input))
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .when(own, |actions| {
                    actions.child(
                        ui::Button::new(
                            "price-published",
                            "Use published price",
                            ui::Variant::Outline,
                        )
                        .on_click(move |_, _, cx| {
                            revert.update(cx, |bench, cx| {
                                set_price(bench, &model_revert, Value::Null);
                                close_form(bench, cx);
                            })
                        }),
                    )
                })
                .child(
                    ui::Button::new("price-cancel", "Cancel", ui::Variant::Outline)
                        .on_click(move |_, _, cx| cancel.update(cx, close_form)),
                )
                .child(
                    ui::Button::new("price-save", "Save", ui::Variant::Primary)
                        .disabled(!valid)
                        .on_click(move |_, _, cx| {
                            save.update(cx, |bench, cx| {
                                let price = bench
                                    .settings
                                    .as_ref()
                                    .and_then(|settings| settings.pages.activity.as_ref())
                                    .and_then(|fields| typed_price(&fields.prices, cx));
                                if let Some(price) = price {
                                    set_price(bench, &model_save, price);
                                    close_form(bench, cx);
                                }
                            })
                        }),
                ),
        )
        .into_any_element()
}

fn models(summary: &Value, fields: &Fields, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(models) = summary["models"].as_array() else {
        return div().into_any_element();
    };
    if models.is_empty() {
        return rows::note("Nothing recorded yet.", cx);
    }
    let prices = &summary["pricing"]["overrides"];
    div()
        .children(models.iter().map(|model| {
            let id = model["model"].as_str().unwrap_or_default().to_owned();
            let name = model_name(&id);
            let own = prices.get(&id).filter(|price| !price.is_null()).cloned();
            let input = number(&model["input"])
                + number(&model["cacheRead"])
                + number(&model["cacheWrite"]);
            let cost = number(&model["costUsd"]);
            let details: Vec<String> = [
                Some(format!(
                    "{} tokens",
                    compact(
                        number(&model["input"])
                            + number(&model["output"])
                            + number(&model["cacheWrite"])
                    )
                )),
                (number(&model["cacheRead"]) > 0.)
                    .then(|| format!("{}% cached", percent(number(&model["cacheRead"]), input))),
                (number(&model["fastCostUsd"]) > 0.)
                    .then(|| format!("{}% fast", percent(number(&model["fastCostUsd"]), cost))),
            ]
            .into_iter()
            .flatten()
            .collect();
            let opener = |label: &'static str, tag: &'static str| {
                let (this, id, own) = (this.clone(), id.clone(), own.clone());
                div()
                    .id(SharedString::from(format!("{tag}-{id}")))
                    .cursor_pointer()
                    .text_size(crate::theme::ui_text(11.5))
                    .text_color(t.text_secondary)
                    .hover(|style| style.text_color(t.text_primary))
                    .child(label)
                    .on_click(move |_, window, cx| {
                        let (id, own) = (id.clone(), own.clone());
                        this.update(cx, |bench, cx| edit(bench, id, own, window, cx))
                    })
            };
            let value = match model["priced"].as_bool() == Some(true) {
                true => div().child(money(cost)).into_any_element(),
                false => opener("Set price", "price-set").into_any_element(),
            };
            let row = share_row(
                ui::mark(model["agent"].as_str().unwrap_or_default(), 20.),
                name.clone(),
                (name != id).then(|| id.clone()),
                details.join(" · "),
                value,
                &t,
            );
            let row = match own.is_some() {
                true => row.child(opener("your price", "price-own")),
                false => row,
            };
            div()
                .child(row)
                .when(fields.model.as_deref() == Some(id.as_str()), |model| {
                    model.child(price_form(&id, fields, own.is_some(), this, cx))
                })
        }))
        .into_any_element()
}

fn pricing_note(summary: &Value, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let unpriced = number(&summary["totals"]["unpricedTokens"]);
    let fetched = summary["pricing"]["fetchedAtMs"]
        .as_i64()
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single());
    let source = match fetched {
        Some(at) => format!("Prices from models.dev, {}", at.format("%b %-d")),
        None => "Published prices could not be loaded".into(),
    };
    let unpriced = match unpriced > 0. {
        true => format!("{} tokens have no price · ", compact(unpriced)),
        false => String::new(),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(14.))
        .py(px(10.))
        .text_size(crate::theme::ui_text(11.5))
        .text_color(t.text_tertiary)
        .child("What these tokens would cost at API prices. Subscription plans bill separately.")
        .child(format!("{unpriced}{source}"))
        .into_any_element()
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.activity else {
        cx.defer_in(window, make);
        return Vec::new();
    };
    let read = bench.read(ACTIVITY);
    let failed = read.is_some_and(Value::is_null);
    let summary = read.filter(|summary| !summary.is_null());
    let key = home::METRICS[bench.metric].0;
    let totals = summary.map(|summary| &summary["totals"]);
    let since = totals
        .and_then(|totals| totals["firstAtMs"].as_i64())
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .map(|at| SharedString::from(format!("since {}", at.format("%b %-d, %Y"))));
    let overview = match totals {
        _ if failed => rows::note("Activity could not be read.", cx),
        Some(totals) if home::is_empty(totals) => rows::note(
            "Nothing yet. Start an agent and its sessions, turns, tokens and commits collect here.",
            cx,
        ),
        Some(totals) => div()
            .p(px(14.))
            .child(home::overview(totals, cx))
            .into_any_element(),
        None => rows::note("Reading activity…", cx),
    };
    let calendar = match summary {
        Some(summary) => div()
            .p(px(14.))
            .child(home::calendar(summary, bench, None, cx))
            .into_any_element(),
        None => div().into_any_element(),
    };
    let list = |name: &str, limit: usize| {
        summary
            .and_then(|summary| summary[name].as_array())
            .map(|all| all.iter().take(limit).cloned().collect::<Vec<_>>())
    };
    let by_model = summary
        .map(|summary| models(summary, fields, this, cx))
        .unwrap_or_else(|| div().into_any_element());
    let by_agent = list("agents", usize::MAX)
        .map(|agents| shares(bench, &agents, true, key, cx))
        .unwrap_or_else(|| div().into_any_element());
    let by_project = div()
        .children(list("projects", 8).map(|projects| shares(bench, &projects, false, key, cx)))
        .children(summary.map(|summary| pricing_note(summary, cx)));
    vec![
        rows::section(
            "Overview",
            since,
            Some("Counted from agents run and commits made inside Sikemux. Work done elsewhere is never read.".into()),
            lit("Overview"),
            overview,
            cx,
        ),
        rows::section("Calendar", None, None, lit("Calendar"), calendar, cx),
        rows::section("By model", None, None, lit("By model"), by_model, cx),
        rows::section("By agent", None, None, lit("By agent"), by_agent, cx),
        rows::section("By project", None, None, lit("By project"), by_project, cx),
    ]
}
