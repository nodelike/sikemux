//! A run of tool calls in a chat: each call's row as the core worked it out,
//! and what the call changed or printed once the reader opens it.

use sikemux_view::chat::ToolStatus;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gpui::{Context, Render};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, ClipboardItem, Div, ElementId, FontWeight, HighlightStyle,
    Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Window,
};
use gpui_transcript::{FileKind, FilePress};
use sikemux_view::chat::{
    ChatPart, DiffLine, DiffSummary, OutputSummary, ToolAction, ToolCall, ToolDetail, ToolRow,
};

use super::Drawn;
use crate::core::ChatAsk;
use crate::icons::Icon;
use crate::motion;
use crate::picture::{self, Picture};
use crate::theme::{self, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, TEXT_2XS, TEXT_3XS, TEXT_XS};
use crate::ui;
use crate::ui::clock::{self, Slot};
use crate::ui::shimmer::shimmer;
use crate::ui::time;

/// An output longer than this opens on its last lines, and the reader asks for the rest.
const OUTPUT_TAIL_LINES: usize = 8;
/// Unchanged lines kept either side of a change; the rest fold into a gap the reader opens.
const DIFF_CONTEXT: usize = 3;
/// A diff taller than this scrolls inside its card.
const DIFF_MOST_HEIGHT: f32 = 360.;
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
    /// Where each diff that scrolls inside its card is scrolled to, by call.
    scrolls: RefCell<HashMap<String, ScrollHandle>>,
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
    /// Whether the reader opened or closed the run starting at `head`, if they touched it.
    pub fn run_open(&self, head: &str) -> Option<bool> {
        self.runs.get(head).copied()
    }

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

    fn status(&self) -> ToolStatus {
        self.tool.status.unwrap_or(ToolStatus::Pending)
    }

    fn running(&self) -> bool {
        self.status().running()
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

/// The seconds since `started`, after `before` already spent, which the clock counts on in
/// place without drawing the transcript again.
fn running_for(key: String, started: u64, before: u64) -> AnyElement {
    Slot::new(key, RunningFor { started, before }).into_any_element()
}

struct RunningFor {
    started: u64,
    before: u64,
}

impl Render for RunningFor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        clock::second(window, cx);
        time::elapsed((time::now_ms().saturating_sub(self.started) + self.before) / 1000)
    }
}

/// A clock's time fading in as it settles on how long its call or run took.
fn settled(id: String, running: bool, time: AnyElement) -> AnyElement {
    motion::replay(
        ElementId::Name(id.into()),
        running,
        motion::TEXT_SWAP,
        move |shown| {
            div()
                .opacity(if running { 1. } else { shown })
                .child(time)
                .into_any_element()
        },
    )
    .into_any_element()
}

/// The word a row names its call by: the call's own, its server's, or the app's for what it did.
pub(super) fn word(row: &ToolRow) -> &str {
    if let Some(word) = row.verb.as_deref().or(row.server.as_deref()) {
        return word;
    }
    match row.action {
        ToolAction::Read => t!("read"),
        ToolAction::Edit => t!("edit"),
        ToolAction::Delete => t!("delete"),
        ToolAction::Move => t!("move"),
        ToolAction::Search => t!("search"),
        ToolAction::Execute => t!("run"),
        ToolAction::Think => t!("think"),
        ToolAction::Fetch => t!("fetch"),
        ToolAction::SwitchMode => t!("mode"),
        ToolAction::Mcp => "mcp",
        ToolAction::Other => t!("tool"),
    }
}

fn plugin_mark(row: &ToolRow) -> Option<Icon> {
    let plugin = row.plugin.as_deref()?;
    Icon::named(plugin.strip_prefix("sikemux.").unwrap_or(plugin))
}

