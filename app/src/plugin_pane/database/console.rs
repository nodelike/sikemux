//! A console on a saved database: SQL to run, and what the last run gave, a statement at a
//! time, with each result's rows in a grid.

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{Editor, EditorState, InputEvent, RopeExt};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, AppContext, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, WeakEntity, Window,
};
use serde_json::json;
use sikemux_plugin_database_views::{Connection, CopyFormat, QueryOutcome, ResultSet};

use super::{Database, Host, PluginPane};
use crate::icons::Icon;
use crate::plugin_pane::{after, listen};
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS_H, TEXT_XS,
};
use crate::ui::{self, Button, MenuRow, Pick, Variant};

/// The most rows kept from each statement, as the person can pick it.
const ROW_LIMITS: [u32; 4] = [100, 500, 1000, 5000];
const ROW_LIMIT: u32 = 500;
const ROW_HEIGHT: f32 = 26.;
const HEADER_HEIGHT: f32 = 30.;
/// A column is as wide as its longest value in the first rows, within these bounds.
const COLUMN_WIDTHS: (f32, f32) = (56., 340.);
const MEASURED_ROWS: usize = 200;
const CELL_PADDING: f32 = 10.;
/// Roughly how wide a character is in the grid's mono face, and in its headers' names.
const CELL_CHARACTER: f32 = 7.;
const NAME_CHARACTER: f32 = 6.6;
const TYPE_CHARACTER: f32 = 6.;
const SQL_TEXT: f32 = 12.;
/// The editor's lines are one and a half times its text high.
const SQL_LINE: f32 = SQL_TEXT * 1.5;
const SQL_ROWS: (usize, usize) = (6, 16);
/// A table's data needs room for rows more than for SQL, so its query starts one line high.
const TABLE_SQL_ROWS: (usize, usize) = (1, 16);

/// One console's SQL and what its last run gave.
pub(super) struct Console {
    connection: String,
    sql: Entity<EditorState>,
    sql_rows: (usize, usize),
    limit: u32,
    /// Names the run going now, so Stop reaches it and not another console's.
    running: Option<String>,
    outcome: Option<Rc<QueryOutcome>>,
    error: Option<SharedString>,
    /// How wide each result's columns are drawn, measured once when the results arrive.
    widths: Vec<Rc<[f32]>>,
    /// The result in front, when a run gave several.
    picked: usize,
    /// The cell picked in the grid, by row and column.
    selected: Option<(usize, usize)>,
}

