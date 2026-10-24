//! The bar under an editor: the project's problems as its language servers
//! report them, and the outline of the file in front.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, ClipboardItem, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled,
};
use serde_json::{json, Value};

use super::{go, OpenAt};
use crate::core;
use crate::hosts::Place as Host;
use crate::icons::Icon;
use crate::send_to;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

/// How deep the outline indents before it stops.
const MAX_DEPTH: usize = 16;

/// What the bar under the editors shows, and what it last read.
#[derive(Default)]
pub(super) struct Insights {
    open: bool,
    tab: Tab,
    /// Every file's problems, as its language server last reported them.
    problems: BTreeMap<String, Vec<Problem>>,
    outline: Outline,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Tab {
    #[default]
    Problems,
    Outline,
}

struct Problem {
    line: u32,
    character: u32,
    severity: u8,
    /// The severity as the language server named it, or "problem" without one.
    kind: &'static str,
    message: SharedString,
    code: Option<SharedString>,
    /// What reported it, such as the server or its checker.
    source: Option<SharedString>,
}

#[derive(Default)]
enum Outline {
    #[default]
    Unavailable,
    Loading(PathBuf),
    Failed,
    Symbols(PathBuf, Vec<Symbol>),
}

struct Symbol {
    depth: usize,
    name: SharedString,
    detail: Option<SharedString>,
    kind: &'static str,
    line: u32,
    character: u32,
}

/// Reads again the problems of files whose language server said they changed.
pub(super) fn diagnostics_changed(
    bench: &mut Workbench,
    paths: &[String],
    cx: &mut Context<Workbench>,
) {
    for path in paths {
        let (asks, path) = (bench.asks.clone(), path.clone());
        cx.spawn(async move |bench, cx| {
            let Ok(read) = core::request(&asks, "lsp.diagnostics", json!({ "path": path })).await
            else {
                return;
            };
            let problems: Vec<Problem> = read
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|payload| {
                    payload["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
                .map(|diagnostic| problem(&diagnostic))
                .collect();
            let _ = bench.update(cx, |bench, cx| {
                let insights = &mut bench.editors.insights;
                match problems.is_empty() {
                    true => insights.problems.remove(&path),
                    false => insights.problems.insert(path, problems),
                };
                bench.parts.redraw(&bench.parts.stage, cx);
            });
        })
        .detach();
    }
}

fn problem(diagnostic: &Value) -> Problem {
    let start = &diagnostic["range"]["start"];
    let (severity, kind) = match diagnostic["severity"].as_str() {
        Some("error") => (0, "error"),
        Some("warning") => (1, "warning"),
        Some("information") => (2, "information"),
        Some("hint") => (3, "hint"),
        _ => (3, "problem"),
    };
    let text = |value: &Value| match value {
        Value::String(text) => Some(text.clone().into()),
        Value::Number(number) => Some(number.to_string().into()),
        _ => None,
    };
    Problem {
        line: start["line"].as_u64().unwrap_or(0) as u32,
        character: start["character"].as_u64().unwrap_or(0) as u32,
        severity,
        kind,
        message: diagnostic["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
            .into(),
        code: text(&diagnostic["code"]),
        source: text(&diagnostic["source"]),
    }
}

/// Asks the language server of the editor of `path` for its outline.
fn read_outline(bench: &mut Workbench, path: &Path, cx: &mut Context<Workbench>) {
    let Some(params) = bench
        .editors
        .open
        .get(&super::FileKey::new(Host::This, path))
        .and_then(|document| Some(document.served.as_ref()?.document()))
    else {
        bench.editors.insights.outline = Outline::Unavailable;
        return bench.parts.redraw(&bench.parts.stage, cx);
    };
    bench.editors.insights.outline = Outline::Loading(path.to_owned());
    bench.parts.redraw(&bench.parts.stage, cx);
    let (asks, path) = (bench.asks.clone(), path.to_owned());
    cx.spawn(async move |bench, cx| {
        let read = core::request(&asks, "lsp.symbols", params).await;
        let _ = bench.update(cx, |bench, cx| {
            let outline = &mut bench.editors.insights.outline;
            if !matches!(outline, Outline::Loading(asked) if *asked == path) {
                return;
            }
            *outline = match read {
                Ok(symbols) => {
                    let mut flat = Vec::new();
                    flatten(&symbols, 0, &mut flat);
                    Outline::Symbols(path, flat)
                }
                Err(_) => Outline::Failed,
            };
            bench.parts.redraw(&bench.parts.stage, cx);
        });
    })
    .detach();
}

fn flatten(symbols: &Value, depth: usize, flat: &mut Vec<Symbol>) {
    for symbol in symbols.as_array().into_iter().flatten() {
        let start = &symbol["selectionRange"]["start"];
        flat.push(Symbol {
            depth: depth.min(MAX_DEPTH),
            name: symbol["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
                .into(),
            detail: symbol["detail"]
                .as_str()
                .map(|detail| detail.to_owned().into()),
            kind: kind_word(symbol["kind"].as_u64().unwrap_or(0)),
            line: start["line"].as_u64().unwrap_or(0) as u32,
            character: start["character"].as_u64().unwrap_or(0) as u32,
        });
        flatten(&symbol["children"], depth + 1, flat);
    }
}

/// A symbol's kind as the protocol numbers them, in a word.
fn kind_word(kind: u64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type",
        _ => "symbol",
    }
}

fn severity_colour(severity: u8, t: &Tokens) -> Hsla {
    match severity {
        0 => t.danger,
        1 => t.warn,
        2 => t.accent,
        _ => t.text_secondary,
    }
}

/// The bar under the editor of `path`: collapsed to its header, or open on
/// the project's problems or the file's outline.
pub(super) fn bar(
    bench: &Workbench,
    this: &Entity<Workbench>,
    path: &Path,
    project: Option<&Path>,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let insights = &bench.editors.insights;
    let in_project =
        |file: &str| project.is_none_or(|project| Path::new(file).starts_with(project));
    let mut problems: Vec<(&str, &Problem)> = insights
        .problems
        .iter()
        .filter(|(file, _)| in_project(file))
        .flat_map(|(file, problems)| problems.iter().map(move |problem| (file.as_str(), problem)))
        .collect();
    problems
        .sort_by_key(|(file, problem)| (problem.severity, *file, problem.line, problem.character));
    let tab = |id: &'static str, label: SharedString, shown: Tab| {
        let (this, path) = (this.clone(), path.to_owned());
        let chosen = insights.open && insights.tab == shown;
        div()
            .id(id)
            .px(px(8.))
            .h(px(22.))
            .flex()
            .items_center()
            .rounded(theme::RADIUS_2)
            .cursor_pointer()
            .text_color(if chosen {
                t.text_primary
            } else {
                t.text_tertiary
            })
            .when(chosen, |tab| tab.bg(t.surface_chosen))
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    let insights = &mut bench.editors.insights;
                    insights.open = !(insights.open && insights.tab == shown);
                    insights.tab = shown;
                    if insights.open && shown == Tab::Outline {
                        read_outline(bench, &path, cx);
                    }
                    bench.parts.redraw(&bench.parts.stage, cx);
                })
            })
            .child(label)
    };
    let header = div()
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(29.))
        .px(px(8.))
        .border_t_1()
        .border_color(t.border)
        .child(tab(
            "insights-problems",
            format!("Problems {}", problems.len()).into(),
            Tab::Problems,
        ))
        .child(tab("insights-outline", "Outline".into(), Tab::Outline))
        .child(div().flex_1())
        .when(insights.open && insights.tab == Tab::Outline, |header| {
            let (this, path) = (this.clone(), path.to_owned());
            header.child(
                ui::IconButton::new("outline-refresh", Icon::Refresh, "Refresh the outline")
                    .small()
                    .on_click(move |_, _, cx| {
                        this.update(cx, |bench, cx| read_outline(bench, &path, cx))
                    }),
            )
        })
        .child({
            let this = this.clone();
            ui::IconButton::new(
                "insights-toggle",
                if insights.open {
                    Icon::ArrowDown
                } else {
                    Icon::ArrowUp
                },
                if insights.open { "Hide" } else { "Show" },
            )
            .small()
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    bench.editors.insights.open ^= true;
                    bench.parts.redraw(&bench.parts.stage, cx);
                })
            })
        });
    let body = insights.open.then(|| {
        let rows: Vec<AnyElement> = match insights.tab {
            Tab::Problems => problems
                .iter()
                .enumerate()
                .map(|(index, (file, problem))| {
                    let at = OpenAt {
                        path: PathBuf::from(file),
                        line: problem.line,
                        column: Some(problem.character),
                    };
                    let place = format!(
                        "{}:{}:{}",
                        project
                            .and_then(|project| Path::new(file).strip_prefix(project).ok())
                            .unwrap_or(Path::new(file))
                            .display(),
                        problem.line + 1,
                        problem.character + 1
                    );
                    row(this, ("problem", index), at.clone(), &t)
                        .context_menu(problem_menu(this, at, problem, project))
                        .child(
                            div()
                                .size(px(7.))
                                .rounded_full()
                                .bg(severity_colour(problem.severity, &t)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_color(t.text_primary)
                                .child(problem.message.clone()),
                        )
                        .children(
                            problem
                                .code
                                .clone()
                                .map(|code| div().text_color(t.text_tertiary).child(code)),
                        )
                        .child(div().text_color(t.text_tertiary).child(place))
                        .into_any_element()
                })
                .collect(),
            Tab::Outline => match &insights.outline {
                Outline::Symbols(file, symbols) if file == path => symbols
                    .iter()
                    .enumerate()
                    .map(|(index, symbol)| {
                        let at = OpenAt {
                            path: file.clone(),
                            line: symbol.line,
                            column: Some(symbol.character),
                        };
                        row(this, ("symbol", index), at, &t)
                            .pl(px(12. + 14. * symbol.depth as f32))
                            .child(div().text_color(t.text_tertiary).child(symbol.kind))
                            .child(div().text_color(t.text_primary).child(symbol.name.clone()))
                            .children(symbol.detail.clone().map(|detail| {
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(t.text_tertiary)
                                    .child(detail)
                            }))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_color(t.text_tertiary)
                                    .child((symbol.line + 1).to_string()),
                            )
                            .into_any_element()
                    })
                    .collect(),
                outline => {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let said = match outline {
                        Outline::Loading(_) => "Loading outline…".to_owned(),
                        Outline::Failed => {
                            "The language server could not provide an outline".to_owned()
                        }
                        Outline::Symbols(..) => format!("No symbols in {name}"),
                        Outline::Unavailable => "Outline is unavailable for this file".to_owned(),
                    };
                    vec![div()
                        .px(px(12.))
                        .py(px(8.))
                        .text_color(t.text_tertiary)
                        .child(said)
                        .into_any_element()]
                }
            },
        };
        div()
            .id("insights-rows")
            .flex()
            .flex_col()
            .h(px(230.))
            .min_h(px(120.))
            .overflow_y_scroll()
            .children(rows)
    });
    div()
        .flex()
        .flex_col()
        .font_family(theme::MONO)
        .text_size(crate::theme::ui_text(12.))
        .child(header)
        .children(body)
}