fn icon(call: &Call) -> Icon {
    let row = &call.tool.row;
    if call.status() == ToolStatus::Failed {
        return Icon::Warning;
    }
    let plugin = plugin_mark(row);
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
/// A plugin's call shows only its service's own colours, in its mark.
fn tint(call: &Call, looks: &Drawn) -> Hsla {
    let tints = &looks.tints;
    if call.status() == ToolStatus::Failed {
        return tints.failed;
    }
    if plugin_mark(&call.tool.row).is_some() {
        return looks.quiet;
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

/// The calls a run has drawn, and those that joined it since it first drew, with when.
/// A run first drawn live with one call has just come into the chat itself.
struct Arrivals {
    seen: HashSet<String>,
    run: Option<Instant>,
    calls: Vec<(String, Instant)>,
}

/// Whether the run `key` is still coming into the chat, and which of its calls are still
/// coming into it. A run drawn again, as when scrolled back to, only shows.
fn arrivals(
    key: &str,
    calls: &[Call],
    live: bool,
    window: &mut Window,
    cx: &mut App,
) -> (bool, HashSet<String>) {
    let lasts = motion::ROW_ARRIVAL.lasts();
    let now = Instant::now();
    let state = window.use_keyed_state(
        ElementId::Name(format!("run-arrivals:{key}").into()),
        cx,
        |_, _| Arrivals {
            seen: calls
                .iter()
                .map(|call| call.tool.tool_call_id.clone())
                .collect(),
            run: (live && calls.len() == 1).then_some(now),
            calls: Vec::new(),
        },
    );
    state.update(cx, |arrivals, _| {
        let run = arrivals.run.is_some_and(|at| at.elapsed() < lasts);
        for call in calls {
            let id = &call.tool.tool_call_id;
            if arrivals.seen.insert(id.clone()) && !run {
                arrivals.calls.push((id.clone(), now));
            }
        }
        arrivals.calls.retain(|(_, at)| at.elapsed() < lasts);
    });
    let arrivals = state.read(cx);
    let run = arrivals.run.is_some_and(|at| at.elapsed() < lasts);
    let calls = arrivals.calls.iter().map(|(id, _)| id.clone()).collect();
    (run, calls)
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
    let key = first.id().to_owned();
    let (run_arrives, arriving) = arrivals(&key, &calls, live, window, cx);
    let spent: u64 = calls.iter().map(Call::took).sum();
    let open = looks
        .reading
        .tools
        .runs
        .get(&key)
        .copied()
        .unwrap_or(live || current.is_some());
    let count: u64 = calls.iter().map(|call| call.tool.row.calls()).sum();
    let said = current.and_then(|call| call.tool.description.clone());
    let calls_said = match count {
        1 => t!("1 tool call").to_owned(),
        count => t!("{count} tool calls", count = count),
    };
    let label = match (&said, current.is_some()) {
        (Some(said), _) => div()
            .min_w_0()
            .text_size(crate::theme::ui_text(TEXT_XS))
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
        Some(call) => Some(running_for(
            format!("run-seconds:{key}"),
            call.started.unwrap_or_else(time::now_ms),
            spent,
        )),
        None => (spent > 0).then(|| time::took(spent).into_any_element()),
    };
    let time = time.map(|time| settled(format!("run-time:{key}"), current.is_some(), time));
    let (reader, hover, ink) = (looks.reader.clone(), looks.hover, looks.ink);
    let sum = div()
        .id(SharedString::from(format!("run:{key}")))
        .h(px(22.))
        .ml(px(-6.))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
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
                    .child(t!("{count} calls", count = count)),
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
        let rows = calls.iter().enumerate().map(|(ix, call)| {
            let row = row(call, ix + 1 == calls.len(), kind, looks, window, cx);
            let id = &call.tool.tool_call_id;
            if !arriving.contains(id) {
                return row;
            }
            let key = SharedString::from(format!("call-arrive:{id}"));
            motion::arriving(key, motion::ROW_ARRIVAL, row).into_any_element()
        });
        let rows = div()
            .ml(SPACE_1H)
            .py(SPACE_0)
            .flex()
            .flex_col()
            .children(rows);
        motion::folded(
            ElementId::Name(format!("run-body:{key}").into()),
            shown,
            rows,
            window,
            cx,
        )
    });
    let run = div()
        .font_family(theme::MONO)
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .child(div().flex().child(sum))
        .children(body);
    if !run_arrives {
        return run.into_any_element();
    }
    let key = SharedString::from(format!("run-arrive:{key}"));
    motion::arriving(key, motion::ROW_ARRIVAL, run).into_any_element()
}

/// Opens a call's detail or closes it, asking the core for it the first time.
fn toggle_detail(
    call: &Call,
    looks: &Drawn,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
    let id = call.tool.tool_call_id.clone();
    let reader = looks.reader.clone();
    let (bench, agent, ended) = (looks.bench.clone(), looks.agent.clone(), call.ended);
    move |_, _, cx| {
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
    }
}

/// A call's detail unfolding under its line while the reader has it open.
fn opened_detail(
    call: &Call,
    open: bool,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let id = &call.tool.tool_call_id;
    let shown = motion::follow(
        ElementId::Name(format!("call-fold:{id}").into()),
        if open { 1. } else { 0. },
        if open {
            motion::UNFOLD
        } else {
            motion::FOLD_AWAY
        },
        window,
        cx,
    );
    (shown > 0.).then(|| {
        let detail = detail(call, looks, cx);
        motion::folded(
            ElementId::Name(format!("call-body:{id}").into()),
            shown,
            detail,
            window,
            cx,
        )
        .into_any_element()
    })
}

/// One call: a tick off the run's spine, its icon and word in the colour of what
/// it did, what it was about, and how long it took, opening onto its detail.
fn row(
    call: &Call,
    last: bool,
    kind: usize,
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = call.tool.tool_call_id.clone();
    let row = &call.tool.row;
    let running = call.running();
    let opens = call.opens();
    let open = opens && looks.reading.tools.calls.contains(&id);
    let tint = tint(call, looks);
    let target = (!row.target.is_empty()).then(|| target(row, running, &id, looks, cx));
    let elapsed = if call.status() == ToolStatus::Cancelled {
        Some(t!("stopped").into_any_element())
    } else if running {
        call.started
            .map(|started| running_for(format!("call-seconds:{id}"), started, 0))
    } else {
        call.ended
            .zip(call.started)
            .map(|(ended, started)| time::took(ended.saturating_sub(started)).into_any_element())
    };
    let elapsed = elapsed.map(|elapsed| settled(format!("call-time:{id}"), running, elapsed));
    let quiet = call.diff.is_none() && call.status() != ToolStatus::Failed;
    let group = SharedString::from(format!("tool:{id}"));
    let end = div()
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(crate::theme::ui_text(TEXT_3XS))
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
        .children(row.times.map(|times| format!("×{times}")))
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
        .pr(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
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
                .child(ui::picture(icon(call), 11., tint)),
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
    let toggle = toggle_detail(call, looks);
    let node = div()
        .id(group.clone())
        .group(group)
        .relative()
        .flex()
        .flex_col()
        .child(spine(last, looks))
        .child(line)
        .when(opens, |node| node.cursor_pointer().on_click(toggle));
    let node = node.children(opened_detail(call, open, looks, window, cx));
    node.into_any_element()
}

/// Where the call reached, in its row: a file it names opens as a file the chat names does,
/// and a page it names as a link does, without opening or closing the row.
fn target(row: &ToolRow, running: bool, id: &str, looks: &Drawn, cx: &App) -> Div {
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
            let (path, files) = (path.clone(), looks.files.clone());
            Some((
                0..text.len(),
                std::rc::Rc::new(move |press, window, cx| {
                    let file = (path.as_str(), FileKind::File, press);
                    super::files::pressed(&agent, &files, &bench, file, window, cx);
                }),
            ))
        }
        None => web_link(text).map(|range| {
            let url = text[range.clone()].to_owned();
            let open: Opens = std::rc::Rc::new(move |press, window, cx| {
                if !matches!(press, FilePress::Menu(_)) {
                    super::link::open(&url, &agent, &bench, window, cx)
                }
            });
            (range, open)
        }),
    };
    let named = row
        .path
        .as_deref()
        .and_then(|path| looks.files.borrow().resolve(path));
    let line = div()
        .min_w_0()
        .truncate()
        .flex()
        .items_center()
        .text_color(ink)
        .children(named.map(|named| {
            let name = std::path::Path::new(&named.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            div()
                .mr(px(5.))
                .child(crate::file_icons::glyph(&name, 12., cx))
        }));
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
                    .on_click({
                        let open = open.clone();
                        move |event, window, cx| {
                            cx.stop_propagation();
                            let press = if event.click_count() > 1 {
                                FilePress::OpenTwice
                            } else {
                                FilePress::Open
                            };
                            open(press, window, cx);
                        }
                    })
                    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        cx.stop_propagation();
                        open(FilePress::Menu(event.position), window, cx);
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

type Opens = std::rc::Rc<dyn Fn(FilePress, &mut gpui_kit::Window, &mut App)>;

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
fn copy(id: String, label: &'static str, text: String) -> ui::IconButton {
    ui::IconButton::new(SharedString::from(id), Icon::Copy, label)
        .glyph(12.)
        .width(22.)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        })
}

