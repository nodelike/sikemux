//! A job's log, step by step: each step a card that folds, its lines in the colours the
//! job printed them in, with search across the whole log.

use std::collections::HashSet;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, list, px, AnyElement, App, AppContext, Context, Entity, FontWeight, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, ListAlignment, ListState, ParentElement, Rgba, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Subscription, Window,
};
use serde_json::Value;
use sikemux_plugin_api::codehost::{Job, JobLogLine, LogColor, LogMark, OpenJobLog, Outcome, Step};

use super::{elapsed, notice, outcome_word, HostArea, Read, Said};
use crate::icons::Icon;
use crate::theme::{self, Tokens, RADIUS_2, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS_H, TEXT_XS};
use crate::ui::{self, Button, IconButton, Variant};

pub const KEY: &str = "host-log";
const LINE: f32 = 18.;

#[derive(Clone, Copy, PartialEq)]
enum Item {
    Header(usize),
    Line(usize),
    Empty(usize),
}

/// One step's stretch of the log: lines `from` up to `to`.
struct Section {
    key: String,
    step: Option<Step>,
    from: usize,
    to: usize,
}

pub struct Log {
    /// The job whose log it is.
    pub job: String,
    log: Read<OpenJobLog>,
    sections: Vec<Section>,
    opened: HashSet<String>,
    /// How each step last stood, so a step opens when it fails or starts.
    seen: Vec<(String, Outcome)>,
    brought_failure: bool,
    find: Option<(Entity<InputState>, Subscription)>,
    needle: String,
    matches: Vec<usize>,
    current: usize,
    rows: ListState,
    items: Vec<Item>,
}

impl Log {
    pub fn new(job: String) -> Self {
        Self {
            job,
            log: None,
            sections: Vec::new(),
            opened: HashSet::new(),
            seen: Vec::new(),
            brought_failure: false,
            find: None,
            needle: String::new(),
            matches: Vec::new(),
            current: 0,
            rows: ListState::new(0, ListAlignment::Top, px(800.)),
            items: Vec::new(),
        }
    }

    pub fn take(&mut self, value: &Value, job: &Job) {
        let log: Result<OpenJobLog, SharedString> = super::answer(value);
        if let Ok(read) = &log {
            self.sections = read
                .sections
                .iter()
                .map(|section| Section {
                    key: section
                        .step
                        .map_or_else(|| "log".to_owned(), |step| format!("step:{step}")),
                    step: section.step.and_then(|number| {
                        job.steps.iter().find(|step| step.number == number).cloned()
                    }),
                    from: section.from,
                    to: section.to,
                })
                .collect();
        }
        self.log = Some(log);
        self.open_moved();
        self.search();
        self.lay_out();
        let running = job.status != "completed";
        if running && self.needle.is_empty() && !self.items.is_empty() {
            self.rows.scroll_to_reveal_item(self.items.len() - 1);
        }
    }

    /// A step that failed opens, and the first to fail comes into view; a running
    /// step, or a job's log without steps, opens too. A step that passed stays folded.
    fn open_moved(&mut self) {
        let mut first_failed = None;
        for section in &self.sections {
            let outcome = section
                .step
                .as_ref()
                .map_or(Outcome::Running, |step| step.outcome);
            if self
                .seen
                .iter()
                .any(|(key, was)| key == &section.key && *was == outcome)
            {
                continue;
            }
            self.seen.retain(|(key, _)| key != &section.key);
            self.seen.push((section.key.clone(), outcome));
            match outcome {
                Outcome::Failure => {
                    self.opened.insert(section.key.clone());
                    first_failed.get_or_insert(section.from);
                }
                Outcome::Running => {
                    self.opened.insert(section.key.clone());
                }
                _ => {}
            }
        }
        if let (Some(line), false) = (first_failed, self.brought_failure) {
            self.brought_failure = true;
            self.lay_out();
            self.show(line);
        }
    }

    fn lines(&self) -> &[JobLogLine] {
        match &self.log {
            Some(Ok(log)) => &log.lines,
            _ => &[],
        }
    }

