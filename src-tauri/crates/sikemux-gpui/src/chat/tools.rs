//! A run of tool calls in a chat: each call's row as the core worked it out,
//! and what the call changed or printed once the reader opens it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, Action, AnyElement, App, ClipboardItem, Div, ElementId, HighlightStyle,
    Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Window,
};
use sikemux_wire::chat::{
    ChatPart, DiffLine, DiffSummary, OutputSummary, ToolAction, ToolCall, ToolDetail, ToolRow,
};

use super::{duration, Drawn};
use crate::core::ChatAsk;
use crate::editor::OpenAt;
use crate::icons::Icon;
use crate::motion;
use crate::picture::{self, Picture};
use crate::theme;
use crate::ui;
use crate::ui::shimmer::shimmer;

/// An output longer than this opens on its first lines, and the reader asks for the rest.
const OUTPUT_FOLD_LINES: usize = 12;
/// Unchanged lines kept either side of a change; the rest fold into a gap the reader opens.
const DIFF_CONTEXT: usize = 3;
/// The width of a character of the mono face the run is set in, at its size.
const MONO_CHAR: f32 = 6.6;

/// What the reader opened among a chat's tool calls, and what the core sent of each one opened.
#[derive(Default)]
pub struct Opened {
    /// Runs the reader opened or closed, by their first call's part.
    runs: HashMap<String, bool>,
    /// Calls the reader opened, by call.
    calls: HashSet<String>,
    /// Outputs shown whole and diffs shown with every unchanged line, by call.
    whole: HashSet<String>,
    details: HashMap<String, Asked>,
}

/// A call's detail as asked for: when the call had ended as of asking, what came back, and
/// the picture it printed, read once.
struct Asked {
    ended: Option<u64>,
    detail: Option<ToolDetail>,
    picture: Option<Picture>,
}

impl Opened {
    /// The open call this part is, when its detail was asked for before it ended, to ask again.
    pub fn stale(&self, part: &ChatPart) -> Option<String> {
        let ChatPart::Tool { tool, ended_at, .. } = part else {
            return None;
        };
        let asked = self.details.get(&tool.tool_call_id)?;
        (self.calls.contains(&tool.tool_call_id) && asked.ended != *ended_at)
            .then(|| tool.tool_call_id.clone())
    }

    /// Opens the call `id`, or closes it, as a click on its row does; answers whether its
    /// detail is to be asked of the core.
    pub fn toggle(&mut self, id: &str, ended: Option<u64>) -> bool {
        if !self.calls.remove(id) {
            self.calls.insert(id.to_owned());
        }
        let held = self
            .details
            .get(id)
            .is_some_and(|asked| asked.ended == ended);
        let ask = self.calls.contains(id) && !held;
        if ask {
            let asked = Asked {
                ended,
                detail: None,
                picture: None,
            };
            self.details.insert(id.to_owned(), asked);
        }
        ask
    }

    /// How many calls have their detail held, and how many the reader has open.
    pub fn counts(&self) -> (usize, usize) {
        let held = self
            .details
            .values()
            .filter(|asked| asked.detail.is_some())
            .count();
        (held, self.calls.len())
    }

    pub fn held(&mut self, tool: String, detail: ToolDetail) {
        if let Some(asked) = self.details.get_mut(&tool) {
            let printed = detail
                .output
                .as_ref()
                .and_then(|output| output.image.as_ref());
            asked.picture = printed.and_then(|image| Picture::sent(&image.mime_type, &image.data));
            asked.detail = Some(detail);
        }
    }
}

/// A tool part's fields, as a row draws them.
struct Call<'a> {
    tool: &'a ToolCall,
    diff: Option<&'a DiffSummary>,
    output: Option<&'a OutputSummary>,
    failure: Option<&'a str>,
    started: Option<u64>,
    ended: Option<u64>,
}

