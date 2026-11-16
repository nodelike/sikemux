//! The desk's Calls page: the open call of a project in full, its options first, then why it
//! came up and what its asker opened, with the other calls waiting under it.

use super::*;
use crate::ui::Clicks;
use gpui_kit::StatefulInteractiveElement;
use sikemux_view::calls::{CallChange, CallChangedFile, CallOpened};
use std::path::Path;

const COLUMN: f32 = 640.;
const ATTACHMENT_HEIGHT: f32 = 240.;
const OPTION_PICTURE_HEIGHT: f32 = 200.;
const FOOT_HEIGHT: f32 = 52.;
/// The hover group of a call's row in a chat, which offers Open in Calls while hovered.
const CHAT_ROW: &str = "call-chat-row";

/// The project path of agent `agent`'s desk.
fn project_of(bench: &Workbench, agent: &str) -> Option<String> {
    let row = bench.agent_by_id(agent)?;
    (bench.projects().iter())
        .find(|project| project.id == row.project)
        .map(|project| project.path.to_string_lossy().into_owned())
}

/// How many calls wait on the person in the project of agent `agent`'s desk.
pub(crate) fn waiting_on_desk(bench: &Workbench, agent: &str) -> usize {
    project_of(bench, agent).map_or(0, |project| {
        bench.open_calls(&bench.place(), &project).len()
    })
}

/// The Calls page of one agent's desk, over the calls of its project.
pub struct Page {
    bench: WeakEntity<Workbench>,
    cards: Entity<Cards>,
    agent: String,
    _cards: Subscription,
}

impl Page {
    pub(crate) fn new(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> Entity<Self> {
        let weak = cx.entity().downgrade();
        let cards = bench.calls.clone();
        let agent = agent.to_owned();
        cx.new(|cx| {
            let observed = cx.observe(&cards, |_, _, cx| cx.notify());
            Self {
                bench: weak,
                cards,
                agent,
                _cards: observed,
            }
        })
    }
}

impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let Some(bench) = self.bench.upgrade() else {
            return div().into_any_element();
        };
        let (calls, target) = {
            let read = bench.read(cx);
            let calls: Vec<PersonCall> = project_of(read, &self.agent)
                .map(|project| {
                    (read.open_calls(&read.place(), &project).into_iter())
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            (calls, read.target())
        };
        let opened = self.cards.read(cx).opened.clone();
        let Some(open) = (opened.as_ref())
            .and_then(|id| calls.iter().find(|call| &call.id == id))
            .or(calls.first())
            .cloned()
        else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(SPACE_2)
                .child(ui::icon(Icon::Bell, 18.).text_color(t.text_disabled))
                .child(ui::meta(t!("Nothing waits on you"), cx))
                .into_any_element();
        };
        let through = Through {
            target: &target,
            cards: &self.cards,
            bench: &self.bench,
        };
        let waiting: Vec<AnyElement> = (calls.iter())
            .filter(|call| call.id != open.id)
            .map(|call| folded(&self.cards, call, &target, &self.bench, cx))
            .collect();
        let next = (calls.iter())
            .cycle()
            .skip_while(|call| call.id != open.id)
            .nth(1)
            .filter(|call| call.id != open.id)
            .map(|call| call.id.clone());
        let body = body(&open, through, window, cx);
        let waiting = (!waiting.is_empty()).then(|| {
            div()
                .mt(px(26.))
                .child(section(t!("Waiting"), Some(waiting.len()), &t, cx))
                .child(
                    div()
                        .mt(px(6.))
                        .mx(px(-6.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(waiting),
                )
        });
        let foot = foot(&open, next, through, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("calls-page-{}", self.agent)))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(COLUMN))
                            .px(px(20.))
                            .pt(SPACE_1)
                            .pb(px(28.))
                            .child(body)
                            .children(waiting),
                    ),
            )
            .child(foot)
            .into_any_element()
    }
}