    fn lay_out(&mut self) {
        let mut items = Vec::new();
        for (at, section) in self.sections.iter().enumerate() {
            items.push(Item::Header(at));
            if !self.opened.contains(&section.key) {
                continue;
            }
            if section.to == section.from {
                items.push(Item::Empty(at));
            }
            items.extend((section.from..section.to).map(Item::Line));
        }
        crate::git::relist(&self.rows, &self.items, &items);
        self.items = items;
    }

    /// Opens the step `line` is in and brings the line into view.
    fn show(&mut self, line: usize) {
        if let Some(section) = self
            .sections
            .iter()
            .find(|section| line >= section.from && line < section.to)
        {
            if self.opened.insert(section.key.clone()) {
                self.lay_out();
            }
        }
        if let Some(item) = self.items.iter().position(|item| *item == Item::Line(line)) {
            self.rows.scroll_to_reveal_item(item);
        }
    }

    fn search(&mut self) {
        self.matches = if self.needle.is_empty() {
            Vec::new()
        } else {
            self.lines()
                .iter()
                .enumerate()
                .filter(|(_, line)| line.text.to_lowercase().contains(&self.needle))
                .map(|(at, _)| at)
                .collect()
        };
        self.current = self.current.min(self.matches.len().saturating_sub(1));
    }

    fn go(&mut self, step: isize) {
        if self.matches.is_empty() {
            return;
        }
        let count = self.matches.len() as isize;
        self.current = ((self.current as isize + step).rem_euclid(count)) as usize;
        self.show(self.matches[self.current]);
    }
}

impl HostArea {
    pub(super) fn log_mut(&mut self) -> Option<&mut Log> {
        self.runs.open.as_mut()?.log.as_mut()
    }

    fn find_typed(&mut self, cx: &mut Context<Self>) {
        let Some(log) = self.log_mut() else {
            return;
        };
        let Some((find, _)) = &log.find else {
            return;
        };
        log.needle = find.read(cx).value().trim().to_lowercase();
        log.current = 0;
        log.search();
        if let Some(first) = log.matches.first().copied() {
            log.show(first);
        }
        cx.notify();
    }
}

/// The colour a log span names, from the terminal's palette.
fn ink(color: &LogColor, t: &Tokens) -> Hsla {
    let [r, g, b] = match color {
        LogColor::Palette { index } => t.terminal.color(usize::from(*index)),
        LogColor::Rgb { hex } => {
            let value = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
            [(value >> 16) as u8, (value >> 8) as u8, value as u8]
        }
    };
    Rgba {
        r: f32::from(r) / 255.,
        g: f32::from(g) / 255.,
        b: f32::from(b) / 255.,
        a: 1.,
    }
    .into()
}

fn line_text(line: &JobLogLine, t: &Tokens) -> StyledText {
    if line.spans.is_empty() {
        let text = if line.text.is_empty() {
            " ".to_owned()
        } else {
            line.text.clone()
        };
        return StyledText::new(text);
    }
    let mut text = String::new();
    let mut styles = Vec::new();
    for span in &line.spans {
        let start = text.len();
        text.push_str(&span.text);
        let style = HighlightStyle {
            color: span.fg.as_ref().map(|fg| {
                let mut fg = ink(fg, t);
                if span.dim {
                    fg.a *= 0.65;
                }
                fg
            }),
            background_color: span.bg.as_ref().map(|bg| ink(bg, t)),
            font_weight: span.bold.then_some(FontWeight::SEMIBOLD),
            font_style: span.italic.then_some(gpui_kit::FontStyle::Italic),
            underline: span.underline.then(|| gpui_kit::UnderlineStyle {
                thickness: px(1.),
                ..Default::default()
            }),
            ..Default::default()
        };
        styles.push((start..text.len(), style));
    }
    StyledText::new(text).with_highlights(styles)
}