/// The run's spine down the left of a row, and the tick that joins the row to it;
/// the last row's turns the corner instead.
pub(super) fn spine(last: bool, looks: &Drawn) -> Div {
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
fn detail(call: &Call, looks: &Drawn, cx: &mut App) -> AnyElement {
    let id = &call.tool.tool_call_id;
    let asked = looks.reading.tools.details.get(id);
    let held = asked.and_then(|asked| asked.detail.as_ref());
    let body = if call.diff.is_some() {
        held.and_then(|held| held.diff.as_ref())
            .map(|diff| diff_body(id, &diff.lines, looks).into_any_element())
    } else if call.command().is_some() || call.output.is_some() {
        let picture = asked.and_then(|asked| asked.picture.clone());
        Some(terminal(call, held, picture, looks, cx).into_any_element())
    } else {
        call.failure.map(|failure| {
            div()
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .line_height(relative(1.55))
                .text_color(looks.tints.failed)
                .child(failure.to_owned())
                .into_any_element()
        })
    };
    div()
        .ml(px(33.))
        .mt(px(1.))
        .mb(SPACE_1H)
        .children(body)
        .into_any_element()
}

/// A quiet sunken surface for what a call printed or changed, set in the mono face.
fn well(looks: &Drawn) -> Div {
    div()
        .rounded(px(8.))
        .bg(looks.tints.sunken)
        .font_family(theme::MONO)
        .text_size(crate::theme::ui_text(TEXT_XS))
}

/// What a command printed: its exit code and size, then its last lines, the rest a click away.
fn terminal(
    call: &Call,
    held: Option<&ToolDetail>,
    picture: Option<Picture>,
    looks: &Drawn,
    cx: &mut App,
) -> Div {
    let id = call.tool.tool_call_id.clone();
    let output = held.and_then(|held| held.output.as_ref());
    let whole = looks.reading.tools.whole.contains(&id);
    let text = output.map_or("", |output| output.text.trim_end_matches('\n'));
    let count = if text.is_empty() {
        0
    } else {
        text.split('\n').count()
    };
    let folds = count > OUTPUT_TAIL_LINES;
    let shown = match folds && !whole {
        true => {
            let from = text
                .rmatch_indices('\n')
                .nth(OUTPUT_TAIL_LINES - 1)
                .map_or(0, |(at, _)| at + 1);
            &text[from..]
        }
        false => text,
    };
    let exit = (call.status() == ToolStatus::Failed).then(|| {
        match call.output.and_then(|output| output.exit_code) {
            Some(code) => t!("exit {code}", code = code),
            None => t!("failed").to_owned(),
        }
    });
    let lines = (count > 0).then(|| match count {
        1 => t!("1 line").to_owned(),
        count => t!("{count} lines", count = count),
    });
    let more = folds.then(|| {
        let (reader, id, quiet, ink) = (looks.reader.clone(), id.clone(), looks.quiet, looks.ink);
        let key = id.clone();
        div()
            .id(SharedString::from(format!("more:{id}")))
            .ml_auto()
            .flex()
            .items_center()
            .gap(px(5.))
            .text_color(quiet)
            .cursor_pointer()
            .hover(move |more| more.text_color(ink))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                reader.update(cx, |reading, cx| {
                    if !reading.tools.whole.remove(&id) {
                        reading.tools.whole.insert(id.clone());
                    }
                    cx.notify();
                });
            })
            .child(chevron(format!("more-chevron:{key}"), whole, quiet))
            .child(if whole {
                t!("Show less")
            } else {
                t!("Show all")
            })
    });
    let head = div()
        .h(px(26.))
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(crate::theme::ui_text(TEXT_XS))
        .children(exit.map(|exit| {
            div()
                .px(px(7.))
                .py(px(1.))
                .rounded(px(10.))
                .bg(looks.tints.failed_ground)
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .font_weight(FontWeight::MEDIUM)
                .text_color(looks.tints.failed_ink)
                .child(exit)
        }))
        .children(lines.map(|lines| div().text_color(looks.faint).child(lines)))
        .children(more);
    let shot = picture.map(|picture| {
        div().pt(SPACE_1).child(picture::thumb(
            SharedString::from(format!("shot:{id}")),
            picture,
            px(300.),
            px(440.),
            looks.bench.clone(),
            looks.quiet,
            cx,
        ))
    });
    let printed = output.filter(|_| !shown.is_empty()).map(|_| {
        well(looks)
            .relative()
            .mt(px(4.))
            .pl(px(12.))
            .pr(px(30.))
            .py(px(10.))
            .line_height(relative(1.7))
            .text_color(looks.tints.printed)
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(crate::ui::selectable::selectable(
                        SharedString::from(format!("output:{id}")),
                        shown.to_owned(),
                        [],
                    )),
            )
            .child(div().absolute().top(px(6.)).right(px(6.)).child(copy(
                format!("copy-output:{id}"),
                t!("Copy output"),
                text.to_owned(),
            )))
    });
    let quiet_note = |words: &'static str| {
        div()
            .pt(px(4.))
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .text_color(looks.faint)
            .child(words)
    };
    let silent = output.is_some() && text.is_empty() && shot.is_none();
    let cut = output.is_some_and(|output| output.cut) && (!folds || whole);
    div()
        .flex()
        .flex_col()
        .child(head)
        .children(shot)
        .children(printed)
        .when(silent, |body| body.child(quiet_note(t!("No output"))))
        .when(cut, |body| {
            body.child(quiet_note(t!("The rest of the output was not kept")))
        })
}

