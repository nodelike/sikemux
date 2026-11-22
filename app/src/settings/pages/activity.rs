//! What agents did across every project: the totals, the calendar, and the
//! share each model, agent and project had, priced at API rates.

use std::sync::Arc;

use crate::ui::Clicks;
use chrono::TimeZone;
use gpui_kit::base::Popover;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, Anchor, AnyElement, App, AppContext, ClipboardItem, Context, Entity, Image,
    ImageFormat, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, WeakEntity,
    Window,
};
use serde_json::{json, Map, Value};
use sikemux_calls::activity_log::ActivityModel;
use sikemux_calls::activity_log::ActivityPriceParams;
use sikemux_calls::activity_log::ActivityShare;
use sikemux_calls::activity_log::ActivitySummary;
use sikemux_calls::activity_log::ActivitySummaryParams;
use sikemux_calls::activity_log::PriceOverride;
use sikemux_calls::call;

use super::week_card;
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
use crate::toast;
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

/// How wide the float shows the card.
const PREVIEW_WIDTH: f32 = 316.;

/// The price being entered for a model, in USD per million tokens, and the week's card.
pub struct Fields {
    model: Option<String>,
    prices: Vec<Entity<InputState>>,
    week: Week,
}

/// The card of the last seven days, from the moment its float opens.
#[derive(Clone)]
enum Week {
    Reading,
    Empty,
    Failed,
    Drawn {
        saved: Arc<Image>,
        preview: Arc<Image>,
    },
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
            since_ms: None,
        },
    );
    let fields = Fields {
        model: None,
        prices: Vec::new(),
        week: Week::Reading,
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
                    ]
                    .get(at)
                    .copied()
                    .flatten()
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
        .border_b(crate::theme::border())
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
                        .font_family(theme::mono_font())
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
        .border_b(crate::theme::border())
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
            let name = crate::ui::model_name(&id);
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
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                    .text_color(t.text_secondary)
                    .hover(|style| style.text_color(t.text_primary))
                    .child(label)
                    .clicks(move |_, window, cx| {
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
                .border_b(crate::theme::border())
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

/// Asks the core for the last seven days, which the card is drawn from once they arrive.
fn read_week(bench: &Workbench, cx: &mut App) {
    let (since, _) = week_card::window();
    Target::here(&bench.asks).answer::<call::ActivitySummary>(
        Read::ActivityWeek,
        ActivitySummaryParams {
            utc_offset_minutes: i64::from(utc_offset_minutes()),
            project: None,
            since_ms: Some(since),
        },
    );
    change_fields(bench, cx, |fields, cx| {
        fields.week = Week::Reading;
        cx.notify();
    });
}

/// Draws the card from the week the core answered, away from the main thread.
pub fn week_read(
    bench: &mut Workbench,
    summary: Option<ActivitySummary>,
    cx: &mut Context<Workbench>,
) {
    let (_, range) = week_card::window();
    let card = match summary {
        None => Err(Week::Failed),
        Some(summary) => week_card::Card::of(&summary, &range).ok_or(Week::Empty),
    };
    let card = match card {
        Ok(card) => card,
        Err(week) => {
            change_fields(bench, cx, |fields, cx| {
                fields.week = week;
                cx.notify();
            });
            return;
        }
    };
    let drawn = cx.background_spawn(async move {
        let saved = week_card::draw(&card, week_card::SAVED_SCALE)?;
        let preview = week_card::draw(&card, week_card::PREVIEW_SCALE)?;
        Ok::<_, String>((saved, preview))
    });
    cx.spawn(async move |bench, cx| {
        let week = match drawn.await {
            Ok((saved, preview)) => Week::Drawn {
                saved: Arc::new(Image::from_bytes(ImageFormat::Png, saved)),
                preview: Arc::new(Image::from_bytes(ImageFormat::Png, preview)),
            },
            Err(error) => {
                eprintln!("sikemux: the week's card could not be drawn: {error}");
                Week::Failed
            }
        };
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, cx| {
                fields.week = week;
                cx.notify();
            })
        });
    })
    .detach();
}