/// The open call in full: who asks, the question, the options with the note, why it came up
/// and what its asker opened.
fn body(call: &PersonCall, through: Through, window: &mut Window, cx: &mut App) -> AnyElement {
    let Through {
        target,
        cards,
        bench,
    } = through;
    let t = Tokens::get(cx).clone();
    let field = cards.update(cx, |cards, cx| {
        cards.saw(call, target);
        cards.field(call, window, cx)
    });
    let (chosen, sending, action_open) = {
        let cards = cards.read(cx);
        (
            cards.chosen(call),
            cards.sending(&call.id),
            cards.action_open(&call.id),
        )
    };
    let typed = !field.read(cx).value().trim().is_empty();
    let open_chat = {
        let (bench, agent) = (bench.clone(), call.asker.agent_id.clone());
        Button::new(
            SharedString::from(format!("call-page-chat-{}", call.id)),
            t!("Open chat"),
            Variant::Ghost,
        )
        .icon(Icon::Message)
        .sized(24., 8., 12.)
        .on_click(move |_, _, cx| {
            bench
                .update(cx, |bench, cx| bench.open_agent(&agent, cx))
                .ok();
        })
    };
    let who = who(
        call,
        through,
        Some(div().ml_auto().child(open_chat).into_any_element()),
        cx,
    );
    let question = div()
        .mt(px(12.))
        .text_size(theme::ui_text(20.))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(relative(1.3))
        .text_color(t.text_primary)
        .child(call.question.clone());
    let focused = field.read(cx).focus_handle(cx).is_focused(window);
    let lit = focused || typed;
    let hint = note_hint(call, &chosen);
    let pictured = !call.secret && call.options.iter().any(|option| option.picture.is_some());
    let answers = match pictured {
        true => div()
            .child(option_pictures(call, &chosen, sending, cards, bench, &t))
            .children(against_line(call, &t).map(|line| div().mt(px(10.)).child(line)))
            .child(div().mt(px(12.)).child(answers(
                std::iter::empty(),
                note_row(&field, hint, lit, false, &t),
                &t,
            ))),
        false => {
            let rows = (call.options.iter())
                .map(|option| option_row(call, option, &chosen, sending, cards, &t));
            let under = !call.options.is_empty();
            div()
                .child(answers(rows, note_row(&field, hint, lit, under, &t), &t))
                .children(against_line(call, &t).map(|line| div().mt(px(10.)).child(line)))
        }
    };
    let runs = (call.kind == CallKind::Production)
        .then_some(call.action.as_ref())
        .flatten()
        .map(|action| what_runs(call, action, action_open, cards, &t));
    let prose = |text: String| {
        div()
            .text_size(theme::ui_text(13.5))
            .line_height(relative(1.6))
            .text_color(t.text_secondary)
            .child(text)
    };
    let why: Vec<gpui_kit::Div> = (context_words(call).into_iter().map(prose))
        .chain(
            (call.evidence.iter())
                .filter(|evidence| evidence.picture.is_none())
                .map(|evidence| prose(evidence.what.clone()).text_color(t.text_tertiary)),
        )
        .collect();
    let why = (!why.is_empty()).then(|| {
        div()
            .mt(px(24.))
            .child(section(t!("Why it came up"), None, &t, cx))
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .flex_col()
                    .gap(SPACE_2)
                    .children(why),
            )
    });
    let opened = opened_by(call, through, &t, cx);
    div()
        .flex()
        .flex_col()
        .child(who)
        .child(question)
        .child(div().mt(px(14.)).child(answers))
        .children(kept_line(call, &t))
        .children(runs)
        .children(why)
        .children(opened)
        .into_any_element()
}