/// Makes the log's search field the first time the log shows, which needs the window.
pub fn prepare(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) {
    let Some(log) = area.log_mut().filter(|log| log.find.is_none()) else {
        return;
    };
    let field = cx.new(|cx| InputState::new(window, cx).placeholder("Search the log"));
    let typing = cx.subscribe_in(
        &field,
        window,
        |area, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => area.find_typed(cx),
            InputEvent::PressEnter { shift, .. } => {
                if let Some(log) = area.log_mut() {
                    log.go(if *shift { -1 } else { 1 });
                }
                cx.notify();
            }
            _ => {}
        },
    );
    log.find = Some((field, typing));
}

pub fn view(area: &HostArea, job: &Job, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let running = job.status != "completed";
    let Some(log) = area
        .runs
        .open
        .as_ref()
        .and_then(|opened| opened.log.as_ref())
    else {
        return notice("Loading log", None, cx);
    };
    let read = match &log.log {
        None => return notice("Loading log", None, cx),
        Some(Err(error)) => return notice("Could not read the log", Some(error.clone()), cx),
        Some(Ok(read)) => read,
    };
    if read.expired {
        return notice(
            "The log is gone",
            Some("The host keeps job logs for a limited time, and this one has aged out.".into()),
            cx,
        );
    }
    if read.lines.is_empty() {
        let said = if running {
            "This job has not written anything yet."
        } else {
            "This job wrote no log."
        };
        return notice(said, None, cx);
    }
    let count = read.lines.len();
    let Some(field) = log.find.as_ref().map(|(field, _)| field.clone()) else {
        return notice("Loading log", None, cx);
    };
    let found = (!log.needle.is_empty()).then(|| {
        if log.matches.is_empty() {
            "no matches".to_owned()
        } else {
            format!("{} of {}", log.current + 1, log.matches.len())
        }
    });
    let many = log.matches.len() > 1;
    let all_open = log
        .sections
        .iter()
        .all(|section| log.opened.contains(&section.key));
    let head = div()
        .px(SPACE_3)
        .py(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .child(ui::icon(Icon::Search, 13.).text_color(t.text_tertiary))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(Input::new(&field).appearance(false)),
        )
        .children(found.map(|found| ui::meta(found, cx)))
        .when(many, |head| {
            head.child(
                IconButton::new("log-previous", Icon::ArrowUp, "Previous match")
                    .small()
                    .on_click(cx.listener(|area, _, _, cx| {
                        if let Some(log) = area.log_mut() {
                            log.go(-1);
                        }
                        cx.notify();
                    })),
            )
            .child(
                IconButton::new("log-next", Icon::ArrowDown, "Next match")
                    .small()
                    .on_click(cx.listener(|area, _, _, cx| {
                        if let Some(log) = area.log_mut() {
                            log.go(1);
                        }
                        cx.notify();
                    })),
            )
        })
        .child(ui::meta(
            format!(
                "{count} line{}{}",
                if count == 1 { "" } else { "s" },
                if running { " so far" } else { "" }
            ),
            cx,
        ))
        .child(
            Button::new(
                "log-fold",
                if all_open {
                    "Collapse all"
                } else {
                    "Expand all"
                },
                Variant::Outline,
            )
            .icon(if all_open {
                Icon::CollapseAll
            } else {
                Icon::ExpandAll
            })
            .on_click(cx.listener(move |area, _, _, cx| {
                if let Some(log) = area.log_mut() {
                    if all_open {
                        log.opened.clear();
                    } else {
                        log.opened = log
                            .sections
                            .iter()
                            .map(|section| section.key.clone())
                            .collect();
                    }
                    log.lay_out();
                }
                cx.notify();
            })),
        )
        .child(
            Button::new("log-copy", "Copy", Variant::Outline)
                .icon(Icon::Copy)
                .on_click(cx.listener(|area, _, _, cx| {
                    let Some(log) = area.log_mut() else {
                        return;
                    };
                    let lines = log.lines();
                    let text = lines
                        .iter()
                        .map(|line| line.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let count = lines.len();
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    area.say(Said::Done(format!("Copied {count} lines").into()), cx);
                })),
        );
    let view = cx.entity();
    let job_name = job.name.clone();
    let rows = list(log.rows.clone(), move |index, _, cx| {
        let cx = &*cx;
        let area = view.read(cx);
        let Some(log) = area
            .runs
            .open
            .as_ref()
            .and_then(|opened| opened.log.as_ref())
        else {
            return div().into_any_element();
        };
        match log.items.get(index).copied() {
            Some(item) => item_of(&view, log, item, &job_name, cx),
            None => div().into_any_element(),
        }
    })
    .flex_1()
    .min_h_0();
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(head)
        .child(rows)
        .into_any_element()
}

fn item_of(view: &Entity<HostArea>, log: &Log, item: Item, job: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    match item {
        Item::Header(at) => {
            let section = &log.sections[at];
            let open = log.opened.contains(&section.key);
            let key = section.key.clone();
            let (title, outcome, took) = match &section.step {
                Some(step) => (
                    step.name.clone(),
                    Some(step.outcome),
                    step.started_at
                        .as_deref()
                        .filter(|_| step.outcome != Outcome::Skipped)
                        .map(|started| elapsed(started, step.completed_at.as_deref())),
                ),
                None => (job.to_owned(), None, None),
            };
            let folding = view.clone();
            div()
                .id(SharedString::from(format!("log-step-{}", section.key)))
                .mx(SPACE_3)
                .mt(SPACE_2)
                .h(px(30.))
                .px(SPACE_2H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .bg(t.surface_raised)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    let key = key.clone();
                    folding.update(cx, |area, cx| {
                        if let Some(log) = area.log_mut() {
                            if !log.opened.remove(&key) {
                                log.opened.insert(key);
                            }
                            log.lay_out();
                        }
                        cx.notify();
                    })
                })
                .child(crate::motion::turn(
                    format!("log-chevron:{}", section.key),
                    crate::motion::fold_angle(open),
                    crate::motion::LIST_FOLD,
                    ui::icon(Icon::Chevron, 11.).text_color(t.text_tertiary),
                ))
                .children(outcome.map(|outcome| {
                    ui::run_mark(
                        format!("log-mark-{}", section.key),
                        outcome_word(outcome),
                        14.,
                        cx,
                    )
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_primary)
                        .child(title),
                )
                .children(took.map(|took| ui::meta(took, cx).font_features(theme::tabular())))
                .into_any_element()
        }
        Item::Empty(_) => div()
            .mx(SPACE_3)
            .px(SPACE_3)
            .py(SPACE_2)
            .child(ui::meta("This step wrote nothing.", cx))
            .into_any_element(),
        Item::Line(index) => {
            let Some(line) = log.lines().get(index) else {
                return div().into_any_element();
            };
            let section_from = log
                .sections
                .iter()
                .find(|section| index >= section.from && index < section.to)
                .map_or(0, |section| section.from);
            let current = log.matches.get(log.current) == Some(&index);
            let matched = current || log.matches.binary_search(&index).is_ok();
            let (ink, weight) = match line.mark {
                Some(LogMark::Error) => (t.danger, FontWeight::NORMAL),
                Some(LogMark::Warning) => (t.warn, FontWeight::NORMAL),
                Some(LogMark::Notice | LogMark::Command) => (t.accent, FontWeight::NORMAL),
                Some(LogMark::Debug) => (t.text_tertiary, FontWeight::NORMAL),
                Some(LogMark::Group | LogMark::Section) => (t.text_primary, FontWeight::SEMIBOLD),
                Some(LogMark::Endgroup) | None => (t.text_secondary, FontWeight::NORMAL),
            };
            div()
                .mx(SPACE_3)
                .h(px(LINE))
                .flex()
                .items_center()
                .border_l_1()
                .border_r_1()
                .border_color(t.border)
                .bg(t.surface)
                .when(matched, |row| {
                    row.bg(if current {
                        t.warn.opacity(0.22)
                    } else {
                        t.warn.opacity(0.1)
                    })
                })
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS_H))
                .child(
                    div()
                        .w(px(52.))
                        .flex_none()
                        .pr(SPACE_2H)
                        .text_right()
                        .text_color(t.ink_faint)
                        .child((index - section_from + 1).to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(ink)
                        .font_weight(weight)
                        .child(line_text(line, &t)),
                )
                .into_any_element()
        }
    }
}