/// The lines a change made, with their old and new numbers, a +/− gutter and the lines around
/// them; the unchanged rest folds into a row the reader opens.
fn diff_body(id: &str, lines: &[DiffLine], looks: &Drawn) -> impl IntoElement {
    let whole = looks.reading.tools.whole.contains(id);
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
    let scroll = looks
        .reading
        .tools
        .scrolls
        .borrow_mut()
        .entry(id.to_owned())
        .or_default()
        .clone();
    let held = scroll.clone();
    well(looks)
        .id(SharedString::from(format!("diff-body:{id}")))
        .max_h(px(DIFF_MOST_HEIGHT))
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .on_scroll(move |_, _, cx| {
            let (at, most) = (held.offset().y, held.max_offset().y);
            if most > px(0.) && at <= px(0.) && at >= -most {
                cx.stop_propagation();
            }
        })
        .flex()
        .flex_col()
        .line_height(relative(1.85))
        .children(body)
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

/// The two number columns and the sign's, before a diff line's text.
const DIFF_NUMBER_WIDTH: f32 = 34.;
const DIFF_SIGN_WIDTH: f32 = 18.;

fn diff_gap(id: &str, gap: usize, looks: &Drawn) -> AnyElement {
    let (reader, id, quiet, ink) = (looks.reader.clone(), id.to_owned(), looks.quiet, looks.ink);
    div()
        .id(SharedString::from(format!("gap:{id}:{gap}")))
        .h(px(28.))
        .pl(px(DIFF_NUMBER_WIDTH * 2. + DIFF_SIGN_WIDTH))
        .flex()
        .items_center()
        .gap(px(6.))
        .font_family(theme::UI)
        .text_color(quiet)
        .cursor_pointer()
        .hover(move |gap| gap.text_color(ink))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            reader.update(cx, |reading, cx| {
                reading.tools.whole.insert(id.clone());
                cx.notify();
            });
        })
        .child(ui::icon(Icon::Chevron, 10.))
        .child(match gap {
            1 => t!("1 more line").to_owned(),
            gap => t!("{gap} more lines", gap = gap),
        })
        .into_any_element()
}