impl Console {
    pub(super) fn new(
        key: String,
        connection: String,
        table: bool,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> Self {
        let sql = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language("sql")
                .line_number(false)
                .folding(false)
                .indent_guides(false)
                .placeholder("select * from …");
            state.set_highlighter_factory(crate::syntax::highlighters(cx), cx);
            state
        });
        cx.subscribe(&sql, move |pane, _, event: &InputEvent, cx| match event {
            InputEvent::PressEnter {
                secondary: true, ..
            } => {
                if let Some((database, host)) = pane.parts::<Database>() {
                    run(database, host, &key, cx);
                }
            }
            InputEvent::Change => cx.notify(),
            _ => {}
        })
        .detach();
        Self {
            connection,
            sql,
            sql_rows: if table { TABLE_SQL_ROWS } else { SQL_ROWS },
            limit: ROW_LIMIT,
            running: None,
            outcome: None,
            error: None,
            widths: Vec::new(),
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
        json!({ "id": console.connection, "sql": sql, "limit": console.limit, "run": run }),
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
                    console.widths = outcome.results.iter().map(column_widths).collect();
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
    let Some((connection, run)) = database
        .consoles
        .get(key)
        .and_then(|console| Some((console.connection.clone(), console.running.clone()?)))
    else {
        return;
    };
    let call = host.call("cancel", json!({ "id": connection, "run": run }));
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
    lead: Option<AnyElement>,
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
    let version = connection.connected.clone().map(|version| {
        let short = version
            .split_once(" (")
            .map_or(version.as_str(), |(short, _)| short)
            .to_owned();
        div()
            .id(SharedString::from(format!("db-version-{key}")))
            .min_w_0()
            .child(ui::meta(short, cx).truncate())
            .tooltip(ui::tooltip(version))
    });
    let toolbar = div()
        .h(px(44.))
        .flex_none()
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .children(lead)
        .child(
            div()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(SPACE_2)
                .child(
                    ui::text(connection.name.clone(), 12.5, FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .flex_none(),
                )
                .children(version),
        )
        .child(div().flex_1())
        .children(connection.read_only.then(|| ui::caps("Read only", cx)))
        .children(running.then(|| {
            Button::new("db-stop", "Stop", Variant::Outline).on_click(listen::<Database, _>(
                cx,
                move |database, host, _, _, cx| stop(database, host, &stop_key, cx),
            ))
        }))
        .child(ui::split_button(
            &format!("db-run-{key}"),
            if running { "Running…" } else { "Run" },
            Some(Icon::Run),
            running || !written,
            false,
            listen::<Database, _>(cx, move |database, host, _, _, cx| {
                run(database, host, &run_key, cx)
            }),
            Some(limit_menu(key, console.limit, cx.weak_entity())),
            cx,
        ));
    let (fewest, most) = console.sql_rows;
    let rows = console.sql.read(cx).text().lines_len().clamp(fewest, most);
    let editor = div()
        .flex_none()
        .mx(SPACE_3)
        .mb(SPACE_3)
        .px(SPACE_1H)
        .py(px(7.))
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .flex()
        .items_start()
        .gap(SPACE_2)
        .child(
            div().flex_1().min_w_0().child(
                Editor::new(&console.sql)
                    .appearance(false)
                    .with_size(Size::XSmall)
                    .text_size(theme::ui_text(SQL_TEXT))
                    .h(theme::ui_text(rows as f32 * SQL_LINE)),
            ),
        )
        .child(
            div()
                .flex_none()
                .h(theme::ui_text(SQL_LINE))
                .pr(SPACE_1)
                .flex()
                .items_center()
                .child(ui::meta("⌘↵ to run", cx).text_color(t.ink_faint)),
        );
    let below = match (&console.error, &console.outcome) {
        (Some(error), _) => div()
            .m(SPACE_3)
            .mt_0()
            .px(SPACE_3)
            .py(SPACE_2)
            .rounded(RADIUS_2)
            .bg(t.danger.opacity(0.12))
            .text_size(theme::ui_text(TEXT_XS))
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

/// The Run button's menu: how many rows each statement keeps.
fn limit_menu(key: &str, limit: u32, pane: WeakEntity<PluginPane>) -> ui::SplitMenu {
    let key = key.to_owned();
    Rc::new(move |mut popup: PopupMenu, _, cx| {
        popup = popup.min_w(px(200.)).label("Rows kept per statement");
        for each in ROW_LIMITS {
            let (pane, key) = (pane.clone(), key.clone());
            popup = popup.item(
                MenuRow::new(format!("{each} rows"))
                    .item(cx)
                    .checked(each == limit)
                    .on_click(move |_, _, cx| {
                        let key = key.clone();
                        act(&pane, cx, move |database, _| {
                            if let Some(console) = database.consoles.get_mut(&key) {
                                console.limit = each;
                            }
                        });
                    }),
            );
        }
        popup
    })
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
                .map(|number| (SharedString::from(format!("Result {number}")), None))
                .collect(),
            at,
        )
        .slide()
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
    let copies = (!result.columns.is_empty()).then(|| copy_menu(key, &outcome, at, cx));
    let send = send_to_agent(key, result, host, cx);
    let bar = div()
        .h(px(40.))
        .flex_none()
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .children(statements)
        .child(ui::meta(result.summary.clone(), cx))
        .child(ui::meta("·", cx).text_color(t.ink_faint))
        .child(ui::meta(format!("{} ms", outcome.millis), cx))
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

/// Copies the result in front as tab-separated rows, as spreadsheets paste them, or as CSV.
fn copy_menu(
    key: &str,
    outcome: &Rc<QueryOutcome>,
    at: usize,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let (pane, outcome) = (cx.weak_entity(), outcome.clone());
    ui::select(
        SharedString::from(format!("db-copy-{key}")),
        "Copy",
        vec![
            Pick::new("tsv", "Copy rows").hint("Tab-separated"),
            Pick::new("csv", "Copy as CSV"),
        ],
        None,
        false,
        move |picked, _, cx| {
            let Some(result) = outcome.results.get(at) else {
                return;
            };
            let (format, what) = match picked.as_ref() {
                "csv" => (CopyFormat::Csv, "the results as CSV"),
                _ => (CopyFormat::Tsv, "the results"),
            };
            let params = json!({
                "result": { "columns": result.columns, "rows": result.rows },
                "format": format,
            });
            let _ = pane.update(cx, |pane, cx| {
                let call = pane.host.call("copyResult", params);
                after::<Database, _>(
                    call,
                    move |_, host, done, cx| match done {
                        Ok(text) => {
                            host.copy(text.as_str().unwrap_or_default().to_owned(), what, cx)
                        }
                        Err(failure) => host.said(&failure, cx),
                    },
                    cx,
                );
            });
        },
        cx,
    )
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

/// How wide each column is drawn: enough for its name and type and its longest value in the
/// first rows, within bounds, so a short id column stays narrow and long text is cut.
fn column_widths(result: &ResultSet) -> Rc<[f32]> {
    let (narrowest, widest) = COLUMN_WIDTHS;
    result
        .columns
        .iter()
        .enumerate()
        .map(|(at, column)| {
            let typed = match column.type_name.is_empty() {
                true => 0.,
                false => 6. + column.type_name.chars().count() as f32 * TYPE_CHARACTER,
            };
            let header = column.name.chars().count() as f32 * NAME_CHARACTER + typed;
            let longest = result
                .rows
                .iter()
                .take(MEASURED_ROWS)
                .map(|row| match row.get(at) {
                    Some(Some(cell)) => cell.chars().count(),
                    _ => "NULL".len(),
                })
                .max()
                .unwrap_or(0);
            let content = header.max(longest as f32 * CELL_CHARACTER);
            (content + CELL_PADDING * 2.)
                .clamp(narrowest, widest)
                .ceil()
        })
        .collect()
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
    let widths = console
        .widths
        .get(at)
        .cloned()
        .unwrap_or_else(|| column_widths(result));
    let digits = result.rows.len().max(1).to_string().len() as f32;
    let number_width = (digits * CELL_CHARACTER + CELL_PADDING * 2.).max(40.);
    let width = number_width + widths.iter().sum::<f32>();
    let (line, rule) = (t.line_soft, t.border);
    let header = div()
        .w(px(width))
        .h(px(HEADER_HEIGHT))
        .flex_none()
        .flex()
        .bg(t.surface)
        .border_b_1()
        .border_color(rule)
        .child(
            div()
                .w(px(number_width))
                .h_full()
                .flex_none()
                .border_r_1()
                .border_color(line),
        )
        .children(
            result
                .columns
                .iter()
                .zip(widths.iter())
                .map(|(column, width)| {
                    div()
                        .w(px(*width))
                        .h_full()
                        .flex_none()
                        .px(px(CELL_PADDING))
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .overflow_hidden()
                        .border_r_1()
                        .border_color(line)
                        .when(column.numeric, |cell| cell.justify_end())
                        .child(
                            ui::text(column.name.clone(), 11.5, FontWeight::SEMIBOLD)
                                .text_color(t.text_primary)
                                .flex_none(),
                        )
                        .children((!column.type_name.is_empty()).then(|| {
                            ui::mono(column.type_name.clone(), 10., cx)
                                .text_color(t.ink_faint)
                                .truncate()
                        }))
                }),
        );
    let (ink, dim, faint, chosen, hover) = (
        t.text_secondary,
        t.text_tertiary,
        t.ink_faint,
        t.surface_chosen,
        t.surface_hover,
    );
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
                        .id(("db-row", row))
                        .w(px(width))
                        .h(px(ROW_HEIGHT))
                        .flex()
                        .border_b_1()
                        .border_color(line)
                        .hover(move |style| style.bg(hover))
                        .font_family(theme::MONO)
                        .font_features(theme::tabular())
                        .text_size(theme::ui_text(TEXT_2XS_H))
                        .child(
                            div()
                                .w(px(number_width))
                                .h_full()
                                .flex_none()
                                .px(px(CELL_PADDING))
                                .flex()
                                .items_center()
                                .justify_end()
                                .border_r_1()
                                .border_color(line)
                                .text_color(faint)
                                .child((row + 1).to_string()),
                        )
                        .children(cells.iter().zip(widths.iter()).enumerate().map(
                            |(column, (cell, width))| {
                                let numeric =
                                    result.columns.get(column).is_some_and(|each| each.numeric);
                                let (pane, key) = (pane.clone(), rows_key.clone());
                                div()
                                    .id(("db-cell", row * 4096 + column))
                                    .w(px(*width))
                                    .h_full()
                                    .flex_none()
                                    .px(px(CELL_PADDING))
                                    .flex()
                                    .items_center()
                                    .border_r_1()
                                    .border_color(line)
                                    .when(numeric, |cell| cell.justify_end())
                                    .when(selected == Some((row, column)), |cell| cell.bg(chosen))
                                    .text_color(if cell.is_some() { ink } else { dim })
                                    .child(
                                        div()
                                            .min_w_0()
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
                            },
                        ))
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
            .px(SPACE_3)
            .py(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .border_t_1()
            .border_color(t.border)
            .child(ui::meta(name, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::MONO)
                    .text_size(theme::ui_text(TEXT_2XS_H))
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
