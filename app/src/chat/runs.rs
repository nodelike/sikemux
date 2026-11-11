//! A run of tool calls as one flat strip: a sentence saying what the run did, as the core
//! counted it, opening onto a line for each thing it did and, behind that, every call.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, Window,
};
use sikemux_view::chat::{ChatPart, RunClause, RunLine, ToolFamily, ToolIntent, ToolRun};

use super::tools::{self, Call};
use super::Drawn;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_XS,
    TEXT_XS_H,
};
use crate::ui;
use crate::ui::shimmer::shimmer;
use crate::ui::time;
use gpui_transcript::{FileKind, FilePress};

/// The most things a merged line names before it counts the rest.
const LINE_NAMES: usize = 4;
/// The most clauses a strip's sentence says before it counts the rest.
const SENTENCE_CLAUSES: usize = 4;
/// The most pictures a line shows of those its calls looked at.
const LINE_PICTURES: usize = 6;

pub fn strip(
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
    let counted = match first {
        ChatPart::Tool { run: Some(run), .. } => std::borrow::Cow::Borrowed(&**run),
        _ => std::borrow::Cow::Owned(uncounted(&calls)),
    };
    let run: &ToolRun = &counted;
    let current = calls.iter().find(|call| call.running());
    if run.said.is_empty() && current.is_none() {
        return div().into_any_element();
    }
    let key = first.id().to_owned();
    let t = Tokens::get(cx).clone();
    let open = looks
        .reading
        .tools
        .runs
        .get(&key)
        .copied()
        .unwrap_or(live || current.is_some());
    let spent: u64 = calls.iter().map(Call::took).sum();
    let time = match current {
        Some(call) => Some(tools::running_for(
            format!("run-seconds:{key}"),
            call.started.unwrap_or_else(time::now_ms),
            spent,
        )),
        None => (spent > 0).then(|| time::took(spent).into_any_element()),
    };
    let time = time.map(|time| tools::settled(format!("run-time:{key}"), current.is_some(), time));
    let lead = run.lines.first().and_then(|line| line_call(line, &calls));
    let lead_icon = match (lead, run.said.first()) {
        (Some(call), _) if !call.failed() => {
            line_icon(call.tool.row.intent, call.tool.row.family, Some(call))
        }
        (_, Some(clause)) => line_icon(clause.intent, clause.family, None),
        _ => Icon::Command,
    };
    let lead_tint = run
        .said
        .first()
        .map_or(looks.quiet, |clause| tint(clause.intent, looks));
    let said = sentence(run, current.map(|call| &*call.tool.row));
    let (adds, dels) = calls
        .iter()
        .filter_map(|call| call.diff)
        .fold((0, 0), |(adds, dels), diff| {
            (adds + diff.adds, dels + diff.dels)
        });
    let (reader, hover) = (looks.reader.clone(), looks.hover);
    let words = if current.is_some() {
        div()
            .min_w_0()
            .flex()
            .child(shimmer(format!("run:{key}:said"), said))
    } else {
        div().min_w_0().truncate().child(said)
    };
    let head = div()
        .id(SharedString::from(format!("run:{key}")))
        .h(px(30.))
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_3)
        .cursor_pointer()
        .hover(move |head| head.bg(hover))
        .child(ui::picture(lead_icon, 13., lead_tint))
        .child(words)
        .when(adds + dels > 0, |head| {
            head.child(
                div()
                    .flex_none()
                    .text_color(looks.tints.added)
                    .child(format!("+{adds}")),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(looks.tints.deleted_line)
                    .child(format!("−{dels}")),
            )
        })
        .when(run.failed > 0, |head| {
            head.child(
                div()
                    .flex_none()
                    .text_color(looks.tints.failed)
                    .child(format!("{} failed", run.failed)),
            )
        })
        .child(div().flex_1())
        .children(time.map(|time| div().flex_none().text_color(looks.faint).child(time)))
        .child(tools::chevron(
            format!("run-chevron:{key}"),
            open,
            looks.quiet,
        ))
        .on_click({
            let key = key.clone();
            move |_, _, cx| {
                reader.update(cx, |reading, cx| {
                    reading.tools.runs.insert(key.clone(), !open);
                    cx.notify();
                })
            }
        });
    let shown = motion::follow(
        ElementId::Name(format!("run-fold:{key}").into()),
        if open { 1. } else { 0. },
        if open {
            motion::UNFOLD
        } else {
            motion::FOLD_AWAY
        },
        window,
        cx,
    );
    let body = (shown > 0.).then(|| {
        let raw_key = format!("raw:{key}");
        let raw_open = looks
            .reading
            .tools
            .runs
            .get(&raw_key)
            .copied()
            .unwrap_or(false);
        let lines = run
            .lines
            .iter()
            .map(|line| run_line(line, &calls, looks, window, cx))
            .collect::<Vec<_>>();
        let total: u64 = calls.iter().map(|call| call.tool.row.calls()).sum();
        let reader = looks.reader.clone();
        let raw_toggle = div()
            .id(SharedString::from(raw_key.clone()))
            .mt(SPACE_1)
            .h(px(22.))
            .flex()
            .items_center()
            .gap(SPACE_1)
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(looks.quiet)
            .cursor_pointer()
            .hover(|toggle| toggle.text_color(t.text_secondary))
            .child(format!(
                "All {total} {}",
                if total == 1 { "call" } else { "calls" }
            ))
            .child(tools::chevron(
                format!("raw-chevron:{key}"),
                raw_open,
                looks.quiet,
            ))
            .when(run.hidden > 0, |toggle| {
                toggle.child(
                    div()
                        .ml(SPACE_2)
                        .text_color(looks.faint)
                        .child(format!("+{} housekeeping", run.hidden)),
                )
            })
            .on_click(move |_, _, cx| {
                reader.update(cx, |reading, cx| {
                    reading.tools.runs.insert(raw_key.clone(), !raw_open);
                    cx.notify();
                })
            });
        let lines_say_all = run.hidden == 0
            && run.lines.len() == calls.len()
            && calls.iter().all(|call| call.tool.row.calls() == 1);
        let raw = raw_open.then(|| tools::rows(&calls, looks, window, cx));
        let body = div()
            .px(SPACE_3)
            .pb(SPACE_2H)
            .flex()
            .flex_col()
            .children(lines)
            .when(!lines_say_all, |body| {
                body.child(div().flex().child(raw_toggle))
            })
            .children(raw);
        motion::folded(
            ElementId::Name(format!("run-body:{key}").into()),
            shown,
            body,
            window,
            cx,
        )
    });
    let strip = div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(theme::RADIUS_3)
        .bg(strip_ground(&t))
        .font_family(theme::UI)
        .text_size(theme::ui_text(TEXT_XS_H))
        .text_color(t.text_secondary)
        .child(head)
        .children(body);
    let (arrives, _) = tools::arrivals(&key, &calls, live, window, cx);
    if !arrives {
        return strip.into_any_element();
    }
    let key = SharedString::from(format!("run-arrive:{key}"));
    motion::arriving(key, motion::ROW_ARRIVAL, strip).into_any_element()
}

