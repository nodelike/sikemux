//! The desk's Calls page: the open call of a project from the top of the pane, the page it links
//! under its question, its choices as tiles with Go under them, then why it came up and what its
//! asker opened, the other calls waiting under it, and the composer it is answered with pinned at
//! the bottom.

use super::*;
use crate::chat::composer::{self, Act};
use crate::ui::Clicks;
use gpui_kit::StatefulInteractiveElement;
use sikemux_view::calls::{CallChange, CallChangedFile, CallOpened, CallOption};
use std::path::Path;
use std::rc::Rc;

const COLUMN: f32 = 720.;
const SIDE: f32 = 24.;
const PROSE: f32 = 620.;
const ATTACHMENT_HEIGHT: f32 = 240.;
const OPTION_PICTURE_HEIGHT: f32 = 176.;
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
            .map(|call| folded(&self.cards, call, None, &target, &self.bench, cx))
            .collect();
        let next = (calls.iter())
            .cycle()
            .skip_while(|call| call.id != open.id)
            .nth(1)
            .filter(|call| call.id != open.id)
            .map(|call| call.id.clone());
        let field = self.cards.update(cx, |cards, cx| {
            cards.saw(&open, &target);
            cards.field(&open, window, cx)
        });
        let reply = match open.secret {
            true => None,
            false => {
                let hint = reply_hint(&open, &self.cards.read(cx).chosen(&open)).into();
                (self.bench)
                    .update(cx, |bench, cx| bench.reply_view(&open, hint, window, cx))
                    .ok()
                    .flatten()
            }
        };
        let body = body(&open, next, through, cx);
        let waiting = (!waiting.is_empty()).then(|| {
            div()
                .mt(px(28.))
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
        let answered_with = match &reply {
            Some(view) => view.clone().into_element().isolated().into_any_element(),
            None => secret_field(&open, &field, through, window, cx),
        };
        let owner = reply::owner(&open.id);
        let page = div().size_full().relative().flex().flex_col();
        let page = match reply {
            Some(_) => crate::chat::takes_drops(page, &owner, self.bench.clone()),
            None => page,
        };
        page.child(
            div()
                .id(SharedString::from(format!("calls-page-{}", self.agent)))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .w_full()
                        .max_w(px(COLUMN + 2. * SIDE))
                        .mx_auto()
                        .px(px(SIDE))
                        .pt(px(SIDE))
                        .pb(px(28.))
                        .child(body)
                        .children(waiting),
                ),
        )
        .child(
            // The composer keeps a margin of its own inside; this one lines it up with the
            // column above.
            div().flex_none().px(px(SIDE) - theme::SPACE_4).child(
                div()
                    .w_full()
                    .max_w(px(COLUMN) + theme::SPACE_4 * 2.)
                    .mx_auto()
                    .child(answered_with),
            ),
        )
        .when(reply.is_some() && cx.has_active_drag(), |page| {
            let words = t!("Drop files to send with your answer");
            let hint = crate::chat::drop_hint(words, &owner, bench.read(cx), cx);
            page.child(gpui_kit::deferred(hint))
        })
        .into_any_element()
    }
}

/// What the empty composer of `call` says: what its words will be, and how files join them.
fn reply_hint(call: &PersonCall, chosen: &[String]) -> &'static str {
    let production = call.kind == CallKind::Production;
    match (call.options.is_empty(), chosen.is_empty(), production) {
        (true, ..) => t!("Answer in your own words, or drop a screenshot…"),
        (false, true, _) => t!("Pick one above, or answer in your own words…"),
        (false, false, true) => t!("Add a note, then click Go…"),
        (false, false, false) => t!("Add a note or drop a screenshot…"),
    }
}