/// What a right click on a problem offers: going to it, copying what it says, or handing it to
/// an agent, as the Mac's does.
fn problem_menu(
    this: &Entity<Workbench>,
    at: OpenAt,
    problem: &Problem,
    project: Option<&Path>,
) -> impl Fn(
    gpui_kit::component::menu::PopupMenu,
    &mut gpui_kit::Window,
    &mut Context<gpui_kit::component::menu::PopupMenu>,
) -> gpui_kit::component::menu::PopupMenu
       + 'static {
    let this = this.clone();
    let message = problem.message.clone();
    let delivery = delivery(problem, &at, project);
    move |popup, window, cx| {
        let (go_to, at) = (this.clone(), at.clone());
        let message = message.clone();
        let popup = popup
            .item(
                PopupMenuItem::new("Go to Problem")
                    .icon(Icon::Editor)
                    .on_click(move |_, _, cx| {
                        let at = at.clone();
                        go_to.update(cx, |bench, cx| go(bench, at, cx))
                    }),
            )
            .item(
                PopupMenuItem::new("Copy Message")
                    .icon(Icon::Copy)
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(message.to_string()))
                    }),
            )
            .separator();
        send_to::submenu_with_text(
            popup,
            "Send to Agent…",
            this.downgrade(),
            delivery.clone(),
            window,
            cx,
        )
    }
}