fn diff_line(line: &DiffLine, looks: &Drawn) -> AnyElement {
    let tints = &looks.tints;
    let (ground, sign, ink, number, mark) = match line.sign.as_str() {
        "+" => (
            Some(tints.added_ground),
            tints.added,
            tints.added_ink,
            tints.added_number,
            tints.added_mark,
        ),
        "-" => (
            Some(tints.deleted_ground),
            tints.deleted_line,
            tints.deleted_ink,
            tints.deleted_number,
            tints.deleted_mark,
        ),
        _ => (
            None,
            looks.faint,
            looks.quiet,
            tints.gutter,
            tints.added_mark,
        ),
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
    let column = |at: Option<u64>| {
        div()
            .w(px(DIFF_NUMBER_WIDTH))
            .flex_none()
            .pr(SPACE_2)
            .flex()
            .justify_end()
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(number)
            .children(at.map(|n| n.to_string()))
    };
    div()
        .flex()
        .whitespace_nowrap()
        .when_some(ground, |line, ground| line.bg(ground))
        .child(column(line.old_line))
        .child(column(line.new_line))
        .child(
            div()
                .w(px(DIFF_SIGN_WIDTH))
                .flex_none()
                .flex()
                .justify_center()
                .text_color(sign)
                .when(line.sign != " ", |gutter| gutter.child(line.sign.clone())),
        )
        .child(div().min_w_0().pr(SPACE_2).text_color(ink).child(text))
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