/// The open call in full: who asks, the question, its choices with Go under them, why it came
/// up and what its asker opened.
fn body(call: &PersonCall, next: Option<String>, through: Through, cx: &mut App) -> AnyElement {
    let Through { cards, bench, .. } = through;
    let t = Tokens::get(cx).clone();
    let (chosen, sending) = {
        let cards = cards.read(cx);
        (cards.chosen(call), cards.sending(&call.id))
    };
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
    let tail = div()
        .flex_1()
        .flex()
        .items_center()
        .children(changes(call, &t))
        .child(div().ml_auto().child(open_chat));
    let who = who(call, through, Some(tail.into_any_element()), cx);
    let prose = |text: String| {
        div()
            .max_w(px(PROSE))
            .text_size(theme::ui_text(13.5))
            .line_height(relative(1.6))
            .text_color(t.text_secondary)
            .child(text)
    };
    let mut lines = (call.question.lines().map(str::trim)).filter(|line| !line.is_empty());
    let question = div()
        .mt(px(14.))
        .text_size(theme::ui_text(21.))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(relative(1.3))
        .text_color(t.text_primary)
        .child(lines.next().unwrap_or_default().to_owned());
    let more: Vec<gpui_kit::Div> = lines
        .map(|line| prose(line.to_owned()).mt(px(10.)))
        .collect();
    let page = linked_page(call, through, cx).map(|page| div().mt(px(16.)).child(page));
    let tiles = (!call.options.is_empty()).then(|| tiles(call, &chosen, sending, through, &t));
    let against = against_line(call, &t).map(|line| div().mt(SPACE_3_PX).child(line));
    let runs = (call.kind == CallKind::Production)
        .then_some(call.action.as_deref())
        .flatten()
        .map(|action| runs(call, action, &t));
    let go = go_row(call, &chosen, sending, next, through);
    let why: Vec<gpui_kit::Div> = (context_words(call).into_iter().map(prose))
        .chain(
            (call.evidence.iter())
                .filter(|evidence| evidence.picture.is_none())
                .map(|evidence| prose(evidence.what.clone()).text_color(t.text_tertiary)),
        )
        .collect();
    let why = (!why.is_empty()).then(|| {
        div()
            .mt(px(28.))
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
        .children(more)
        .children(page)
        .children(tiles)
        .children(against)
        .children(runs)
        .children(go)
        .children(why)
        .children(opened)
        .into_any_element()
}

/// Says a call changes production, and where, beside who asks it.
fn changes(call: &PersonCall, t: &Tokens) -> Option<gpui_kit::Div> {
    let words = match &call.place {
        Some(place) => t!("Production · {place}", place = place.name),
        None if call.kind == CallKind::Production => t!("Production").to_owned(),
        None => return None,
    };
    Some(
        div()
            .h(px(19.))
            .px(px(7.))
            .flex()
            .flex_none()
            .items_center()
            .rounded(radius_2())
            .bg(Hsla { a: 0.1, ..t.warn })
            .text_size(theme::ui_text(11.5))
            .text_color(t.warn)
            .child(words),
    )
}

/// The call's choices as tiles, each a full-width row.
fn tiles(
    call: &PersonCall,
    chosen: &[String],
    sending: bool,
    through: Through,
    t: &Tokens,
) -> gpui_kit::Div {
    let pictured = !call.secret && call.options.iter().any(|option| option.picture.is_some());
    let tiles = call.options.iter().enumerate().map(|(ix, option)| {
        let on = chosen.contains(&option.id);
        tile(call, (ix, option), (on, pictured), sending, through, t)
    });
    div()
        .mt(px(18.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .children(tiles)
}

/// One choice of `call`, its picture over its words when the call's choices have pictures. A
/// click chooses it and sends nothing; a click on its picture opens that full size.
fn tile(
    call: &PersonCall,
    (ix, option): (usize, &CallOption),
    (on, pictured): (bool, bool),
    sending: bool,
    through: Through,
    t: &Tokens,
) -> AnyElement {
    let Through { cards, bench, .. } = through;
    let picked = call.pick.as_ref() == Some(&option.id);
    let preview = pictured.then(|| {
        let shown = option.picture.as_deref().map(|path| {
            picture(
                SharedString::from(format!("call-option-picture-{}-{ix}", call.id)),
                path,
                option.label.clone(),
                Fill::Contain,
                bench,
                t,
            )
            .size_full()
            .rounded(px(3.))
            .border_0()
        });
        let pick = picked.then(|| {
            div()
                .absolute()
                .top(SPACE_2)
                .right(SPACE_2)
                .h(px(20.))
                .px(SPACE_1H)
                .flex()
                .items_center()
                .rounded(radius_2())
                .bg(t.surface_raised)
                .child(pick_mark(t))
        });
        div()
            .relative()
            .h(px(OPTION_PICTURE_HEIGHT))
            .w_full()
            .p(SPACE_3_PX)
            .bg(t.surface_sunken)
            .border_b(crate::theme::border())
            .border_color(t.line_soft)
            .children(shown)
            .children(pick)
    });
    let name = div()
        .flex()
        .items_center()
        .gap(px(9.))
        .child(radio(on, t))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(theme::ui_text(13.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .child(option.label.clone()),
        )
        .children((picked && !pictured).then(|| pick_mark(t)));
    let detail = option.detail.clone().map(|detail| {
        div()
            .mt(px(5.))
            .text_size(theme::ui_text(12.5))
            .line_height(relative(1.5))
            .text_color(t.text_tertiary)
            .child(detail)
    });
    let (cards, held, id) = (cards.clone(), call.clone(), option.id.clone());
    let hover = t.surface_hover;
    BaseButton::new(SharedString::from(format!("call-option-{}-{ix}", call.id)))
        .w_full()
        .flex()
        .flex_col()
        .justify_start()
        .overflow_hidden()
        .rounded(theme::radius_3())
        .border(crate::theme::border())
        .disabled(sending)
        .map(|tile| match on {
            true => tile.bg(t.surface_chosen).border_color(t.border_strong),
            false => (tile.bg(t.surface_composer).border_color(t.border))
                .hover(move |style| style.bg(hover)),
        })
        .on_click(move |_, _, cx| {
            cards.update(cx, |cards, cx| cards.choose(&held, &id, cx));
        })
        .children(preview)
        .child(
            div()
                .w_full()
                .px(px(14.))
                .pt(SPACE_3_PX)
                .pb(px(13.))
                .child(name)
                .children(detail),
        )
        .into_any_element()
}

/// What a production call runs on a yes, in full under its name.
fn runs(call: &PersonCall, action: &str, t: &Tokens) -> gpui_kit::Div {
    div()
        .mt(px(14.))
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(
            div()
                .text_size(theme::ui_text(12.))
                .text_color(t.text_tertiary)
                .child(t!("What runs")),
        )
        .child(
            div()
                .px(SPACE_3_PX)
                .py(px(9.))
                .rounded(px(8.))
                .border(crate::theme::border())
                .border_color(t.line_soft)
                .bg(t.surface_sunken)
                .font_family(theme::mono_font())
                .text_size(theme::ui_text(12.))
                .line_height(relative(1.55))
                .text_color(t.text_secondary)
                .child(crate::ui::selectable::selectable(
                    SharedString::from(format!("call-page-action-{}", call.id)),
                    action.to_owned(),
                    std::iter::empty(),
                )),
        )
}

/// Under the choices: Next call, which turns to the next one waiting, and the button that
/// answers this one with the choice made. A call with no choices is answered from its composer.
fn go_row(
    call: &PersonCall,
    chosen: &[String],
    sending: bool,
    next: Option<String>,
    through: Through,
) -> Option<gpui_kit::Div> {
    let go = (!call.options.is_empty()).then(|| {
        let option =
            (chosen.first()).and_then(|id| call.options.iter().find(|option| &option.id == id));
        let words: SharedString = match option {
            Some(option) => t!("Go with {option}", option = option.label).into(),
            None => t!("Go with this").into(),
        };
        let ready = !chosen.is_empty();
        go_button(call, words, ready, sending, through.bench, Workbench::reply).sized(30., 14., 13.)
    });
    let next = next.map(|next| {
        let cards = through.cards.clone();
        Button::new(
            SharedString::from(format!("call-next-{}", call.id)),
            t!("Next call"),
            Variant::Ghost,
        )
        .sized(30., 10., 12.5)
        .on_click(move |_, _, cx| {
            cards.update(cx, |cards, cx| {
                cards.opened = Some(next.clone());
                cx.notify();
            });
        })
    });
    (go.is_some() || next.is_some()).then(|| {
        div()
            .mt(px(14.))
            .flex()
            .items_center()
            .justify_end()
            .gap(SPACE_1H)
            .children(next)
            .children(go)
    })
}

/// What a call for a secret is answered with, in the composer's place: its masked field, which
/// takes no files, and the composer's own arrow.
fn secret_field(
    call: &PersonCall,
    field: &Entity<InputState>,
    through: Through,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let (chosen, sending) = {
        let cards = through.cards.read(cx);
        (cards.chosen(call), cards.sending(&call.id))
    };
    let typed = !field.read(cx).value().trim().is_empty();
    let lit = typed || field.read(cx).focus_handle(cx).is_focused(window);
    let on_send: Act = {
        let (bench, id) = (through.bench.clone(), call.id.clone());
        Rc::new(move |_: &mut Window, cx: &mut App| {
            bench.update(cx, |bench, cx| bench.go_with(&id, cx)).ok();
        })
    };
    let on_stop: Act = Rc::new(|_: &mut Window, _: &mut App| {});
    let ready = (typed || !chosen.is_empty()) && !sending;
    let by_mouse = call.kind == CallKind::Production;
    let send = composer::send(false, ready, None, (on_send, by_mouse), on_stop, 0., cx);
    div()
        .px(theme::SPACE_4)
        .pb(theme::SPACE_4)
        .child(
            div()
                .p(SPACE_2)
                .pl(SPACE_3_PX)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .rounded(theme::radius_3())
                .border(crate::theme::border())
                .border_color(t.border)
                .bg(t.surface_composer)
                .child(ui::icon(Icon::Lock, 12.).text_color(t.text_tertiary))
                .child(hinted_field(field, note_hint(call, &chosen), lit, t))
                .child(send),
        )
        .children(kept_line(call, t))
        .into_any_element()
}

/// What the asker opened for the call, drawn in full: its files, its changes and its evidence
/// pictures, each opening where it lives.
fn opened_by(call: &PersonCall, through: Through, t: &Tokens, cx: &mut App) -> Option<AnyElement> {
    let bench = through.bench;
    let owner = call.about.as_ref().unwrap_or(&call.asker);
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
    let shown: Vec<AnyElement> = opened.chain(pictures).collect();
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
                    .items_start()
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
        .max_h(px(ATTACHMENT_HEIGHT))
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

/// The pill naming who asks `call`, which opens that agent.
fn asker_pill(call: &PersonCall, through: Through, cx: &App) -> AnyElement {
    let Through { target, bench, .. } = through;
    let asker = (bench.upgrade())
        .map(|strong| asker_mark(strong.read(cx), call, 12., cx))
        .unwrap_or_else(|| ui::mark(&call.asker.cli, 12.));
    crate::chat::agent_pill_of(
        &call.asker.agent_id,
        &call.asker.name,
        asker,
        11.5,
        target.clone(),
        bench,
        cx,
    )
    .into_any_element()
}

/// A chat's open calls when more than one waits: one row with their count and the newest
/// question, which opens into a block of their rows.
pub(super) fn chat_fold(
    calls: &[PersonCall],
    open: bool,
    toggle: impl Fn(&mut App) + 'static,
    agent: &str,
    through: Through,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let Some(newest) = calls.iter().max_by_key(|call| call.raised_at) else {
        return div().into_any_element();
    };
    let one_asker = (calls.iter()).all(|call| call.asker.agent_id == newest.asker.agent_id);
    let dot = || div().text_color(t.text_disabled).child("·");
    let chevron = ui::icon(Icon::Chevron, 10.)
        .text_color(t.text_tertiary)
        .with_transformation(gpui_kit::Transformation::rotate(gpui_kit::Radians(
            match open {
                true => -std::f32::consts::FRAC_PI_2,
                false => std::f32::consts::FRAC_PI_2,
            },
        )));
    let folded = (!open).then(|| {
        [
            dot().into_any_element(),
            asker_pill(newest, through, cx),
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_secondary)
                .child(t!(
                    "Newest: {question}",
                    question = headline(&newest.question)
                ))
                .into_any_element(),
            div()
                .text_size(theme::ui_text(11.5))
                .font_features(theme::tabular())
                .text_color(t.text_tertiary)
                .child(ui::time::age(newest.raised_at))
                .into_any_element(),
        ]
    });
    let unfolded = open.then(|| {
        let asker = one_asker.then(|| [dot().into_any_element(), asker_pill(newest, through, cx)]);
        (asker.into_iter().flatten()).chain([div().flex_1().into_any_element()])
    });
    let hover = t.surface_hover;
    let head = BaseButton::new("call-chat-fold")
        .w_full()
        .h(px(36.))
        .pl(SPACE_2)
        .pr(px(10.))
        .flex()
        .items_center()
        .gap(px(9.))
        .text_size(theme::ui_text(13.))
        .text_color(t.text_primary)
        .hover(move |style| style.bg(hover))
        .on_click(move |_, _, cx| toggle(cx))
        .child(ui::icon(Icon::Bell, 12.).text_color(t.warn))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .child(t!("{count} calls", count = calls.len())),
        )
        .children(folded.into_iter().flatten())
        .children(unfolded.into_iter().flatten())
        .child(chevron);
    let rows = (open.then_some(calls).into_iter().flatten()).map(|call| {
        div()
            .border_t(crate::theme::border())
            .border_color(t.line)
            .child(chat_row(call, agent, through, true, cx))
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(radius_4())
        .border(crate::theme::border())
        .border_color(t.line)
        .bg(t.surface_raised)
        .overflow_hidden()
        .child(head)
        .children(rows)
        .into_any_element()
}

/// A call in its asker's chat, folded to one row: who asks, the question and its age, which
/// offers Open in Calls while hovered and opens it on the agent's desk. Inside the open calls'
/// block it drops its own frame and leaves its bell to the block's first row.
pub(super) fn chat_row(
    call: &PersonCall,
    agent: &str,
    through: Through,
    in_block: bool,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let (cards, id, bench_to, agent) = (
        through.cards.clone(),
        call.id.clone(),
        through.bench.clone(),
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
    let bell = match in_block {
        true => div().w(px(12.)).flex_none().into_any_element(),
        false => ui::icon(Icon::Bell, 12.)
            .text_color(t.warn)
            .into_any_element(),
    };
    BaseButton::new(SharedString::from(format!("call-chat-row-{}", call.id)))
        .group(CHAT_ROW)
        .w_full()
        .h(px(36.))
        .pl(SPACE_2)
        .pr(px(10.))
        .flex()
        .items_center()
        .gap(px(9.))
        .when(!in_block, |row| {
            row.rounded(radius_4())
                .border(crate::theme::border())
                .border_color(t.line)
                .bg(t.surface_raised)
        })
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
        .child(bell)
        .child(asker_pill(call, through, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(headline(&call.question).to_owned()),
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