/// What the asker opened for the call, drawn in full: the page it links and its evidence
/// pictures, each opening where it lives.
fn opened_by(call: &PersonCall, through: Through, t: &Tokens, cx: &mut App) -> Option<AnyElement> {
    let Through {
        target,
        cards,
        bench,
    } = through;
    let owner = call.about.as_ref().unwrap_or(&call.asker);
    let page = call.page.as_ref().map(|page| {
        let picture = cards.update(cx, |cards, cx| cards.page_picture(&page.id, target, cx));
        let (bench, agent, id) = (bench.clone(), owner.agent_id.clone(), page.id.clone());
        let shown = div()
            .h(px(ATTACHMENT_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.surface_sunken)
            .child(match picture {
                Some(picture) => img(picture)
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                None => ui::icon(Icon::Tabs, 18.)
                    .text_color(t.text_disabled)
                    .into_any_element(),
            });
        let hover = t.surface_hover;
        attachment(
            div()
                .id(SharedString::from(format!("call-page-{}", call.id)))
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .clicks(move |_, _, cx| {
                    bench
                        .update(cx, |bench, cx| bench.open_page_in_tab(&agent, &id, cx))
                        .ok();
                }),
            shown,
            Icon::Tabs,
            page.title.clone(),
            t,
        )
        .into_any_element()
    });
    let pictures = (call.evidence.iter().enumerate()).filter_map(|(ix, evidence)| {
        let path = evidence.picture.as_deref()?;
        let shown = picture(
            SharedString::from(format!("call-page-evidence-{}-{ix}", call.id)),
            path,
            evidence.what.clone(),
            Fill::Contain,
            bench,
            t,
        )
        .w_full()
        .h(px(ATTACHMENT_HEIGHT))
        .rounded_none()
        .border_0();
        Some(
            attachment(
                div().id(SharedString::from(format!("call-shot-{}-{ix}", call.id))),
                shown,
                Icon::Image,
                evidence.what.clone(),
                t,
            )
            .into_any_element(),
        )
    });
    let opened = (call.opened.iter().enumerate())
        .filter_map(|(ix, opened)| opened_one(call, ix, opened, &owner.agent_id, bench, t));
    let shown: Vec<AnyElement> = page.into_iter().chain(opened).chain(pictures).collect();
    if shown.is_empty() {
        return None;
    }
    let label = t!("What @{name} opened", name = owner.name);
    Some(
        div()
            .mt(px(24.))
            .child(section(&label, Some(shown.len()), t, cx))
            .child(
                div()
                    .mt(px(12.))
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .children(shown),
            )
            .into_any_element(),
    )
}

/// A file as it read where the asker opened it, or the changes of its checkout; a click opens it.
fn opened_one(
    call: &PersonCall,
    ix: usize,
    opened: &CallOpened,
    agent: &str,
    bench: &WeakEntity<Workbench>,
    t: &Tokens,
) -> Option<AnyElement> {
    let id = SharedString::from(format!("call-opened-{}-{ix}", call.id));
    let hover = t.surface_hover;
    let frame = div()
        .id(id)
        .cursor_pointer()
        .hover(move |style| style.bg(hover));
    let (bench, agent) = (bench.clone(), agent.to_owned());
    let shown = match opened {
        CallOpened::File {
            path,
            line,
            first,
            excerpt,
        } => {
            let (opening, at) = (PathBuf::from(path), *line);
            let frame = frame.clicks(move |_, _, cx| {
                bench
                    .update(cx, |bench, cx| {
                        bench.open_file_on_desk(&agent, opening.clone(), at, None, cx)
                    })
                    .ok();
            });
            let name = Path::new(path)
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
            let caption = match line {
                Some(line) => format!("{name}:{line}"),
                None => name,
            };
            attachment(
                frame,
                file_lines(excerpt, *first, *line, t),
                Icon::File,
                caption,
                t,
            )
        }
        CallOpened::Diff { files, .. } => {
            let frame = frame.clicks(move |_, _, cx| {
                bench.update(cx, |bench, cx| bench.open_git(cx)).ok();
            });
            let caption = t!("Changes · {count} files", count = files.len());
            attachment(frame, changed_files(files, t), Icon::Git, caption, t)
        }
        CallOpened::Unknown => return None,
    };
    Some(shown.into_any_element())
}

fn file_lines(excerpt: &str, first: u32, opened_at: Option<u32>, t: &Tokens) -> gpui_kit::Div {
    let rows = excerpt.lines().enumerate().map(|(ix, text)| {
        let number = first + ix as u32;
        div()
            .flex()
            .gap(SPACE_3_PX)
            .px(SPACE_3_PX)
            .when(opened_at == Some(number), |row| row.bg(t.surface_chosen))
            .child(
                div()
                    .w(px(28.))
                    .flex_none()
                    .text_right()
                    .text_color(t.text_disabled)
                    .child(number.to_string()),
            )
            .child(
                div()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_color(t.text_secondary)
                    .child(text.to_owned()),
            )
    });
    div()
        .h(px(ATTACHMENT_HEIGHT))
        .w_full()
        .py(SPACE_2)
        .overflow_hidden()
        .bg(t.surface_sunken)
        .font_family(crate::theme::mono_font())
        .text_size(theme::ui_text(11.5))
        .line_height(px(18.))
        .children(rows)
}

fn changed_files(files: &[CallChangedFile], t: &Tokens) -> gpui_kit::Div {
    let rows = files.iter().map(|file| {
        let (mark, colour) = match file.change {
            CallChange::Added => ("A", t.git_added),
            CallChange::Deleted => ("D", t.git_deleted),
            CallChange::Renamed => ("R", t.git_renamed),
            CallChange::Modified | CallChange::Unknown => ("M", t.git_modified),
        };
        div()
            .h(px(22.))
            .px(SPACE_3_PX)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .font_family(crate::theme::mono_font())
                    .text_color(colour)
                    .child(mark),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_secondary)
                    .child(file.path.clone()),
            )
    });
    div()
        .h(px(ATTACHMENT_HEIGHT))
        .w_full()
        .py(SPACE_2)
        .overflow_hidden()
        .bg(t.surface_sunken)
        .text_size(theme::ui_text(12.))
        .children(rows)
}