impl<'a> Call<'a> {
    fn of(part: &'a ChatPart) -> Option<Call<'a>> {
        let ChatPart::Tool {
            tool,
            diff,
            output,
            failure,
            started_at,
            ended_at,
            ..
        } = part
        else {
            return None;
        };
        Some(Call {
            tool,
            diff: diff.as_ref(),
            output: output.as_ref(),
            failure: failure.as_deref(),
            started: *started_at,
            ended: *ended_at,
        })
    }

    fn status(&self) -> &str {
        self.tool.status.as_deref().unwrap_or("pending")
    }

    fn running(&self) -> bool {
        !matches!(self.status(), "completed" | "failed" | "cancelled")
    }

    fn took(&self) -> u64 {
        self.ended
            .zip(self.started)
            .map_or(0, |(ended, started)| ended.saturating_sub(started))
    }

    /// The command a call ran, as it was typed.
    fn command(&self) -> Option<&str> {
        (self.tool.kind.as_deref() == Some("execute")).then(|| self.tool.title.trim())
    }

    /// Whether the row has more to show: a change, what it printed, why it failed, or a command longer than its line.
    fn opens(&self) -> bool {
        let cut_off = self
            .command()
            .is_some_and(|command| self.running() || command != self.tool.row.target);
        self.diff.is_some() || self.output.is_some() || self.failure.is_some() || cut_off
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Whole seconds, as a running call counts them.
fn seconds(ms: u64) -> String {
    if ms < 1000 {
        return "0s".into();
    }
    duration(ms - ms % 1000)
}

/// The word a row names its call by: the call's own, its server's, or the app's for what it did.
pub(super) fn word(row: &ToolRow) -> &str {
    if let Some(word) = row.verb.as_deref().or(row.server.as_deref()) {
        return word;
    }
    match row.action {
        ToolAction::Read => "read",
        ToolAction::Edit => "edit",
        ToolAction::Delete => "delete",
        ToolAction::Move => "move",
        ToolAction::Search => "search",
        ToolAction::Execute => "run",
        ToolAction::Think => "think",
        ToolAction::Fetch => "fetch",
        ToolAction::SwitchMode => "mode",
        ToolAction::Mcp => "mcp",
        ToolAction::Other => "tool",
    }
}

fn icon(call: &Call) -> Icon {
    let row = &call.tool.row;
    if call.status() == "failed" {
        return Icon::Warning;
    }
    let plugin = row
        .plugin
        .as_deref()
        .and_then(|plugin| Icon::named(plugin.strip_prefix("sikemux.").unwrap_or(plugin)));
    let own = row.icon.as_deref().and_then(Icon::named);
    plugin.or(own).unwrap_or(match row.action {
        ToolAction::Mcp => Icon::Plug,
        ToolAction::Read => Icon::File,
        ToolAction::Search => Icon::Search,
        ToolAction::Edit => Icon::Pencil,
        ToolAction::Move => Icon::Move,
        ToolAction::Delete => Icon::Trash,
        ToolAction::Execute => Icon::Command,
        ToolAction::Fetch => Icon::Globe,
        _ => Icon::Agent,
    })
}

/// A call takes the colour of what it did, mixed back toward the row's text so a run reads as a column.
fn tint(call: &Call, looks: &Drawn) -> Hsla {
    let tints = &looks.tints;
    if call.status() == "failed" {
        return tints.failed;
    }
    match call.tool.row.action {
        ToolAction::Read | ToolAction::Search | ToolAction::Fetch => tints.looked,
        ToolAction::Edit | ToolAction::Move => tints.changed,
        ToolAction::Delete => tints.deleted,
        ToolAction::Execute => tints.ran,
        ToolAction::Mcp => tints.called,
        _ => looks.quiet,
    }
}

fn chevron(id: impl Into<SharedString>, open: bool, tint: Hsla) -> impl IntoElement {
    motion::turn(
        id,
        motion::fold_angle(open),
        motion::DETAIL_FOLD,
        ui::icon(Icon::Chevron, 10.).text_color(tint),
    )
}

/// A run of calls: a line counting them and how long they took, and while the
/// agent is still adding to it, or the reader opened it, each call's row.
pub fn run(
    parts: &[&ChatPart],
    live: bool,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let calls: Vec<Call> = parts.iter().filter_map(|part| Call::of(part)).collect();
    let Some(first) = parts.first() else {
        return div().into_any_element();
    };
    let current = calls.iter().find(|call| call.running());
    let spent: u64 = calls.iter().map(Call::took).sum();
    let key = first.id().to_owned();
    let open = looks
        .reading
        .tools
        .runs
        .get(&key)
        .copied()
        .unwrap_or(live || current.is_some());
    let count = calls.len();
    let said = current.and_then(|call| call.tool.description.clone());
    let calls_said = format!("{count} tool {}", if count == 1 { "call" } else { "calls" });
    let label = match (&said, current.is_some()) {
        (Some(said), _) => div()
            .min_w_0()
            .text_size(crate::theme::ui_text(12.))
            .child(shimmer(format!("run:{key}:said"), said.clone())),
        (None, true) => div()
            .min_w_0()
            .child(shimmer(format!("run:{key}:calls"), calls_said)),
        (None, false) => div()
            .min_w_0()
            .truncate()
            .text_color(looks.quiet)
            .child(calls_said),
    };
    let time = match current {
        Some(call) => Some(seconds(
            now().saturating_sub(call.started.unwrap_or_else(now)) + spent,
        )),
        None => (spent > 0).then(|| duration(spent)),
    };
    let (reader, hover, ink) = (looks.reader.clone(), looks.hover, looks.ink);
    let sum = div()
        .id(SharedString::from(format!("run:{key}")))
        .h(px(22.))
        .ml(px(-6.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .max_w_full()
        .font_family(theme::UI)
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .hover(move |sum| sum.bg(hover).text_color(ink))
        .child(label)
        .when(said.is_some() && count > 1, |sum| {
            sum.child(
                div()
                    .flex_none()
                    .text_color(looks.faint)
                    .child(format!("{count} calls")),
            )
        })
        .children(time.map(|time| {
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_color(looks.quiet)
                .child(time)
        }))
        .child(div().flex_none().opacity(0.7).child(chevron(
            format!("run-chevron:{key}"),
            open,
            looks.quiet,
        )))
        .on_click({
            let key = key.clone();
            move |_, _, cx| {
                reader.update(cx, |reading, cx| {
                    reading.tools.runs.insert(key.clone(), !open);
                    cx.notify();
                })
            }
        });
    let kind = calls
        .iter()
        .map(|call| word(&call.tool.row).chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 16);
    let ease = if open {
        motion::UNFOLD
    } else {
        motion::FOLD_AWAY
    };
    let shown = motion::follow(
        ElementId::Name(format!("run-fold:{key}").into()),
        if open { 1. } else { 0. },
        ease,
        window,
        cx,
    );
    let body = (shown > 0.).then(|| {
        let rows = calls
            .iter()
            .enumerate()
            .map(|(ix, call)| row(call, ix + 1 == count, kind, looks));
        let rows = div().ml(px(6.)).py(px(2.)).flex().flex_col().children(rows);
        motion::folded(
            ElementId::Name(format!("run-body:{key}").into()),
            shown,
            rows,
            window,
            cx,
        )
    });
    div()
        .my(px(7.))
        .font_family(theme::MONO)
        .text_size(crate::theme::ui_text(11.))
        .child(div().flex().child(sum))
        .children(body)
        .into_any_element()
}

/// One call: a tick off the run's spine, its icon and word in the colour of what
/// it did, what it was about, and how long it took, opening onto its detail.
fn row(call: &Call, last: bool, kind: usize, looks: &Drawn) -> AnyElement {
    let id = call.tool.tool_call_id.clone();
    let row = &call.tool.row;
    let running = call.running();
    let opens = call.opens();
    let open = opens && looks.reading.tools.calls.contains(&id);
    let tint = tint(call, looks);
    let target = (!row.target.is_empty()).then(|| target(row, running, &id, looks));
    let elapsed = if call.status() == "cancelled" {
        Some("stopped".to_owned())
    } else if running {
        call.started
            .map(|started| seconds(now().saturating_sub(started)))
    } else {
        call.ended
            .zip(call.started)
            .map(|(ended, started)| duration(ended.saturating_sub(started)))
    };
    let quiet = call.diff.is_none() && call.status() != "failed";
    let group = SharedString::from(format!("tool:{id}"));
    let end = div()
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(crate::theme::ui_text(10.))
        .text_color(looks.quiet)
        .children(call.diff.map(|diff| {
            div()
                .flex()
                .gap(px(5.))
                .child(
                    div()
                        .text_color(looks.tints.added)
                        .child(format!("+{}", diff.adds)),
                )
                .child(
                    div()
                        .text_color(looks.tints.deleted_line)
                        .child(format!("−{}", diff.dels)),
                )
        }))
        .children(elapsed)
        .when(opens, |end| {
            let shown = div()
                .opacity(if quiet && !open {
                    0.
                } else if open {
                    1.
                } else {
                    0.55
                })
                .child(chevron(format!("call-chevron:{id}"), open, looks.quiet));
            end.child(shown.group_hover(group.clone(), |chevron| chevron.opacity(0.55)))
        });
    let line = div()
        .relative()
        .h(px(22.))
        .pr(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .max_w_full()
        .child(div().w(px(9.)).flex_none())
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(13.))
                .right_0()
                .rounded(theme::RADIUS_2)
                .when(opens, |fill| {
                    fill.group_hover(group.clone(), |fill| fill.bg(looks.hover))
                }),
        )
        .child(
            div()
                .w(px(16.))
                .flex_none()
                .flex()
                .justify_center()
                .child(ui::icon(icon(call), 11.).text_color(tint)),
        )
        .child(
            div()
                .w(px(kind as f32 * MONO_CHAR + 1.))
                .flex_none()
                .truncate()
                .text_color(tint)
                .child(word(row).to_owned()),
        )
        .children(target)
        .child(end);
    let reader = looks.reader.clone();
    let (bench, agent, ended) = (looks.bench.clone(), looks.agent.clone(), call.ended);
    let toggle = move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
        let ask = reader.update(cx, |reading, cx| {
            cx.notify();
            reading.tools.toggle(&id, ended)
        });
        if ask {
            let tool = id.clone();
            bench
                .update(cx, |bench, _| {
                    bench.ask_chat(ChatAsk::Detail {
                        agent: agent.clone(),
                        tool,
                    })
                })
                .ok();
        }
    };
    let node = div()
        .id(group.clone())
        .group(group)
        .relative()
        .flex()
        .flex_col()
        .child(spine(last, looks))
        .child(line)
        .when(opens, |node| node.cursor_pointer().on_click(toggle))
        .when(open, |node| node.child(detail(call, looks)));
    node.into_any_element()
}

/// Where the call reached, in its row: a file it names opens in the editor and a page it
/// names opens as a link does, without opening or closing the row.
fn target(row: &ToolRow, running: bool, id: &str, looks: &Drawn) -> Div {
    let ink = if running { looks.quiet } else { looks.ink };
    let text = row.target.as_str();
    if running && !text.is_empty() {
        return div()
            .min_w_0()
            .flex()
            .child(shimmer(format!("target:{id}:live"), text.to_owned()));
    }
    let (bench, agent) = (looks.bench.clone(), looks.agent.clone());
    let opens: Option<(std::ops::Range<usize>, Opens)> = match &row.path {
        Some(path) => {
            let path = PathBuf::from(path);
            Some((
                0..text.len(),
                Box::new(move |_, cx| {
                    bench
                        .update(cx, |bench, cx| bench.open_file(&path, cx))
                        .ok();
                }),
            ))
        }
        None => web_link(text).map(|range| {
            let url = text[range.clone()].to_owned();
            let open: Opens =
                Box::new(move |window, cx| super::link::open(&url, &agent, &bench, window, cx));
            (range, open)
        }),
    };
    let line = div().min_w_0().truncate().flex().text_color(ink);
    let line = match opens {
        Some((range, open)) => line
            .child(text[..range.start].to_owned())
            .child(
                div()
                    .id(SharedString::from(format!("target:{id}")))
                    .flex_none()
                    .text_color(looks.accent)
                    .cursor_pointer()
                    .hover(|link| link.underline())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        open(window, cx);
                    })
                    .child(text[range.clone()].to_owned()),
            )
            .child(text[range.end..].to_owned()),
        None => line.child(text.to_owned()),
    };
    line.children(row.detail.clone().map(|detail| {
        div()
            .ml(px(MONO_CHAR))
            .text_color(looks.quiet)
            .child(detail)
    }))
}

type Opens = Box<dyn Fn(&mut gpui_kit::Window, &mut App)>;

/// Where a web address starts and ends in a row's target, its closing punctuation left out.
fn web_link(text: &str) -> Option<std::ops::Range<usize>> {
    let start = text.find("https://").or_else(|| text.find("http://"))?;
    let rest = &text[start..];
    let end = rest
        .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`'))
        .unwrap_or(rest.len());
    let link = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);
    Some(start..start + link.len())
}

/// Copies `text`, without opening or closing the row it sits in.
fn copy(id: String, what: &str, text: String) -> ui::IconButton {
    ui::IconButton::new(SharedString::from(id), Icon::Copy, format!("Copy {what}"))
        .glyph(12.)
        .width(22.)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        })
}

/// The run's spine down the left of a row, and the tick that joins the row to it;
/// the last row's turns the corner instead.
fn spine(last: bool, looks: &Drawn) -> Div {
    let (spine, tick) = (looks.tints.spine, looks.tints.tick);
    if last {
        return div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(9.))
            .h(px(11.5))
            .border_l_1()
            .border_b_1()
            .border_color(spine)
            .rounded_bl(px(4.));
    }
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w(px(9.))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(1.))
                .bg(spine),
        )
        .child(
            div()
                .absolute()
                .top(px(11.))
                .left_0()
                .w(px(9.))
                .h(px(1.))
                .bg(tick),
        )
}

/// What an opened call did: its change, the command and what it printed, or why it failed.
fn detail(call: &Call, looks: &Drawn) -> AnyElement {
    let id = &call.tool.tool_call_id;
    let asked = looks.reading.tools.details.get(id);
    let held = asked.and_then(|asked| asked.detail.as_ref());
    let body = if call.diff.is_some() {
        held.and_then(|held| held.diff.as_ref()).map(|diff| {
            diff_body(id, &diff.path, diff.adds, diff.dels, &diff.lines, looks).into_any_element()
        })
    } else if call.command().is_some() || call.output.is_some() {
        let picture = asked.and_then(|asked| asked.picture.clone());
        Some(terminal(call, held, picture, looks).into_any_element())
    } else {
        call.failure.map(|failure| {
            div()
                .text_size(crate::theme::ui_text(11.))
                .line_height(relative(1.55))
                .text_color(looks.tints.failed)
                .child(failure.to_owned())
                .into_any_element()
        })
    };
    div()
        .ml(px(33.))
        .mt(px(1.))
        .mb(px(6.))
        .children(body)
        .into_any_element()
}

fn boxed(looks: &Drawn) -> Div {
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .border_1()
        .border_color(looks.tints.line_soft)
        .rounded(theme::RADIUS_2)
        .bg(looks.tints.well)
        .text_size(crate::theme::ui_text(11.))
        .line_height(crate::theme::ui_text(17.))
}

/// A command and what it printed, the way a terminal would have shown them.
fn terminal(
    call: &Call,
    held: Option<&ToolDetail>,
    picture: Option<Picture>,
    looks: &Drawn,
) -> Div {
    let id = call.tool.tool_call_id.clone();
    let output = held.and_then(|held| held.output.as_ref());
    let whole = looks.reading.tools.whole.contains(&id);
    let text = output.map_or("", |output| output.text.as_str());
    let lines: Vec<&str> = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    };
    let folds = lines.len() > OUTPUT_FOLD_LINES;
    let shown = if folds && !whole {
        lines[..OUTPUT_FOLD_LINES].join("\n")
    } else {
        text.to_owned()
    };
    let rule = looks.tints.line_soft;
    let command = call.command().map(|command| {
        div()
            .flex()
            .gap(px(7.))
            .pl(px(9.))
            .pr(px(5.))
            .py(px(5.))
            .child(div().flex_none().text_color(looks.tints.ran).child("$"))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_color(looks.ink)
                    .child(command.to_owned()),
            )
            .child(copy(
                format!("copy-command:{id}"),
                "command",
                command.to_owned(),
            ))
    });
    let shot = picture.map(|picture| {
        div().p(px(4.)).child(picture::thumb(
            SharedString::from(format!("shot:{id}")),
            picture,
            px(300.),
            px(440.),
            looks.bench.clone(),
            looks.quiet,
        ))
    });
    let printed = output
        .filter(|_| !shown.is_empty() || shot.is_none())
        .map(|_| {
            if shown.is_empty() {
                return div()
                    .px(px(9.))
                    .py(px(5.))
                    .text_size(crate::theme::ui_text(10.))
                    .text_color(looks.faint)
                    .child("No output");
            }
            div()
                .relative()
                .pl(px(9.))
                .pr(px(30.))
                .pt(px(6.))
                .pb(px(7.))
                .text_color(looks.quiet)
                .child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(crate::ui::selectable::selectable(
                            SharedString::from(format!("output:{id}")),
                            shown,
                            [],
                        )),
                )
                .child(div().absolute().top(px(2.)).right(px(4.)).child(copy(
                    format!("copy-output:{id}"),
                    "output",
                    text.to_owned(),
                )))
        });
    let more = folds.then(|| {
        let (reader, hover) = (looks.reader.clone(), looks.hover);
        let label = if whole {
            "Show fewer lines".to_owned()
        } else {
            format!("Show all {} lines", lines.len())
        };
        div()
            .id(SharedString::from(format!("more:{id}")))
            .h(px(22.))
            .pl(px(9.))
            .flex()
            .items_center()
            .text_size(crate::theme::ui_text(10.))
            .text_color(looks.faint)
            .cursor_pointer()
            .hover(move |more| more.bg(hover))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                reader.update(cx, |reading, cx| {
                    if !reading.tools.whole.remove(&id) {
                        reading.tools.whole.insert(id.clone());
                    }
                    cx.notify();
                });
            })
            .child(label)
    });
    let cut = output.is_some_and(|output| output.cut) && (!folds || whole);
    let exit = call
        .output
        .and_then(|output| output.exit_code)
        .filter(|_| call.status() == "failed");
    let mut parts: Vec<AnyElement> = command
        .into_iter()
        .map(IntoElement::into_any_element)
        .collect();
    parts.extend(shot.map(IntoElement::into_any_element));
    parts.extend(printed.map(IntoElement::into_any_element));
    parts.extend(more.map(IntoElement::into_any_element));
    if cut {
        parts.push(
            div()
                .px(px(9.))
                .py(px(5.))
                .text_size(crate::theme::ui_text(10.))
                .text_color(looks.faint)
                .child("The rest of the output was not kept")
                .into_any_element(),
        );
    }
    let ruled = parts.into_iter().enumerate().map(|(ix, part)| {
        div()
            .when(ix > 0, |part| part.border_t_1().border_color(rule))
            .child(part)
    });
    boxed(looks).children(ruled).children(exit.map(|code| {
        div()
            .px(px(9.))
            .pb(px(6.))
            .text_size(crate::theme::ui_text(10.))
            .text_color(looks.tints.failed)
            .child(format!("exit {code}"))
    }))
}

/// The lines a change made, numbered, with the lines around it and a gap for the rest.
fn diff_body(id: &str, path: &str, adds: u64, dels: u64, lines: &[DiffLine], looks: &Drawn) -> Div {
    let open = OpenAt {
        path: PathBuf::from(path),
        line: lines
            .iter()
            .find(|line| line.sign != " ")
            .and_then(|line| line.new_line.or(line.old_line))
            .map_or(0, |line| line.saturating_sub(1) as u32),
        column: None,
    };
    let whole = looks.reading.tools.whole.contains(id);
    let head = div()
        .h(px(22.))
        .px(px(9.))
        .flex()
        .items_center()
        .gap(px(8.))
        .border_b_1()
        .border_color(looks.tints.line_soft)
        .text_size(crate::theme::ui_text(10.))
        .text_color(looks.faint)
        .child(ui::icon(Icon::File, 12.).text_color(looks.faint))
        .child(
            div()
                .id(SharedString::from(format!("open:{id}")))
                .min_w_0()
                .truncate()
                .text_color(looks.quiet)
                .cursor_pointer()
                .hover(|path| path.underline())
                .tooltip(ui::tooltip("Open in editor"))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(open.boxed_clone(), cx);
                })
                .child(path.to_owned()),
        )
        .child(
            div()
                .ml_auto()
                .text_color(looks.tints.added)
                .child(format!("+{adds}")),
        )
        .child(
            div()
                .text_color(looks.tints.deleted_line)
                .child(format!("−{dels}")),
        );
    let keep = kept(lines, if whole { usize::MAX } else { DIFF_CONTEXT });
    let mut body: Vec<AnyElement> = Vec::new();
    let mut gap = 0;
    for (line, kept) in lines.iter().zip(&keep) {
        if !*kept {
            gap += 1;
            continue;
        }
        if gap > 0 {
            body.push(diff_gap(id, gap, looks));
            gap = 0;
        }
        body.push(diff_line(line, looks));
    }
    if gap > 0 {
        body.push(diff_gap(id, gap, looks));
    }
    boxed(looks)
        .child(head)
        .child(div().py(px(3.)).flex().flex_col().children(body))
}

/// Which lines are within `context` of a change.
fn kept(lines: &[DiffLine], context: usize) -> Vec<bool> {
    let mut keep = vec![false; lines.len()];
    for (ix, line) in lines.iter().enumerate() {
        if line.sign != " " {
            let end = ix.saturating_add(context).min(lines.len() - 1);
            keep[ix.saturating_sub(context)..=end]
                .iter_mut()
                .for_each(|kept| *kept = true);
        }
    }
    keep
}

fn diff_gap(id: &str, gap: usize, looks: &Drawn) -> AnyElement {
    let (reader, id, hover) = (looks.reader.clone(), id.to_owned(), looks.hover);
    div()
        .id(SharedString::from(format!("gap:{id}:{gap}")))
        .h(px(20.))
        .pl(px(42.))
        .flex()
        .items_center()
        .text_size(crate::theme::ui_text(10.))
        .text_color(looks.faint)
        .cursor_pointer()
        .hover(move |gap| gap.bg(hover))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            reader.update(cx, |reading, cx| {
                reading.tools.whole.insert(id.clone());
                cx.notify();
            });
        })
        .child(format!(
            "{gap} unchanged {}",
            if gap == 1 { "line" } else { "lines" }
        ))
        .into_any_element()
}

fn diff_line(line: &DiffLine, looks: &Drawn) -> AnyElement {
    let tints = &looks.tints;
    let (ground, sign, mark) = match line.sign.as_str() {
        "+" => (Some(tints.added_ground), tints.added, tints.added_mark),
        "-" => (
            Some(tints.deleted_ground),
            tints.deleted_line,
            tints.deleted_mark,
        ),
        _ => (None, looks.faint, tints.added_mark),
    };
    let number = if line.sign == "+" {
        line.new_line
    } else {
        line.old_line
    };
    let marked = line
        .mark
        .as_deref()
        .and_then(|mark| units_to_bytes(&line.text, mark));
    let text = StyledText::new(line.text.clone()).with_highlights(marked.map(|range| {
        (
            range,
            HighlightStyle {
                background_color: Some(mark),
                ..Default::default()
            },
        )
    }));
    div()
        .flex()
        .whitespace_nowrap()
        .when_some(ground, |line, ground| line.bg(ground))
        .child(
            div()
                .w(px(30.))
                .flex_none()
                .pr(px(8.))
                .flex()
                .justify_end()
                .text_size(crate::theme::ui_text(10.))
                .text_color(looks.faint)
                .children(number.map(|n| n.to_string())),
        )
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .text_color(sign)
                .child(line.sign.clone()),
        )
        .child(div().min_w_0().text_color(looks.ink).child(text))
        .into_any_element()
}

/// A span the core counts in UTF-16 units, as byte offsets into `text`.
fn units_to_bytes(text: &str, mark: &[u64]) -> Option<std::ops::Range<usize>> {
    let [start, end] = mark else {
        return None;
    };
    let byte = |unit: u64| {
        let mut units = 0;
        for (at, c) in text.char_indices() {
            if units >= unit {
                return at;
            }
            units += c.len_utf16() as u64;
        }
        text.len()
    };
    let range = byte(*start)..byte(*end);
    (range.start < range.end).then_some(range)
}