fn strip_ground(t: &Tokens) -> Hsla {
    Hsla {
        a: 0.55,
        ..t.surface_raised
    }
}

/// A run the core did not count, as when only part of it is in view: its calls, one a line.
fn uncounted(calls: &[Call]) -> ToolRun {
    let mut said: Vec<RunClause> = Vec::new();
    for call in calls {
        let row = &call.tool.row;
        match said
            .iter_mut()
            .find(|clause| clause.intent == row.intent && clause.family == row.family)
        {
            Some(clause) => clause.count += row.calls(),
            None => said.push(RunClause {
                intent: row.intent,
                family: row.family,
                count: row.calls(),
                names: Vec::new(),
                took: None,
            }),
        }
    }
    ToolRun {
        lines: calls
            .iter()
            .zip(0..)
            .map(|(call, at)| RunLine {
                intent: call.tool.row.intent,
                family: call.tool.row.family,
                calls: vec![at],
                failed: call.failed(),
            })
            .collect(),
        failed: calls.iter().filter(|call| call.failed()).count() as u64,
        said,
        ..ToolRun::default()
    }
}

fn line_call<'a>(line: &RunLine, calls: &'a [Call<'a>]) -> Option<&'a Call<'a>> {
    line.calls.first().and_then(|at| calls.get(*at as usize))
}

