//! A console on a saved database: SQL to run, and what the last run gave, a statement at a
//! time, with each result's rows in a grid.

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, AppContext, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, WeakEntity, Window,
};
use serde_json::json;
use sikemux_wire::database::{Connection, CopyFormat, QueryOutcome, ResultSet};

use super::{Database, Host, PluginPane};
use crate::plugin_pane::{after, listen};
use crate::theme::{self, Tokens, RADIUS_2};
use crate::ui::{self, Button, Pick, Variant};

/// The most rows kept from each statement, as the person can pick it.
const ROW_LIMITS: [u32; 4] = [100, 500, 1000, 5000];
const ROW_LIMIT: u32 = 500;
const COLUMN_WIDTH: f32 = 160.;
const NUMBER_WIDTH: f32 = 44.;
const ROW_HEIGHT: f32 = 24.;

/// One console's SQL and what its last run gave.
pub(super) struct Console {
    sql: Entity<TextareaState>,
    limit: u32,
    /// Names the run going now, so Stop reaches it and not another console's.
    running: Option<String>,
    outcome: Option<Rc<QueryOutcome>>,
    error: Option<SharedString>,
    /// The result in front, when a run gave several.
    picked: usize,
    /// The cell picked in the grid, by row and column.
    selected: Option<(usize, usize)>,
}

impl Console {
    pub(super) fn new(key: String, window: &mut Window, cx: &mut Context<PluginPane>) -> Self {
        let sql = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(6, 16)
                .placeholder("select * from …")
        });
        cx.subscribe(&sql, move |pane, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = event
            {
                if let Some((database, host)) = pane.parts::<Database>() {
                    run(database, host, &key, cx);
                }
            }
        })
        .detach();
        Self {
            sql,
            limit: ROW_LIMIT,
            running: None,
            outcome: None,
            error: None,
            picked: 0,
            selected: None,
        }
    }

    /// Puts SQL in the editor without running it, as when a table is picked.
    pub(super) fn load(&self, sql: &str, window: &mut Window, cx: &mut App) {
        self.sql.update(cx, |editor, cx| {
            editor.set_value(sql.to_owned(), window, cx)
        });
    }
}

/// The connection a console's key names: `<connection>/<number>`.
fn connection_of(key: &str) -> &str {
    key.rsplit_once('/').map_or(key, |(id, _)| id)
}

/// Runs the console's SQL on its database. A run already going is left alone rather than
/// queued.
pub(super) fn run(
    database: &mut Database,
    host: &mut Host,
    key: &str,
    cx: &mut Context<PluginPane>,
) {
    let Some(console) = database.consoles.get_mut(key) else {
        return;
    };
    let sql = console.sql.read(cx).value().trim().to_owned();
    if sql.is_empty() || console.running.is_some() {
        return;
    }
    database.runs += 1;
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let run = format!("gpui-{started:x}-{}", database.runs);
    console.running = Some(run.clone());
    console.error = None;
    let call = host.call(
        "query",
        json!({ "id": connection_of(key), "sql": sql, "limit": console.limit, "run": run }),
    );
    let key = key.to_owned();
    after::<Database, _>(
        call,
        move |database, _, done, _| {
            let Some(console) = database.consoles.get_mut(&key) else {
                return;
            };
            console.running = None;
            console.selected = None;
            match done.map(serde_json::from_value::<QueryOutcome>) {
                Ok(Ok(outcome)) => {
                    console.picked = outcome.main;
                    console.outcome = Some(Rc::new(outcome));
                }
                Ok(Err(error)) => {
                    console.error = Some(format!("unreadable results: {error}").into())
                }
                Err(failure) => {
                    console.outcome = None;
                    console.error = Some(failure.message);
                }
            }
        },
        cx,
    );
    cx.notify();
}