/// One thing the asker opened: `shown` over a line naming it.
fn attachment(
    frame: gpui_kit::Stateful<gpui_kit::Div>,
    shown: impl IntoElement,
    icon: Icon,
    name: String,
    t: &Tokens,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    frame
        .flex_1()
        .min_w(px(220.))
        .flex()
        .flex_col()
        .rounded(px(10.))
        .overflow_hidden()
        .border(crate::theme::border())
        .border_color(t.line)
        .child(shown)
        .child(
            div()
                .h(px(32.))
                .px(SPACE_3_PX)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .border_t(crate::theme::border())
                .border_color(t.line)
                .text_size(theme::ui_text(12.))
                .text_color(t.text_secondary)
                .child(ui::icon(icon, 11.).text_color(t.text_tertiary))
                .child(div().min_w_0().truncate().child(name)),
        )
}

/// A taste call's options as large pictures side by side, each with its row under it; a click on
/// the row chooses it and a click on the picture opens it full size.
fn option_pictures(
    call: &PersonCall,
    chosen: &[String],
    sending: bool,
    cards: &Entity<Cards>,
    bench: &WeakEntity<Workbench>,
    t: &Tokens,
) -> AnyElement {
    let tiles = call.options.iter().enumerate().map(|(ix, option)| {
        let on = chosen.contains(&option.id);
        let shown = match option.picture.as_deref() {
            Some(path) => picture(
                SharedString::from(format!("call-option-picture-{}-{ix}", call.id)),
                path,
                option.label.clone(),
                Fill::Contain,
                bench,
                t,
            )
            .size_full()
            .into_any_element(),
            None => div()
                .size_full()
                .rounded(px(8.))
                .bg(t.surface_sunken)
                .into_any_element(),
        };
        let pick = (call.pick.as_ref() == Some(&option.id)).then(|| pick_mark(t).ml_auto());
        let (cards, held, id) = (cards.clone(), call.clone(), option.id.clone());
        let hover = t.surface_hover;
        BaseButton::new(SharedString::from(format!("call-option-{}-{ix}", call.id)))
            .flex_1()
            .min_w(px(220.))
            .flex()
            .flex_col()
            .rounded(px(10.))
            .border(crate::theme::border())
            .border_color(t.line)
            .disabled(sending)
            .map(|tile| match on {
                true => tile.bg(t.surface_chosen),
                false => tile.hover(move |style| style.bg(hover)),
            })
            .on_click(move |_, _, cx| {
                cards.update(cx, |cards, cx| cards.choose(&held, &id, cx));
            })
            .child(
                div()
                    .h(px(OPTION_PICTURE_HEIGHT))
                    .w_full()
                    .p(px(10.))
                    .pb_0()
                    .child(shown),
            )
            .child(
                div()
                    .px(SPACE_3_PX)
                    .py(px(10.))
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .text_size(theme::ui_text(13.))
                    .text_color(t.text_primary)
                    .child(div().mt(px(2.)).child(radio(on, t)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(option.label.clone())
                            .children(option.detail.clone().map(|detail| {
                                div()
                                    .text_size(theme::ui_text(11.5))
                                    .line_height(relative(1.4))
                                    .text_color(t.text_tertiary)
                                    .child(detail)
                            })),
                    )
                    .children(pick),
            )
            .into_any_element()
    });
    div()
        .flex()
        .flex_wrap()
        .gap(px(12.))
        .children(tiles)
        .into_any_element()
}

/// A section's name in spaced capitals, with its count, over a hairline to the edge.
fn section(name: &str, count: Option<usize>, t: &Tokens, cx: &App) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::caps(name, cx))
        .children(count.map(|count| {
            ui::caps(&count.to_string(), cx)
                .text_color(t.text_disabled)
                .font_features(theme::tabular())
        }))
        .child(div().flex_1().h(px(1.)).bg(t.line))
}