/// One thing the run did: a command, a change, or neighbours that did the same thing named
/// together. A line of one call opens onto what it changed or printed.
fn run_line(
    line: &RunLine,
    run: &[Call],
    looks: &Drawn,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let calls: Vec<&Call> = line
        .calls
        .iter()
        .filter_map(|at| run.get(*at as usize))
        .collect();
    let Some(&first) = calls.first() else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx);
    let running = calls.iter().any(|call| call.running());
    let tint = if line.failed {
        looks.tints.failed
    } else {
        tint(line.intent, looks)
    };
    let icon = line_icon(line.intent, line.family, Some(first));
    let said_by_intent = matches!(
        line.intent,
        ToolIntent::Read | ToolIntent::View | ToolIntent::Search | ToolIntent::List
    ) && first.tool.row.subject.is_some();
    let command = calls.len() == 1 && first.command().is_some() && !said_by_intent;
    let main = if command {
        let typed = first.tool.row.target.clone();
        div()
            .min_w_0()
            .truncate()
            .font_family(theme::MONO)
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(if line.failed {
                looks.tints.failed
            } else {
                t.text_secondary
            })
            .child(if running {
                shimmer(format!("line:{}", first.tool.tool_call_id), typed).into_any_element()
            } else {
                typed.into_any_element()
            })
    } else {
        let mut names: Vec<(String, Option<&str>)> = Vec::new();
        for call in &calls {
            let name = named(call);
            if !name.is_empty() && !names.iter().any(|(held, _)| *held == name) {
                names.push((name, call.tool.row.path.as_deref()));
            }
        }
        let more = names.len().saturating_sub(LINE_NAMES);
        names.truncate(LINE_NAMES);
        let (verb, _, _) = words(&RunClause {
            intent: line.intent,
            family: line.family,
            count: calls.len() as u64,
            names: Vec::new(),
            took: None,
        });
        let waited = line.intent == ToolIntent::Wait && !names.is_empty();
        let chips: Vec<AnyElement> = names
            .into_iter()
            .map(|(name, path)| match path {
                Some(path) => chip(&name, path, line.intent != ToolIntent::View, looks, cx),
                None => div()
                    .flex_none()
                    .font_family(theme::MONO)
                    .text_size(theme::ui_text(TEXT_XS))
                    .text_color(looks.ink)
                    .child(name)
                    .into_any_element(),
            })
            .collect();
        div()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .child(
                div()
                    .flex_none()
                    .text_color(t.text_secondary)
                    .child(if waited { "Waited for" } else { verb }),
            )
            .children(chips)
            .children(first.tool.row.within.clone().map(|within| {
                div()
                    .flex_none()
                    .flex()
                    .gap(SPACE_1)
                    .child(div().text_color(looks.quiet).child("in"))
                    .child(
                        div()
                            .font_family(theme::MONO)
                            .text_size(theme::ui_text(TEXT_XS))
                            .text_color(t.text_secondary)
                            .child(within),
                    )
            }))
            .when(more > 0, |main| {
                main.child(
                    div()
                        .flex_none()
                        .text_color(looks.quiet)
                        .child(format!("+{more}")),
                )
            })
    };
    let took: u64 = calls.iter().map(|call| call.took()).sum();
    let diff = (calls.len() == 1).then_some(first.diff).flatten();
    let right = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .text_color(looks.faint)
        .when_some(diff, |right, diff| {
            right
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
        })
        .when(calls.len() > 1, |right| {
            right.child(format!("{} calls", calls.len()))
        })
        .when(command && !running, |right| {
            let (glyph, mark) = if line.failed {
                (Icon::Close, looks.tints.failed)
            } else {
                (Icon::Check, looks.tints.added)
            };
            right.child(ui::icon(glyph, 11.).text_color(mark))
        })
        .when(running, |right| {
            right.children(first.started.map(|started| {
                tools::running_for(
                    format!("line-seconds:{}", first.tool.tool_call_id),
                    started,
                    0,
                )
            }))
        })
        .when(!running && took > 0, |right| right.child(time::took(took)));
    let lone = (calls.len() == 1).then_some(first);
    let opens = lone.is_some_and(|call| call.opens());
    let id = first.tool.tool_call_id.clone();
    let open = opens && looks.reading.tools.calls.contains(&id);
    let head = div()
        .id(SharedString::from(format!("line:{id}")))
        .min_h(px(26.))
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .flex()
                .justify_center()
                .child(ui::picture(icon, 12., tint)),
        )
        .child(div().flex_1().min_w_0().flex().child(main))
        .child(right)
        .when(opens, |head| {
            head.cursor_pointer()
                .on_click(tools::toggle_detail(first, looks))
        });
    let failure = line
        .failed
        .then_some(first.failure)
        .flatten()
        .filter(|_| !open)
        .map(|failure| {
            div()
                .ml(px(24.))
                .mb(SPACE_1)
                .px(SPACE_2H)
                .py(SPACE_1)
                .rounded(theme::RADIUS_2)
                .bg(Hsla {
                    a: 0.08,
                    ..looks.tints.failed
                })
                .font_family(theme::MONO)
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(looks.tints.failed)
                .line_clamp(3)
                .child(failure.to_owned())
        });
    let pictures: Vec<AnyElement> = calls
        .iter()
        .filter(|call| call.tool.row.intent == ToolIntent::View)
        .filter_map(|call| call.tool.row.path.as_deref())
        .map(without_lines)
        .filter(|path| std::path::Path::new(path).is_file())
        .take(LINE_PICTURES)
        .map(|path| thumbnail(path, looks))
        .collect();
    let pictures = (!pictures.is_empty()).then(|| {
        div()
            .ml(px(24.))
            .mb(SPACE_1)
            .flex()
            .flex_wrap()
            .gap(SPACE_2H)
            .children(pictures)
    });
    let detail = lone
        .filter(|_| opens)
        .and_then(|call| tools::opened_detail(call, open, looks, window, cx));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .child(head)
        .children(failure)
        .children(pictures)
        .children(detail)
        .into_any_element()
}