/// "Share week" on the Overview's heading, opening the card with ways to keep it.
fn share_week(fields: &Fields, this: &Entity<Workbench>) -> AnyElement {
    let week = fields.week.clone();
    let opened = this.downgrade();
    let content_bench = this.downgrade();
    Popover::new("usage-share-week")
        .anchor(Anchor::TopRight)
        .offset(SPACE_1H)
        .trigger_with(|_, _, _| {
            ui::Button::new("usage-share-week", t!("Share week"), ui::Variant::Outline)
                .icon(Icon::Share)
                .into_any_element()
        })
        .on_open_change(move |open, _, cx| {
            if *open {
                let _ = opened.update(cx, |bench, cx| read_week(bench, cx));
            }
        })
        .content(move |_, _, cx| week_float(&week, content_bench, cx))
        .into_any_element()
}

fn week_float(week: &Week, bench: WeakEntity<Workbench>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let frame = div()
        .w(px(PREVIEW_WIDTH))
        .h(px(PREVIEW_WIDTH * 9. / 16.))
        .rounded(px(6.))
        .border(theme::border())
        .border_color(t.border)
        .bg(gpui_kit::black())
        .overflow_hidden();
    let words = |text: &str| {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(crate::theme::ui_text(TEXT_SM))
            .text_color(t.text_tertiary)
            .child(SharedString::from(text.to_owned()))
    };
    let (preview, kept) = match week {
        Week::Drawn { saved, preview } => (
            frame.child(gpui_kit::img(preview.clone()).size_full()),
            Some(saved.clone()),
        ),
        Week::Reading => (frame.child(words(t!("Reading the week…"))), None),
        Week::Empty => (frame.child(words(t!("Nothing recorded this week."))), None),
        Week::Failed => (frame.child(words(t!("The week could not be read."))), None),
    };
    let (_, range) = week_card::window();
    let save_bench = bench.clone();
    let save_card = kept.clone();
    let copy_card = kept.clone();
    ui::float(cx)
        .p(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .child(preview)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex_1()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_secondary)
                        .child(range),
                )
                .child(
                    ui::Button::new("usage-week-save", t!("Save PNG"), ui::Variant::Outline)
                        .icon(Icon::Download)
                        .disabled(kept.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(card) = &save_card {
                                save_week(save_bench.clone(), card.clone(), cx);
                            }
                        }),
                )
                .child(
                    ui::Button::new("usage-week-copy", t!("Copy image"), ui::Variant::Primary)
                        .icon(Icon::Copy)
                        .disabled(kept.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(card) = &copy_card {
                                cx.write_to_clipboard(ClipboardItem::new_image(card));
                                toast::show(
                                    &bench,
                                    toast::success(t!("Copied the week's picture")),
                                    cx,
                                );
                            }
                        }),
                ),
        )
        .into_any_element()
}

/// Asks where to keep the card, then writes it there.
fn save_week(bench: WeakEntity<Workbench>, card: Arc<Image>, cx: &mut App) {
    let (_, range) = week_card::window();
    let name = format!(
        "sikemux-week-{}.png",
        range.replace(' ', "-").to_lowercase()
    );
    let folder = sikemux_process::paths::home()
        .map(|home| home.join("Downloads"))
        .unwrap_or_default();
    let picked = cx.prompt_for_new_path(&folder, Some(&name));
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(path))) = picked.await else {
            return;
        };
        let note = match std::fs::write(&path, card.bytes()) {
            Ok(()) => toast::success(t!(
                "Saved the week's picture as {name}",
                name = path.file_name().unwrap_or_default().to_string_lossy()
            )),
            Err(error) => {
                toast::error(t!("The picture could not be saved: {error}", error = error))
            }
        };
        cx.update(|cx| toast::show(&bench, note, cx));
    })
    .detach();
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
            .child(activity_view::calendar(summary, bench, this, cx))
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
            let shown = summary.projects.get(..8).unwrap_or(&summary.projects);
            shares(bench, shown, false, measure, cx)
        }))
        .children(summary.map(|summary| pricing_note(summary, cx)));
    vec![
        rows::section_with(
            n!("Overview"),
            since,
            Some(share_week(fields, this)),
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