/// The page's foot, pinned under it: Not now, which turns to the next call waiting, and the
/// button that answers the open one.
fn foot(call: &PersonCall, next: Option<String>, through: Through, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let cards = through.cards.read(cx);
    let chosen = cards.chosen(call);
    let sending = cards.sending(&call.id);
    let typed = (cards.held.get(&call.id))
        .is_some_and(|held| !held.field.read(cx).value().trim().is_empty());
    let words: SharedString =
        match (chosen.first()).and_then(|id| call.options.iter().find(|option| &option.id == id)) {
            Some(option) => t!("Go with {option}", option = option.label).into(),
            None => t!("Go with this").into(),
        };
    let go = go_button(
        call,
        words,
        !chosen.is_empty() || typed,
        sending,
        through.bench,
    );
    let not_now = next.map(|next| {
        let cards = through.cards.clone();
        Button::new(
            SharedString::from(format!("call-not-now-{}", call.id)),
            t!("Not now"),
            Variant::Ghost,
        )
        .sized(26., 10., 12.)
        .on_click(move |_, _, cx| {
            cards.update(cx, |cards, cx| {
                cards.opened = Some(next.clone());
                cx.notify();
            });
        })
    });
    div()
        .h(px(FOOT_HEIGHT))
        .flex_none()
        .px(px(20.))
        .flex()
        .items_center()
        .justify_end()
        .gap(SPACE_1H)
        .border_t(crate::theme::border())
        .border_color(t.line)
        .children(not_now)
        .child(go)
        .into_any_element()
}

/// A call in its asker's chat, folded to one row: who asks, the question and its age, which
/// offers Open in Calls while hovered and opens it on the agent's desk.
pub(super) fn chat_row(call: &PersonCall, agent: &str, through: Through, cx: &App) -> AnyElement {
    let Through {
        target,
        cards,
        bench,
    } = through;
    let t = Tokens::get(cx);
    let asker = (bench.upgrade())
        .map(|strong| asker_mark(strong.read(cx), call, 12., cx))
        .unwrap_or_else(|| ui::mark(&call.asker.cli, 12.));
    let (cards, id, bench_to, agent) = (
        cards.clone(),
        call.id.clone(),
        bench.clone(),
        agent.to_owned(),
    );
    let hover = t.surface_hover;
    let open_in_calls = div()
        .absolute()
        .right_0()
        .top_0()
        .bottom_0()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .whitespace_nowrap()
        .text_size(theme::ui_text(12.))
        .text_color(t.text_secondary)
        .opacity(0.)
        .group_hover(CHAT_ROW, |style| style.opacity(1.))
        .child(t!("Open in Calls"))
        .child(ui::icon(Icon::Chevron, 10.).text_color(t.text_secondary));
    let age = div()
        .text_size(theme::ui_text(11.5))
        .font_features(theme::tabular())
        .text_color(t.text_tertiary)
        .group_hover(CHAT_ROW, |style| style.opacity(0.))
        .child(ui::time::age(call.raised_at));
    BaseButton::new(SharedString::from(format!("call-chat-row-{}", call.id)))
        .group(CHAT_ROW)
        .w_full()
        .h(px(36.))
        .pl(SPACE_2)
        .pr(px(10.))
        .flex()
        .items_center()
        .gap(px(9.))
        .rounded(radius_4())
        .border(crate::theme::border())
        .border_color(t.line)
        .bg(t.surface_raised)
        .text_size(theme::ui_text(13.))
        .text_color(t.text_primary)
        .hover(move |style| style.bg(hover))
        .on_click(move |_, _, cx| {
            cards.update(cx, |cards, cx| {
                cards.opened = Some(id.clone());
                cx.notify();
            });
            bench_to
                .update(cx, |bench, cx| bench.show_calls(&agent, cx))
                .ok();
        })
        .child(ui::icon(Icon::Bell, 12.).text_color(t.warn))
        .child(crate::chat::agent_pill_of(
            &call.asker.agent_id,
            &call.asker.name,
            asker,
            11.5,
            target.clone(),
            bench,
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(call.question.clone()),
        )
        .child(
            div()
                .relative()
                .flex_none()
                .min_w(px(96.))
                .h_full()
                .flex()
                .items_center()
                .justify_end()
                .child(age)
                .child(open_in_calls),
        )
        .into_any_element()
}