/// A file a line names: its icon, its name and the lines it touched, opening the file.
fn chip(name: &str, path: &str, with_lines: bool, looks: &Drawn, cx: &App) -> AnyElement {
    let lines = path
        .rsplit_once(':')
        .filter(|_| with_lines)
        .map(|(_, lines)| lines)
        .filter(|lines| {
            lines.contains('-') && lines.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        })
        .map(|lines| lines.replace('-', "–"));
    let file = without_lines(path).to_owned();
    let (agent, files, bench) = (
        looks.agent.clone(),
        looks.files.clone(),
        looks.bench.clone(),
    );
    let t = Tokens::get(cx);
    div()
        .id(SharedString::from(format!("chip:{path}")))
        .flex_none()
        .h(px(22.))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded(theme::RADIUS_2)
        .bg(Hsla {
            a: 0.06,
            ..t.text_primary
        })
        .cursor_pointer()
        .hover(|chip| {
            chip.bg(Hsla {
                a: 0.1,
                ..t.text_primary
            })
        })
        .child(crate::file_icons::glyph(name, 12., cx))
        .child(
            div()
                .font_family(theme::MONO)
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(looks.ink)
                .child(name.to_owned()),
        )
        .children(lines.map(|lines| {
            div()
                .text_size(theme::ui_text(TEXT_2XS))
                .text_color(looks.quiet)
                .child(lines)
        }))
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            let press = if event.click_count() > 1 {
                FilePress::OpenTwice
            } else {
                FilePress::Open
            };
            super::files::pressed(
                &agent,
                &files,
                &bench,
                (&file, FileKind::File, press),
                window,
                cx,
            );
        })
        .into_any_element()
}

/// A picture the agent looked at, small, on this host's disk.
fn thumbnail(path: &str, looks: &Drawn) -> AnyElement {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(
            gpui_kit::img(std::path::PathBuf::from(path))
                .w(px(104.))
                .h(px(70.))
                .rounded(theme::RADIUS_2)
                .object_fit(gpui_kit::ObjectFit::Cover),
        )
        .child(
            div()
                .w(px(104.))
                .truncate()
                .font_family(theme::MONO)
                .text_size(theme::ui_text(TEXT_2XS))
                .text_color(looks.quiet)
                .child(name),
        )
        .into_any_element()
}

/// `path` without the `:line` or `:first-last` a call's place adds.
fn without_lines(path: &str) -> &str {
    match path.rsplit_once(':') {
        Some((file, lines))
            if !lines.is_empty() && lines.bytes().all(|b| b.is_ascii_digit() || b == b'-') =>
        {
            file
        }
        _ => path,
    }
}

/// What a line names a call by: the file or pattern it touched, else its target.
fn named(call: &Call) -> String {
    let row = &call.tool.row;
    match (&row.subject, row.family) {
        (Some(name), ToolFamily::Agents | ToolFamily::Waiting) if !name.starts_with('@') => {
            format!("@{name}")
        }
        (Some(name), _) => name.clone(),
        (None, _) => row.target.clone(),
    }
}