fn stop(database: &Database, host: &Host, key: &str, cx: &mut Context<PluginPane>) {
    let Some(run) = database
        .consoles
        .get(key)
        .and_then(|console| console.running.clone())
    else {
        return;
    };
    let call = host.call("cancel", json!({ "id": connection_of(key), "run": run }));
    after::<Database, _>(
        call,
        |_, host, done, cx| {
            if let Err(failure) = done {
                host.said(&failure, cx);
            }
        },
        cx,
    );
}

pub(super) fn page(
    database: &Database,
    key: &str,
    connection: &Connection,
    host: &Host,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let Some(console) = database.consoles.get(key) else {
        return super::super::loading(cx);
    };
    let t = Tokens::get(cx).clone();
    let running = console.running.is_some();
    let written = !console.sql.read(cx).value().trim().is_empty();
    let (run_key, stop_key) = (key.to_owned(), key.to_owned());
    let pane = cx.weak_entity();
    let limit_key = key.to_owned();
    let limits = ui::select(
        SharedString::from(format!("db-limit-{key}")),
        format!("{} rows", console.limit),
        ROW_LIMITS
            .iter()
            .map(|limit| Pick::new(limit.to_string(), format!("{limit} rows")))
            .collect(),
        Some(console.limit.to_string().into()),
        running,
        move |picked, _, cx| {
            let Ok(limit) = picked.parse::<u32>() else {
                return;
            };
            let key = limit_key.clone();
            act(&pane, cx, move |database, _| {
                if let Some(console) = database.consoles.get_mut(&key) {
                    console.limit = limit;
                }
            });
        },
        cx,
    );
    let toolbar = div()
        .px(px(12.))
        .py(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            ui::text(connection.name.clone(), 12.5, FontWeight::SEMIBOLD)
                .text_color(t.text_primary),
        )
        .children(
            connection
                .connected
                .clone()
                .map(|version| ui::meta(version, cx)),
        )
        .child(div().flex_1())
        .children(connection.read_only.then(|| ui::caps("Read only", cx)))
        .child(limits)
        .children(running.then(|| {
            Button::new("db-stop", "Stop", Variant::Outline).on_click(listen::<Database, _>(
                cx,
                move |database, host, _, _, cx| stop(database, host, &stop_key, cx),
            ))
        }))
        .child(
            Button::new(
                "db-run",
                if running { "Running…" } else { "Run" },
                Variant::Primary,
            )
            .keys(Some("⌘↵".into()))
            .disabled(running || !written)
            .on_click(listen::<Database, _>(
                cx,
                move |database, host, _, _, cx| run(database, host, &run_key, cx),
            )),
        );
    let editor = div()
        .mx(px(12.))
        .px(px(10.))
        .py(px(8.))
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .font_family(theme::MONO)
        .child(
            Textarea::new(&console.sql)
                .appearance(false)
                .text_size(theme::ui_text(12.5)),
        );
    let below = match (&console.error, &console.outcome) {
        (Some(error), _) => div()
            .m(px(12.))
            .px(px(12.))
            .py(px(8.))
            .rounded(RADIUS_2)
            .bg(t.danger.opacity(0.12))
            .text_size(theme::ui_text(12.))
            .text_color(t.danger)
            .child(error.clone())
            .into_any_element(),
        (None, Some(outcome)) => results(key, console, outcome.clone(), host, cx),
        (None, None) => super::super::empty(
            if running {
                "Running…"
            } else {
                "Write SQL above and press ⌘↵ to run it."
            },
            cx,
        ),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(toolbar)
        .child(editor)
        .child(div().flex_1().min_h_0().flex().flex_col().child(below))
        .into_any_element()
}

fn results(
    key: &str,
    console: &Console,
    outcome: Rc<QueryOutcome>,
    host: &Host,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let at = console.picked.min(outcome.results.len().saturating_sub(1));
    let Some(result) = outcome.results.get(at) else {
        return super::super::empty("The statement ran and returned nothing.", cx);
    };
    let pane = cx.weak_entity();
    let statements = (outcome.results.len() > 1).then(|| {
        let key = key.to_owned();
        let pane = pane.clone();
        ui::Segmented::words(
            SharedString::from(format!("db-statements-{key}")),
            (1..=outcome.results.len())
                .map(|number| (SharedString::from(number.to_string()), None))
                .collect(),
            at,
        )
        .on_select(move |picked, _, cx| {
            let (key, picked) = (key.clone(), *picked);
            act(&pane, cx, move |database, _| {
                if let Some(console) = database.consoles.get_mut(&key) {
                    console.picked = picked;
                    console.selected = None;
                }
            });
        })
    });
    let copies = (!result.columns.is_empty()).then(|| {
        let copy = |id: &'static str, label: &'static str, format: CopyFormat, what: &'static str| {
            let shown = result.clone();
            Button::new(id, label, Variant::Outline).on_click(listen::<Database, _>(
                cx,
                move |_, host, _, _, cx| {
                    let call = host.call(
                        "copyResult",
                        json!({ "result": { "columns": shown.columns, "rows": shown.rows }, "format": format }),
                    );
                    after::<Database, _>(
                        call,
                        move |_, host, done, cx| match done {
                            Ok(text) => host.copy(text.as_str().unwrap_or_default().to_owned(), what, cx),
                            Err(failure) => host.said(&failure, cx),
                        },
                        cx,
                    );
                },
            ))
        };
        div()
            .flex()
            .gap(px(6.))
            .child(copy("db-copy", "Copy", CopyFormat::Tsv, "the results"))
            .child(copy("db-copy-csv", "Copy CSV", CopyFormat::Csv, "the results as CSV"))
    });
    let send = send_to_agent(key, result, host, cx);
    let bar = div()
        .px(px(12.))
        .py(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .children(statements)
        .child(ui::meta(
            format!("{} · {} ms", result.summary, outcome.millis),
            cx,
        ))
        .child(div().flex_1())
        .children(copies)
        .child(send);
    let body = match result.columns.is_empty() {
        true => super::super::empty(format!("{}.", result.summary), cx),
        false => grid(key, console, outcome.clone(), at, &t, cx),
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(t.border)
        .child(bar)
        .child(body)
        .children(inspector(console, result, cx))
        .into_any_element()
}

fn send_to_agent(
    key: &str,
    result: &ResultSet,
    host: &Host,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let agents = Rc::new(host.agents(cx));
    let picks = agents
        .iter()
        .enumerate()
        .map(|(at, agent)| Pick::new(at.to_string(), agent.label.clone()))
        .collect();
    let (pane, text) = (cx.entity(), result.for_agent.clone());
    ui::select(
        SharedString::from(format!("db-send-{key}")),
        "Send to agent",
        picks,
        None,
        agents.is_empty(),
        move |picked, window, cx| {
            let Some(agent) = picked.parse::<usize>().ok().and_then(|at| agents.get(at)) else {
                return;
            };
            let (to, text) = (agent.to.clone(), text.clone());
            pane.update(cx, |pane, cx| pane.host.send_to_agent(to, text, window, cx));
        },
        cx,
    )
}

/// The result's rows, drawn only where they show so a long one stays quick, with the row
/// numbers at the start and numbers to the right.
fn grid(
    key: &str,
    console: &Console,
    outcome: Rc<QueryOutcome>,
    at: usize,
    t: &Tokens,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let Some(result) = outcome.results.get(at) else {
        return div().into_any_element();
    };
    let width = NUMBER_WIDTH + COLUMN_WIDTH * result.columns.len() as f32;
    let header = div()
        .h(px(ROW_HEIGHT + 4.))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(t.border)
        .child(div().w(px(NUMBER_WIDTH)).flex_none())
        .children(result.columns.iter().map(|column| {
            div()
                .w(px(COLUMN_WIDTH))
                .flex_none()
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .when(column.numeric, |cell| cell.justify_end())
                .child(
                    ui::text(column.name.clone(), 11.5, FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .truncate(),
                )
                .children((!column.type_name.is_empty()).then(|| {
                    ui::mono(column.type_name.clone(), 10., cx).text_color(t.text_tertiary)
                }))
        }));
    let (ink, dim, chosen) = (t.text_secondary, t.text_tertiary, t.surface_chosen);
    let selected = console.selected;
    let pane = cx.weak_entity();
    let rows_key = key.to_owned();
    let rows = uniform_list(
        SharedString::from(format!("db-rows-{key}")),
        result.rows.len(),
        move |range, _, _| {
            let Some(result) = outcome.results.get(at) else {
                return Vec::new();
            };
            range
                .filter_map(|row| Some((row, result.rows.get(row)?)))
                .map(|(row, cells)| {
                    div()
                        .w(px(width))
                        .h(px(ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .font_family(theme::MONO)
                        .text_size(theme::ui_text(11.5))
                        .child(
                            div()
                                .w(px(NUMBER_WIDTH))
                                .flex_none()
                                .px(px(8.))
                                .text_color(dim)
                                .child((row + 1).to_string()),
                        )
                        .children(cells.iter().enumerate().map(|(column, cell)| {
                            let numeric =
                                result.columns.get(column).is_some_and(|each| each.numeric);
                            let (pane, key) = (pane.clone(), rows_key.clone());
                            div()
                                .id(("db-cell", row * 4096 + column))
                                .w(px(COLUMN_WIDTH))
                                .h_full()
                                .flex_none()
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .when(numeric, |cell| cell.justify_end())
                                .when(selected == Some((row, column)), |cell| cell.bg(chosen))
                                .text_color(if cell.is_some() { ink } else { dim })
                                .child(
                                    div()
                                        .truncate()
                                        .child(cell.clone().unwrap_or_else(|| "NULL".into())),
                                )
                                .on_click(move |_, _, cx| {
                                    let key = key.clone();
                                    act(&pane, cx, move |database, _| {
                                        if let Some(console) = database.consoles.get_mut(&key) {
                                            console.selected = Some((row, column));
                                        }
                                    });
                                })
                        }))
                })
                .collect()
        },
    )
    .w(px(width))
    .flex_1();
    div()
        .id(SharedString::from(format!("db-grid-{key}")))
        .flex_1()
        .min_h_0()
        .overflow_x_scroll()
        .child(
            div()
                .w(px(width))
                .h_full()
                .flex()
                .flex_col()
                .child(header)
                .child(rows),
        )
        .into_any_element()
}

/// The picked cell's whole value, to read or copy.
fn inspector(
    console: &Console,
    result: &ResultSet,
    cx: &mut Context<PluginPane>,
) -> Option<AnyElement> {
    let (row, column) = console.selected?;
    let cell = result.rows.get(row)?.get(column)?.clone();
    let name = result.columns.get(column)?.name.clone();
    let t = Tokens::get(cx).clone();
    let copied = cell.clone().unwrap_or_default();
    Some(
        div()
            .px(px(12.))
            .py(px(8.))
            .flex()
            .items_center()
            .gap(px(10.))
            .border_t_1()
            .border_color(t.border)
            .child(ui::meta(name, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::MONO)
                    .text_size(theme::ui_text(11.5))
                    .text_color(if cell.is_some() {
                        t.text_primary
                    } else {
                        t.text_tertiary
                    })
                    .child(cell.unwrap_or_else(|| "NULL".into())),
            )
            .child(
                Button::new("db-copy-value", "Copy value", Variant::Outline).on_click(listen::<
                    Database,
                    _,
                >(
                    cx,
                    move |_, host, _, _, cx| host.copy(copied.clone(), "the value", cx),
                )),
            )
            .into_any_element(),
    )
}

/// Changes the Database panel of `pane` and draws it again.
fn act(pane: &WeakEntity<PluginPane>, cx: &mut App, change: impl FnOnce(&mut Database, &mut Host)) {
    let _ = pane.update(cx, |pane, cx| {
        if let Some((database, host)) = pane.parts::<Database>() {
            change(database, host);
        }
        cx.notify();
    });
}