/// A problem as an agent is told it: what it is, where, and what reported it, then its message.
fn delivery(problem: &Problem, at: &OpenAt, project: Option<&Path>) -> String {
    let file = project
        .and_then(|project| at.path.strip_prefix(project).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .or_else(|| at.path.file_name().map(Path::new))
        .unwrap_or(&at.path);
    let mut kind = problem.kind.to_owned();
    if let Some(first) = kind.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    let source: Vec<&str> = [problem.source.as_deref(), problem.code.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let from = match source.is_empty() {
        true => String::new(),
        false => format!(" from {}", source.join(" ")),
    };
    let message = match problem.message.contains('\n') {
        true => send_to::code_fence(&problem.message),
        false => problem.message.to_string(),
    };
    format!(
        "{kind} at {}:{}:{}{from}:\n\n{message}\n",
        file.display(),
        problem.line + 1,
        problem.character + 1,
    )
}

/// A row that takes the editor to `at` when clicked.
fn row(
    this: &Entity<Workbench>,
    id: (&'static str, usize),
    at: OpenAt,
    t: &Tokens,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let this = this.clone();
    let hover = t.surface_hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(24.))
        .px(px(12.))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_, _, cx| {
            let at = at.clone();
            this.update(cx, |bench, cx| go(bench, at, cx))
        })
}