fn tint(intent: ToolIntent, looks: &Drawn) -> Hsla {
    use ToolIntent as I;
    match intent {
        I::Read
        | I::View
        | I::Search
        | I::List
        | I::Fetch
        | I::Open
        | I::Look
        | I::Click
        | I::Type => looks.tints.looked,
        I::Edit | I::Write | I::Move | I::Commit => looks.tints.changed,
        I::Delete => looks.tints.deleted,
        I::Run | I::Test | I::Build | I::Git | I::Start | I::Stop => looks.tints.ran,
        I::Message => looks.tints.called,
        I::Wait | I::Query | I::Use | I::Mode => looks.quiet,
    }
}

fn line_icon(intent: ToolIntent, family: ToolFamily, call: Option<&Call>) -> Icon {
    use ToolIntent as I;
    if let Some(mark) = call.and_then(|call| tools::plugin_mark(&call.tool.row)) {
        return mark;
    }
    let simulator = family == ToolFamily::Simulator;
    match intent {
        I::Read if family == ToolFamily::Browser => Icon::Eye,
        I::Read | I::Fetch if family == ToolFamily::Web => Icon::Globe,
        I::Read => Icon::File,
        I::View => Icon::Image,
        I::Search if family == ToolFamily::Web => Icon::Globe,
        I::Search => Icon::Search,
        I::List => Icon::Folder,
        I::Edit | I::Write => Icon::Pencil,
        I::Move => Icon::Move,
        I::Delete => Icon::Trash,
        I::Run | I::Test | I::Build => Icon::Command,
        I::Git => Icon::Git,
        I::Commit => Icon::Commit,
        I::Open if simulator => Icon::Phone,
        I::Open | I::Fetch => Icon::Globe,
        I::Click if simulator => Icon::Tap,
        I::Click => Icon::Click,
        I::Type => Icon::Keyboard,
        I::Look => Icon::Camera,
        I::Message => Icon::Message,
        I::Start if family == ToolFamily::Shell => Icon::Run,
        I::Start => Icon::Agent,
        I::Stop => Icon::Stop,
        I::Wait => Icon::Clock,
        I::Mode => Icon::Settings,
        I::Query | I::Use => Icon::Plug,
    }
}

/// The run's sentence, as the app says it: each thing it did, counted, in the order it first
/// did it. A run that only looked says it explored; a running one says what it is doing.
pub(super) fn sentence(run: &ToolRun, running: Option<&sikemux_view::chat::ToolRow>) -> String {
    let doing = running.and_then(|row| {
        run.said.iter().position(|clause| {
            clause.intent == row.intent
                && (clause.family == row.family
                    || clause.family == ToolFamily::Files && row.family == ToolFamily::Shell)
        })
    });
    let order = doing
        .into_iter()
        .chain((0..run.said.len()).filter(|at| Some(*at) != doing));
    let mut clauses: Vec<String> = order
        .take(SENTENCE_CLAUSES)
        .enumerate()
        .map(|(place, at)| {
            let (past, now, rest) = words(&run.said[at]);
            let verb = if Some(at) == doing { now } else { past };
            let said = if rest.is_empty() {
                verb.to_owned()
            } else {
                format!("{verb} {rest}")
            };
            if place == 0 && !run.explored {
                said
            } else {
                lower_first(&said)
            }
        })
        .collect();
    let more = run.said.len().saturating_sub(SENTENCE_CLAUSES);
    if more > 0 {
        clauses.push(format!("{more} more"));
    }
    let said = clauses.join(", ");
    match (run.explored, doing.is_some()) {
        (true, false) => format!("Explored: {said}"),
        (true, true) => format!("Exploring: {said}"),
        (false, _) => said,
    }
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn times(n: u64) -> &'static str {
    match n {
        0 | 1 => "",
        2 => "twice",
        _ => "",
    }
}

fn often(n: u64) -> String {
    match n {
        0 | 1 => String::new(),
        _ if !times(n).is_empty() => times(n).to_owned(),
        n => format!("{n} times"),
    }
}

fn with_often(what: &str, n: u64) -> String {
    let often = often(n);
    if often.is_empty() {
        what.to_owned()
    } else if what.is_empty() {
        often
    } else {
        format!("{what} {often}")
    }
}

fn names_or(clause: &RunClause, one: &str, many: &str) -> String {
    if clause.names.is_empty() {
        return count(clause.count, one, many);
    }
    let names: Vec<String> = clause
        .names
        .iter()
        .map(|name| {
            if name.starts_with('@') {
                name.clone()
            } else {
                format!("@{name}")
            }
        })
        .collect();
    let extra = (clause.count as usize).saturating_sub(names.len());
    match names.as_slice() {
        [one] if extra == 0 => one.clone(),
        [a, b] if extra == 0 => format!("{a} and {b}"),
        _ if clause.count > names.len() as u64 && clause.names.len() >= 3 => {
            count(clause.count, one, many)
        }
        _ => names.join(", "),
    }
}

fn service(clause: &RunClause) -> String {
    clause
        .names
        .first()
        .map(|name| {
            let mut chars = name.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// A clause as the app words it: its verb done and doing, and what it was done to.
fn words(clause: &RunClause) -> (&'static str, &'static str, String) {
    use ToolFamily as F;
    use ToolIntent as I;
    let n = clause.count;
    match (clause.intent, clause.family) {
        (I::Read, F::Browser) => ("Read", "Reading", with_often("the page", n)),
        (I::Read, F::Simulator) => ("Read", "Reading", with_often("the app", n)),
        (I::Read, F::Agents) => ("Read", "Reading", names_or(clause, "agent", "agents")),
        (I::Read, _) => ("Read", "Reading", count(n, "file", "files")),
        (I::View, _) => ("Viewed", "Viewing", count(n, "image", "images")),
        (I::Search, F::Web) => ("Searched", "Searching", with_often("the web", n)),
        (I::Search, _) => ("Searched", "Searching", often(n)),
        (I::List, _) => ("Listed", "Listing", count(n, "folder", "folders")),
        (I::Edit, _) => ("Edited", "Editing", count(n, "file", "files")),
        (I::Write, _) => ("Wrote", "Writing", count(n, "file", "files")),
        (I::Move, _) => ("Moved", "Moving", count(n, "file", "files")),
        (I::Delete, _) => ("Deleted", "Deleting", count(n, "file", "files")),
        (I::Run, _) => ("Ran", "Running", count(n, "command", "commands")),
        (I::Test, _) => ("Ran", "Running", with_often("tests", n)),
        (I::Build, _) => ("Ran", "Running", count(n, "build", "builds")),
        (I::Git, _) => ("Ran", "Running", count(n, "git command", "git commands")),
        (I::Commit, _) => ("Committed", "Committing", count(n, "change", "changes")),
        (I::Open, F::Browser) => ("Opened", "Opening", count(n, "page", "pages")),
        (I::Open, _) => ("Opened", "Opening", with_often("the app", n)),
        (I::Click, F::Simulator) => ("Tapped", "Tapping", often(n)),
        (I::Click, _) => ("Clicked", "Clicking", often(n)),
        (I::Type, _) => ("Typed", "Typing", often(n)),
        (I::Look, _) => ("Took", "Taking", count(n, "screenshot", "screenshots")),
        (I::Message, _) => ("Messaged", "Messaging", names_or(clause, "agent", "agents")),
        (I::Start, F::Agents) => ("Started", "Starting", names_or(clause, "agent", "agents")),
        (I::Start, _) => ("Started", "Starting", count(n, "process", "processes")),
        (I::Stop, F::Agents) => ("Closed", "Closing", names_or(clause, "agent", "agents")),
        (I::Stop, _) => ("Stopped", "Stopping", count(n, "process", "processes")),
        (I::Wait, _) => {
            let took = clause.took.filter(|took| *took >= 1000).map(time::took);
            let whom = (!clause.names.is_empty()).then(|| names_or(clause, "agent", "agents"));
            let rest = match (took, whom) {
                (Some(took), Some(whom)) => format!("{took} for {whom}"),
                (Some(took), None) => took,
                (None, Some(whom)) => format!("for {whom}"),
                (None, None) => String::new(),
            };
            ("Waited", "Waiting", rest)
        }
        (I::Query, _) => ("Asked", "Asking", with_often(&service(clause), n)),
        (I::Fetch, _) => ("Read", "Reading", count(n, "page", "pages")),
        (I::Mode, _) => ("Switched mode", "Switching mode", String::new()),
        (I::Use, _) if !clause.names.is_empty() => {
            ("Used", "Using", with_often(&service(clause), n))
        }
        (I::Use, _) => ("Used", "Using", count(n, "tool", "tools")),
    }
}
