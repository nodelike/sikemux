//! Calls: decisions put to the person, as each host's core lists them in its view. One card
//! draws a call wherever it shows: as the last thing in its asker's chat, and open in the title
//! bar's popover over the calls of the project in front.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::base::{Button as BaseButton, Popover};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, relative, Anchor, AnyElement, App, AppContext, Context, Entity, EventEmitter,
    Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, ObjectFit, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, Subscription, WeakEntity,
    Window,
};
use sikemux_core::server::call;
use sikemux_core::server::questions::AnswerParams;
use sikemux_view::calls::{
    AnswerCallParams, CallClient, CallKind, CallParams, CallResult, CallState, PersonCall,
};
use sikemux_view::device::DeviceView;

use crate::chat::Press;
use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::picture::Picture;
use crate::theme::{self, Tokens, RADIUS_2, RADIUS_4, SPACE_1, SPACE_1H, SPACE_2};
use crate::ui::{self, Button, Variant};
use crate::workbench::{Strip, Workbench};

const CARD_WIDTH: f32 = 452.;
const ROW_HEIGHT: f32 = 34.;
const FACE: f32 = 20.;
const STRIP_HEIGHT: f32 = 132.;
const STRIP_LEAD: f32 = 286.;
const EVIDENCE_HEIGHT: f32 = 148.;
/// The width inside a card's picture frame: the card less its panel's and its own padding and
/// the frame's edges.
const EVIDENCE_WIDTH: f32 = CARD_WIDTH - 2. * 6. - 2. - 2. * 10. - 2.;
const PICTURE_SIZE: f32 = 800.;

/// What the person has done on each open call's card, so the chat and the popover show the same
/// choice and the same words while the call waits.
#[derive(Default)]
pub struct Cards {
    held: HashMap<String, Held>,
    /// The calls this window told their core it drew.
    seen: HashSet<String>,
    /// The call the popover shows open, until another is picked; else its newest.
    opened: Option<String>,
    /// Each evidence picture's width over its height, read once from its file.
    ratios: HashMap<String, Option<f32>>,
}

struct Held {
    /// The options chosen, until the person touches the rows the asker's pick.
    chosen: Option<Vec<String>>,
    field: Entity<InputState>,
    sending: bool,
    /// The row the arrows rest on, on a call that takes several.
    cursor: Option<usize>,
    /// What runs on a yes shows.
    action_open: bool,
    _typed: Subscription,
}

/// Return in a card's field: answer the call with what the card holds.
pub struct GoWith(pub String);

impl EventEmitter<GoWith> for Cards {}

impl Cards {
    fn chosen(&self, call: &PersonCall) -> Vec<String> {
        match self.held.get(&call.id).and_then(|held| held.chosen.clone()) {
            Some(chosen) => chosen,
            None => call.pick.iter().cloned().collect(),
        }
    }

    fn cursor(&self, id: &str) -> Option<usize> {
        self.held.get(id).and_then(|held| held.cursor)
    }

    fn field_focused(&self, id: &str, window: &Window, cx: &App) -> bool {
        (self.held.get(id))
            .is_some_and(|held| held.field.read(cx).focus_handle(cx).is_focused(window))
    }

    /// A key on the card of `call`, as today's question rows take them: a number chooses its
    /// row, or ticks it on a call that takes several; the arrows move the chosen row, or the
    /// row Space ticks. Says whether it took it.
    fn press(&mut self, call: &PersonCall, press: Press, cx: &mut Context<Self>) -> bool {
        let count = call.options.len();
        if count == 0 || !self.held.contains_key(&call.id) {
            return false;
        }
        let position = |id: &String| call.options.iter().position(|option| &option.id == id);
        let first_chosen = self.chosen(call).first().and_then(position);
        let cursor = self.cursor(&call.id).or(first_chosen);
        let step = |from: Option<usize>, up: bool| match (from, up) {
            (Some(at), true) => (at + count - 1) % count,
            (Some(at), false) => (at + 1) % count,
            (None, true) => count - 1,
            (None, false) => 0,
        };
        match (press, call.multi) {
            (Press::Choose(ix), _) if ix < count => {
                if let Some(held) = self.held.get_mut(&call.id) {
                    held.cursor = Some(ix);
                }
                match call.multi {
                    true => self.choose(call, &call.options[ix].id, cx),
                    false => self.set_chosen(call, vec![call.options[ix].id.clone()], cx),
                }
            }
            (Press::Up | Press::Down, true) => {
                let at = step(cursor, press == Press::Up);
                if let Some(held) = self.held.get_mut(&call.id) {
                    held.cursor = Some(at);
                }
                cx.notify();
            }
            (Press::Up | Press::Down, false) => {
                let at = step(first_chosen, press == Press::Up);
                self.set_chosen(call, vec![call.options[at].id.clone()], cx);
            }
            (Press::Tick, true) => {
                let at = cursor.unwrap_or(0);
                self.choose(call, &call.options[at].id, cx);
            }
            _ => return false,
        }
        true
    }

    fn set_chosen(&mut self, call: &PersonCall, chosen: Vec<String>, cx: &mut Context<Self>) {
        if let Some(held) = self.held.get_mut(&call.id) {
            held.chosen = Some(chosen);
        }
        cx.notify();
    }

    fn action_open(&self, id: &str) -> bool {
        self.held.get(id).is_some_and(|held| held.action_open)
    }

    fn sending(&self, id: &str) -> bool {
        self.held.get(id).is_some_and(|held| held.sending)
    }

    /// Chooses an option, or clears the one chosen; on a call that takes several, ticks it.
    fn choose(&mut self, call: &PersonCall, option: &str, cx: &mut Context<Self>) {
        let mut chosen = self.chosen(call);
        let had = chosen.iter().position(|id| id == option);
        match (call.multi, had) {
            (_, Some(at)) => {
                chosen.remove(at);
            }
            (true, None) => chosen.push(option.to_owned()),
            (false, None) => chosen = vec![option.to_owned()],
        }
        if let Some(held) = self.held.get_mut(&call.id) {
            held.chosen = Some(chosen);
        }
        cx.notify();
    }

    /// The card's field, made the first time the card draws.
    fn field(
        &mut self,
        call: &PersonCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(held) = self.held.get(&call.id) {
            return held.field.clone();
        }
        let field = cx.new(|cx| InputState::new(window, cx).masked(call.secret));
        let id = call.id.clone();
        let typed = cx.subscribe(&field, move |_, _, event: &InputEvent, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => cx.emit(GoWith(id.clone())),
            _ => {}
        });
        self.held.insert(
            call.id.clone(),
            Held {
                chosen: None,
                field: field.clone(),
                sending: false,
                cursor: None,
                action_open: false,
                _typed: typed,
            },
        );
        field
    }

    fn ratio(&mut self, path: &str) -> Option<f32> {
        *(self.ratios.entry(path.to_owned())).or_insert_with(|| {
            let (width, height) = image::image_dimensions(path).ok()?;
            (height > 0).then(|| width as f32 / height as f32)
        })
    }

    /// Tells the core once that this window drew the call, which a production call needs
    /// before its answer counts.
    fn saw(&mut self, call: &PersonCall, target: &Target) {
        if self.seen.insert(call.id.clone()) {
            // Unanswered on purpose: a call answered elsewhere meanwhile is no error here.
            drop(target.request_typed::<call::CallSeen>(CallParams {
                id: call.id.clone(),
            }));
        }
    }

    /// What answers the call as the card stands: the options chosen with the note, or the
    /// person's own words alone.
    fn answer(&self, call: &PersonCall, cx: &App) -> Option<AnswerCallParams> {
        let held = self.held.get(&call.id)?;
        let typed = held.field.read(cx).value().trim().to_owned();
        let typed = (!typed.is_empty()).then_some(typed);
        let chosen = self.chosen(call);
        let id = call.id.clone();
        match (call.multi, chosen.as_slice()) {
            (_, []) => typed.map(|words| AnswerCallParams {
                id,
                words: Some(words),
                ..AnswerCallParams::default()
            }),
            (true, options) => Some(AnswerCallParams {
                id,
                options: options.to_vec(),
                note: typed,
                ..AnswerCallParams::default()
            }),
            (false, [option, ..]) => Some(AnswerCallParams {
                id,
                option: Some(option.clone()),
                note: typed,
                ..AnswerCallParams::default()
            }),
        }
    }

    /// Lets go of what was held for calls no longer open anywhere.
    pub(crate) fn keep(&mut self, open: &HashSet<&str>) {
        self.held.retain(|id, _| open.contains(id.as_str()));
        self.seen.retain(|id| open.contains(id.as_str()));
        if self.opened.as_deref().is_some_and(|id| !open.contains(id)) {
            self.opened = None;
        }
    }
}

impl Workbench {
    /// Each core's view this window holds, with where it is.
    fn views(&self) -> impl Iterator<Item = (Place, &DeviceView)> {
        let this = (self.feeds.this.view.as_ref()).map(|view| (Place::This, view));
        let hosts = (self.feeds.hosts())
            .filter_map(|(key, feed)| Some((Place::Account(key.clone()), feed.view.as_ref()?)));
        this.into_iter().chain(hosts)
    }

    pub(crate) fn call_at(&self, id: &str) -> Option<(Place, &PersonCall)> {
        self.views().find_map(|(place, view)| {
            let call = view.calls.iter().find(|call| call.id == id)?;
            Some((place, call))
        })
    }

    /// The calls of `project` on the host at `place` that wait on the person, newest first.
    pub(crate) fn open_calls(&self, place: &Place, project: &str) -> Vec<&PersonCall> {
        let Some(view) = self.view_at(place) else {
            return Vec::new();
        };
        let mut open: Vec<&PersonCall> = (view.calls.iter())
            .filter(|call| call.state == CallState::Open && call.project == project)
            .collect();
        open.sort_by_key(|call| std::cmp::Reverse(call.raised_at));
        open
    }

    /// The calls agent `agent` asks, oldest first, and those answered since its chat's last row.
    pub(crate) fn calls_of(&self, place: &Place, agent: &str, last_row: u64) -> CallsOf {
        let Some(view) = self.view_at(place) else {
            return CallsOf::default();
        };
        let mut of = CallsOf::default();
        for call in view.calls.iter().filter(|call| waits(call, agent)) {
            match call.state {
                CallState::Open => of.open.push(call.clone()),
                _ if settled_at(call) >= last_row => of.settled.push(call.clone()),
                _ => {}
            }
        }
        of.open.sort_by_key(|call| call.raised_at);
        of.settled.sort_by_key(settled_at);
        of
    }

    /// The oldest call agent `agent` waits on the person for, on the host at `place`.
    pub(crate) fn waiting_call(&self, place: &Place, agent: &str) -> Option<&PersonCall> {
        (self.view_at(place)?.calls.iter())
            .filter(|call| call.state == CallState::Open && waits(call, agent))
            .min_by_key(|call| call.raised_at)
    }

    /// Answers the call agent `agent` waits on with what the person typed in its chat, with the
    /// files they sent. A production call passes only by its button, so the words go to the
    /// agent instead, as does anything typed while no call waits. Says whether it answered one.
    pub(crate) fn answer_in_chat(
        &mut self,
        agent: &str,
        text: &str,
        (paths, context): (Vec<String>, Vec<sikemux_wire::protocol::ChatContext>),
        cx: &mut Context<Self>,
    ) -> bool {
        let target = self.chat_target(agent);
        let Some(call) = self
            .waiting_call(target.place(), agent)
            .filter(|call| call.kind != CallKind::Production && !call.secret)
        else {
            return false;
        };
        let asked_by_question = self
            .feeds
            .questions()
            .any(|question| question.id == call.id);
        let asked: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>> =
            match asked_by_question {
                true => Box::pin(target.request_typed::<call::AnswerQuestion>(AnswerParams {
                    id: call.id.clone(),
                    answer: Some(text.to_owned()),
                    paths,
                    context,
                    ..AnswerParams::default()
                })),
                false => Box::pin(target.request_typed::<call::AnswerCall>(AnswerCallParams {
                    id: call.id.clone(),
                    words: Some(text.to_owned()),
                    paths,
                    ..AnswerCallParams::default()
                })),
            };
        cx.spawn(async move |bench, cx| {
            if let Err(error) = asked.await {
                bench
                    .update(cx, |bench, cx| bench.notify(crate::toast::error(error), cx))
                    .ok();
            }
        })
        .detach();
        true
    }

    /// Lets go of the cards of calls answered or gone, once a core's view says so.
    pub(crate) fn keep_cards(&self, cx: &mut Context<Self>) {
        let open: HashSet<&str> = (self.views())
            .flat_map(|(_, view)| view.calls.iter())
            .filter(|call| call.state == CallState::Open)
            .map(|call| call.id.as_str())
            .collect();
        self.calls.update(cx, |cards, _| cards.keep(&open));
    }

    /// The call whose card takes the keys of `agent`'s chat: the oldest it waits on, unless it
    /// asks for a secret, whose field takes them.
    pub(crate) fn keyed_call(&self, place: &Place, agent: &str) -> Option<String> {
        (self.waiting_call(place, agent))
            .filter(|call| !call.secret)
            .map(|call| call.id.clone())
    }

    /// A key on call `id`'s card. Return answers it, as Go with this does; the rest choose its
    /// rows, unless its field holds the keys. Says whether the card took it.
    pub(crate) fn press_call(
        &mut self,
        id: &str,
        press: Press,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((_, call)) = self.call_at(id) else {
            return false;
        };
        let call = call.clone();
        let cards = self.calls.clone();
        if press == Press::Pick {
            self.go_with(id, cx);
            return true;
        }
        if cards.read(cx).field_focused(id, window, cx) {
            return false;
        }
        cards.update(cx, |cards, cx| cards.press(&call, press, cx))
    }

    /// Hands call `id` back to its asker's lead, or its asker, to decide.
    pub(crate) fn not_mine(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((place, _)) = self.call_at(id) else {
            return;
        };
        let handed = Target::new(&self.asks, place)
            .request_typed::<call::CallNotMine>(CallParams { id: id.to_owned() });
        cx.spawn(async move |bench, cx| {
            if let Err(error) = handed.await {
                bench
                    .update(cx, |bench, cx| bench.notify(crate::toast::error(error), cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Answers call `id` with what its card holds.
    pub(crate) fn go_with(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((place, call)) = self.call_at(id) else {
            return;
        };
        let cards = self.calls.clone();
        let Some(params) = cards.read(cx).answer(call, cx) else {
            return;
        };
        if cards.read(cx).sending(id) {
            return;
        }
        cards.update(cx, |cards, cx| {
            if let Some(held) = cards.held.get_mut(id) {
                held.sending = true;
            }
            cx.notify();
        });
        let id = params.id.clone();
        let asked = Target::new(&self.asks, place).request_typed::<call::AnswerCall>(params);
        cx.spawn(async move |bench, cx| {
            let failed = asked.await.err();
            bench
                .update(cx, |bench, cx| {
                    bench.calls.update(cx, |cards, cx| {
                        if let Some(held) = cards.held.get_mut(&id) {
                            held.sending = false;
                        }
                        cx.notify();
                    });
                    if let Some(error) = failed {
                        bench.notify(crate::toast::error(error), cx);
                    }
                })
                .ok();
        })
        .detach();
    }
}

#[derive(Default)]
pub(crate) struct CallsOf {
    pub open: Vec<PersonCall>,
    pub settled: Vec<PersonCall>,
}

/// Whether agent `agent` asks `call`, or waits on it beside its asker.
fn waits(call: &PersonCall, agent: &str) -> bool {
    call.asker.agent_id == agent || call.joined.iter().any(|joined| joined.agent_id == agent)
}

fn settled_at(call: &PersonCall) -> u64 {
    (call.outcome.as_ref().map(|outcome| outcome.at))
        .or(call.answer.as_ref().map(|answer| answer.at))
        .unwrap_or(call.raised_at)
}

/// The asker's own face, or its agent's mark; the chief wears Sikemux's.
fn asker_mark(bench: &Workbench, call: &PersonCall, size: f32, cx: &App) -> AnyElement {
    if call.asker.chief {
        return ui::chief_mark(size);
    }
    match bench.agent_by_id(&call.asker.agent_id) {
        Some(agent) => agent.mark(size, cx),
        None => ui::mark(&call.asker.cli, size),
    }
}

/// Where a card is drawn, which decides what its footer offers: the popover's card opens its
/// asker's chat, closing the popover.
enum Seat {
    Chat,
    Popover(crate::ui::list_picker::Dismiss),
}

/// What a card answers through: the core of its call, the cards' store and the window's store.
struct Through<'a> {
    target: &'a Target,
    cards: &'a Entity<Cards>,
    bench: &'a WeakEntity<Workbench>,
}

/// The card of `call`: who asks, the question, its context and pictures, the options with the
/// field under them, the point against, and the footer that answers it.
fn card(
    call: &PersonCall,
    mark: AnyElement,
    seat: Seat,
    through: Through,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
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
    let (chosen, sending) = {
        let cards = cards.read(cx);
        (cards.chosen(call), cards.sending(&call.id))
    };
    let typed = !field.read(cx).value().trim().is_empty();
    let who = div()
        .h(px(FACE))
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(theme::ui_text(12.))
        .text_color(t.text_tertiary)
        .child(mark)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_secondary)
                .child(format!("@{}", call.asker.name)),
        )
        .child(div().text_color(t.text_disabled).child("·"))
        .child(ui::time::age(call.raised_at));
    let question = div()
        .mt(SPACE_2)
        .mb(px(3.))
        .text_size(theme::ui_text(15.))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(relative(1.35))
        .text_color(t.text_primary)
        .child(call.question.clone());
    let context = match &call.brief {
        Some(brief) => Some(format!(
            "{} {} {}",
            brief.tried, brief.shows, brief.cannot_undo
        )),
        None => call.context.clone(),
    };
    let prose = |text: String| {
        div()
            .text_size(theme::ui_text(12.5))
            .line_height(relative(1.5))
            .text_color(t.text_secondary)
            .child(text)
    };
    let evidence_lines = (call.evidence.iter())
        .filter(|evidence| evidence.picture.is_none())
        .map(|evidence| prose(evidence.what.clone()).text_color(t.text_tertiary));
    let options_pictured =
        !call.secret && call.options.iter().any(|option| option.picture.is_some());
    let pictures = match options_pictured {
        true => Some(strip(call, &chosen, cards, bench, &t)),
        false => {
            let shown: Vec<(&str, String)> = (call.evidence.iter())
                .filter_map(|evidence| Some((evidence.picture.as_deref()?, evidence.what.clone())))
                .take(2)
                .collect();
            let width = match shown.len() > 1 {
                true => (EVIDENCE_WIDTH - 6.) / 2.,
                false => EVIDENCE_WIDTH,
            };
            (!shown.is_empty()).then(|| {
                div()
                    .w_full()
                    .h(px(EVIDENCE_HEIGHT))
                    .flex()
                    .gap(px(6.))
                    .children(shown.into_iter().enumerate().map(|(ix, (path, what))| {
                        let fill = match cards.update(cx, |cards, _| cards.ratio(path)) {
                            Some(ratio) => Fill::Top(ratio, width),
                            None => Fill::Cover,
                        };
                        let label = label_words(&what).map(picture_label);
                        picture(
                            SharedString::from(format!("call-evidence-{}-{ix}", call.id)),
                            path,
                            what,
                            fill,
                            bench,
                            &t,
                        )
                        .w(px(width))
                        .h_full()
                        .children(label)
                    }))
                    .into_any_element()
            })
        }
    };
    let cursor = cards.read(cx).cursor(&call.id).filter(|_| call.multi);
    let rows = (call.options.iter()).enumerate().map(|(ix, option)| {
        let on = chosen.contains(&option.id);
        let key = match call.multi {
            true => tick(on, &t),
            false => radio(on, &t),
        };
        let pick = (call.pick.as_ref() == Some(&option.id)).then(|| {
            div()
                .ml_auto()
                .pl(SPACE_2)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(5.))
                .text_size(theme::ui_text(11.))
                .text_color(t.text_tertiary)
                .child(ui::mark_box(12., ui::mark("sikemux", 17.)))
                .child("pick")
        });
        let label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(div().child(option.label.clone()))
            .children(option.detail.clone().map(|detail| {
                div()
                    .text_size(theme::ui_text(11.5))
                    .line_height(relative(1.4))
                    .text_color(t.text_tertiary)
                    .child(detail)
            }));
        let (cards, call) = (cards.clone(), call.clone());
        let option = option.id.clone();
        let hover = t.surface_hover;
        BaseButton::new(SharedString::from(format!("call-{}-{option}", call.id)))
            .w_full()
            .min_h(px(ROW_HEIGHT))
            .py(px(7.))
            .px(SPACE_3_PX)
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(theme::ui_text(13.))
            .text_color(t.text_primary)
            .disabled(sending)
            .map(|row| match (on, cursor == Some(ix)) {
                (true, _) => row.bg(t.surface_chosen),
                (false, true) => row.bg(hover),
                (false, false) => row.hover(move |style| style.bg(hover)),
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cards.update(cx, |cards, cx| cards.choose(&call, &option, cx));
            })
            .child(key)
            .child(label)
            .children(pick)
            .into_any_element()
    });
    let hint = match (chosen.is_empty(), call.secret) {
        (_, true) => "Type or paste it…",
        (false, false) => "Add a note…",
        (true, false) => "Or answer in your own words…",
    };
    let focused = field.read(cx).focus_handle(cx).is_focused(window);
    let note = note_row(&field, hint, focused || typed, !call.options.is_empty(), &t);
    let mut block = div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.line)
        .rounded(px(8.))
        .overflow_hidden();
    for (ix, row) in rows.enumerate() {
        block = block.child(
            div()
                .when(ix > 0, |row| row.border_t_1().border_color(t.line))
                .child(row),
        );
    }
    let block = block.child(note);
    let kept = call.secret.then(|| {
        div()
            .mt(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(theme::ui_text(11.5))
            .text_color(t.text_tertiary)
            .child(ui::icon(Icon::Shield, 11.).text_color(t.text_tertiary))
            .child(format!(
                "Kept private: @{} reads it, and it never shows in the chat.",
                call.asker.name
            ))
    });
    let against = call.against.clone().map(|against| {
        div()
            .text_size(theme::ui_text(12.5))
            .line_height(relative(1.5))
            .text_color(t.text_secondary)
            .child(
                gpui_kit::StyledText::new(format!("Against it: {against}")).with_highlights([(
                    0.."Against it:".len(),
                    gpui_kit::HighlightStyle {
                        color: Some(t.text_tertiary),
                        ..Default::default()
                    },
                )]),
            )
    });
    let action_open = cards.read(cx).action_open(&call.id);
    let runs = (call.kind == CallKind::Production)
        .then_some(call.action.as_ref())
        .flatten()
        .map(|action| what_runs(call, action, action_open, cards, &t));
    let ready = !chosen.is_empty() || typed;
    let go = {
        let (bench, id) = (bench.clone(), call.id.clone());
        Button::new(
            SharedString::from(format!("call-go-{}", call.id)),
            "Go with this",
            Variant::Primary,
        )
        .sized(26., 11., 12.)
        .disabled(!ready || sending)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            bench.update(cx, |bench, cx| bench.go_with(&id, cx)).ok();
        })
    };
    let open_chat = match seat {
        Seat::Chat => None,
        Seat::Popover(dismiss) => Some((dismiss, bench.clone(), call.asker.agent_id.clone())),
    };
    let open_chat = open_chat.map(|(dismiss, bench, agent)| {
        Button::new(
            SharedString::from(format!("call-chat-{}", call.id)),
            "Open chat",
            Variant::Ghost,
        )
        .icon(Icon::Message)
        .sized(26., 9., 12.)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            dismiss(window, cx);
            bench
                .update(cx, |bench, cx| bench.open_agent(&agent, cx))
                .ok();
        })
    });
    let not_mine = call.can_hand_back.then(|| {
        let (bench, id) = (bench.clone(), call.id.clone());
        Button::new(
            SharedString::from(format!("call-not-mine-{}", call.id)),
            "Not mine",
            Variant::Ghost,
        )
        .sized(26., 9., 12.)
        .disabled(sending)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            bench.update(cx, |bench, cx| bench.not_mine(&id, cx)).ok();
        })
    });
    let foot = div()
        .mt(px(14.))
        .flex()
        .items_center()
        .gap(SPACE_1)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .text_size(theme::ui_text(11.5))
                .text_color(t.text_disabled)
                .child(ui::icon(Icon::Clock, 11.).text_color(t.text_disabled))
                .child(div().truncate().child("Nothing happens until you answer")),
        )
        .children(not_mine)
        .children(open_chat)
        .child(go);
    div()
        .p(px(10.))
        .flex()
        .flex_col()
        .child(who)
        .child(question)
        .children(context.map(prose))
        .children(evidence_lines)
        .map(|card| match pictures {
            Some(pictures) => card
                .child(div().mt(SPACE_3_PX).child(pictures))
                .child(div().h(px(14.))),
            None => card.child(div().h(SPACE_3_PX)),
        })
        .child(block)
        .children(kept)
        .children(against.map(|against| div().mt(SPACE_3_PX).child(against)))
        .children(runs)
        .child(foot)
        .into_any_element()
}

const SPACE_3_PX: gpui_kit::Pixels = theme::SPACE_3;

/// What a production call runs on a yes, folded under a row that opens it.
fn what_runs(
    call: &PersonCall,
    action: &str,
    open: bool,
    cards: &Entity<Cards>,
    t: &Tokens,
) -> impl IntoElement {
    let (cards, id) = (cards.clone(), call.id.clone());
    let hover = t.text_secondary;
    let row = BaseButton::new(SharedString::from(format!("call-runs-{}", call.id)))
        .flex()
        .justify_start()
        .items_center()
        .gap(SPACE_1H)
        .text_size(theme::ui_text(12.))
        .text_color(t.text_tertiary)
        .hover(move |style| style.text_color(hover))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cards.update(cx, |cards, cx| {
                if let Some(held) = cards.held.get_mut(&id) {
                    held.action_open = !held.action_open;
                }
                cx.notify();
            });
        })
        .child(
            ui::icon(Icon::Chevron, 10.)
                .text_color(t.text_tertiary)
                .when(open, |chevron| {
                    chevron.with_transformation(gpui_kit::Transformation::rotate(
                        gpui_kit::Radians(std::f32::consts::FRAC_PI_2),
                    ))
                }),
        )
        .child("What runs");
    let command = open.then(|| {
        div()
            .mt(SPACE_1H)
            .font_family(theme::MONO)
            .text_size(theme::ui_text(12.))
            .line_height(relative(1.5))
            .text_color(t.text_secondary)
            .child(crate::ui::selectable::selectable(
                SharedString::from(format!("call-action-{}", call.id)),
                action.to_owned(),
                std::iter::empty(),
            ))
    });
    div().mt(SPACE_2).child(row).children(command)
}

/// The last row of the options: a note that goes with the option chosen, or with none chosen,
/// the person's own answer.
fn note_row(
    field: &Entity<InputState>,
    hint: &'static str,
    lit: bool,
    under_options: bool,
    t: &Tokens,
) -> impl IntoElement {
    let ring = Hsla {
        a: 0.55,
        ..t.accent
    };
    div()
        .h(px(ROW_HEIGHT))
        .px(SPACE_3_PX)
        .flex()
        .items_center()
        .gap(px(10.))
        .bg(t.surface_sunken)
        .when(under_options && !lit, |row| {
            row.border_t_1().border_color(t.line)
        })
        .when(lit, |row| {
            row.border_1()
                .border_color(ring)
                .when(under_options, |row| row.rounded_b(px(7.)))
                .when(!under_options, |row| row.rounded(px(7.)))
        })
        .child(ui::icon(Icon::Pencil, 11.).text_color(match lit {
            true => t.text_tertiary,
            false => t.text_disabled,
        }))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .text_size(theme::ui_text(13.))
                .text_color(t.text_primary)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .text_color(t.text_disabled)
                        .when(lit, |hint| hint.invisible())
                        .child(hint),
                )
                .child(
                    Input::new(field)
                        .appearance(false)
                        .bordered(false)
                        .p_0()
                        .text_size(theme::ui_text(13.)),
                ),
        )
}

/// The ring of an option on a call that takes one.
fn radio(on: bool, t: &Tokens) -> AnyElement {
    let ring = div().size(px(13.)).flex_none().rounded_full();
    match on {
        true => ring.border(px(4.)).border_color(t.accent).bg(t.on_accent),
        false => ring.border(px(1.5)).border_color(t.text_disabled),
    }
    .into_any_element()
}

/// The box of an option on a call that takes several.
fn tick(on: bool, t: &Tokens) -> AnyElement {
    let tick = div()
        .size(px(13.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.));
    match on {
        true => tick
            .bg(t.accent)
            .child(ui::icon(Icon::Check, 9.).text_color(t.on_accent)),
        false => tick.border(px(1.5)).border_color(t.text_disabled),
    }
    .into_any_element()
}

/// How a picture fills its place on the card.
#[derive(Clone, Copy)]
enum Fill {
    /// All of it covered, cut evenly at the edges.
    Cover,
    /// From its top, as a page is read, given its width over its height and the width it
    /// fills: its full width, or wider when that would leave its place short.
    Top(f32, f32),
}

/// A picture on the card, which opens full size.
fn picture(
    id: SharedString,
    path: &str,
    title: String,
    fill: Fill,
    bench: &WeakEntity<Workbench>,
    t: &Tokens,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let shown = Picture::File(PathBuf::from(path));
    let (bench, opened, title) = (bench.clone(), shown.clone(), SharedString::from(title));
    div()
        .id(id)
        .relative()
        .overflow_hidden()
        .rounded(px(8.))
        .bg(t.surface_sunken)
        .border_1()
        .border_color(t.line)
        .cursor_pointer()
        .child(match fill {
            Fill::Cover => img(shown.source_at(px(PICTURE_SIZE)))
                .size_full()
                .object_fit(ObjectFit::Cover),
            Fill::Top(ratio, width) => {
                let height = (width / ratio).max(EVIDENCE_HEIGHT);
                img(shown.source_at(px(PICTURE_SIZE)))
                    .w(px(height * ratio))
                    .h(px(height))
            }
            .object_fit(ObjectFit::Cover),
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            bench
                .update(cx, |bench, cx| {
                    bench.show_picture_titled(opened.clone(), title.clone(), window, cx)
                })
                .ok();
        })
}

/// An evidence item's own words, when they are short enough to be a name on its picture (Now,
/// New), else none.
fn label_words(words: &str) -> Option<String> {
    let trimmed = words.trim();
    let short = trimmed.chars().count() <= 20 && trimmed.split_whitespace().count() <= 3;
    (!trimmed.is_empty() && short).then(|| trimmed.to_owned())
}

/// A picture's name on its corner, such as Now and New on two evidence pages side by side.
fn picture_label(text: String) -> gpui_kit::Div {
    div()
        .absolute()
        .left(px(6.))
        .bottom(px(6.))
        .max_w(relative(0.9))
        .px(px(6.))
        .py(px(2.))
        .rounded(px(5.))
        .bg(gpui_kit::black().opacity(0.62))
        .text_color(gpui_kit::white())
        .text_size(theme::ui_text(11.))
        .font_weight(FontWeight::MEDIUM)
        .child(div().min_w_0().truncate().child(text))
}

/// A taste call's pictures, one per option: the chosen option's leads, large, and the others
/// wait beside it, smaller and dimmed; a click on one chooses it.
fn strip(
    call: &PersonCall,
    chosen: &[String],
    cards: &Entity<Cards>,
    bench: &WeakEntity<Workbench>,
    t: &Tokens,
) -> AnyElement {
    let lead = (chosen.first())
        .or(call.pick.as_ref())
        .and_then(|id| call.options.iter().position(|option| &option.id == id))
        .unwrap_or(0);
    let shown = |ix: usize| {
        let option = &call.options[ix];
        let id = SharedString::from(format!("call-picture-{}-{ix}", call.id));
        match option.picture.as_deref() {
            Some(path) => picture(id, path, option.label.clone(), Fill::Cover, bench, t),
            None => div().id(id).rounded(px(8.)).bg(t.surface_sunken),
        }
    };
    let rest = (0..call.options.len()).filter(|ix| *ix != lead).map(|ix| {
        let (cards, call, option) = (cards.clone(), call.clone(), call.options[ix].id.clone());
        shown(ix)
            .flex_1()
            .w_full()
            .opacity(0.72)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cards.update(cx, |cards, cx| cards.choose(&call, &option, cx));
            })
    });
    div()
        .h(px(STRIP_HEIGHT))
        .flex()
        .gap(SPACE_1H)
        .child(
            shown(lead)
                .w(px(STRIP_LEAD))
                .flex_none()
                .h_full()
                .border_color(t.border_strong),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .children(rest),
        )
        .into_any_element()
}

/// The card's panel, as it floats in a chat.
fn panel(t: &Tokens) -> gpui_kit::Div {
    div()
        .w(px(CARD_WIDTH))
        .max_w_full()
        .p(SPACE_1H)
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.line)
        .rounded(RADIUS_4)
        .shadow(vec![card_shadow(2., 4., 0.35), card_shadow(14., 40., 0.45)])
}

fn card_shadow(down: f32, blur: f32, alpha: f32) -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow {
        color: gpui_kit::hsla(0., 0., 0., alpha),
        offset: gpui_kit::point(px(0.), px(down)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

/// What came of a call, in one quiet line where its card stood: what the person said and what
/// it led to.
fn notice(call: &PersonCall, t: &Tokens) -> Option<AnyElement> {
    let label = |id: &String| {
        (call.options.iter())
            .find(|option| &option.id == id)
            .map(|option| format!("“{}”", option.label))
    };
    let answered = call.answer.as_ref().map(|answer| {
        let picked: Vec<String> = (answer.option.iter())
            .chain(&answer.options)
            .filter_map(label)
            .collect();
        match (&answer.by, picked.is_empty(), call.secret) {
            (_, _, true) => "You answered, kept private".to_owned(),
            (CallClient::Chat, ..) => "You answered in the chat".to_owned(),
            (_, false, _) => format!("You chose {}", picked.join(", ")),
            (_, true, _) => "You answered in your own words".to_owned(),
        }
    });
    let outcome = call.outcome.as_ref().and_then(|outcome| {
        Some(match (outcome.result, outcome.healthy) {
            (CallResult::Shipped, Some(true)) => "shipped · healthy".to_owned(),
            (CallResult::Shipped, Some(false)) => "shipped · unhealthy".to_owned(),
            (CallResult::Shipped, None) => "shipped".to_owned(),
            (CallResult::NotShipped, _) => "not shipped".to_owned(),
            (CallResult::Failed, _) => "it failed".to_owned(),
            (CallResult::MayHaveRun, _) => "may have run, Sikemux restarted".to_owned(),
            (CallResult::HandedBack, _) => format!("handed back to @{}", call.asker.name),
            (CallResult::Withdrawn, _) => {
                format!("@{} went on without an answer", call.asker.name)
            }
            (CallResult::Heard | CallResult::Unknown, _) => return None,
        })
    });
    let said = match (answered, outcome) {
        (Some(answered), Some(outcome)) => format!("{answered} · {outcome}"),
        (Some(said), None) | (None, Some(said)) => said,
        (None, None) => return None,
    };
    let lost = matches!(
        call.outcome.as_ref().map(|outcome| outcome.result),
        Some(CallResult::Failed)
    );
    Some(
        div()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .text_size(theme::ui_text(12.))
            .text_color(t.text_tertiary)
            .child(
                ui::icon(if lost { Icon::Close } else { Icon::Check }, 11.)
                    .text_color(t.text_tertiary),
            )
            .child(div().min_w_0().truncate().child(said))
            .child(div().text_color(t.text_disabled).child("·"))
            .children(crate::chat::clock(settled_at(call)))
            .into_any_element(),
    )
}

/// The calls an agent asks, at the end of its chat, over the line saying how it stands.
pub struct InChat {
    bench: WeakEntity<Workbench>,
    cards: Entity<Cards>,
    target: Target,
    calls: CallsOf,
    _cards: Subscription,
}

impl InChat {
    pub(crate) fn new(
        bench: WeakEntity<Workbench>,
        cards: Entity<Cards>,
        target: Target,
        cx: &mut Context<Self>,
    ) -> Self {
        let observed = cx.observe(&cards, |_, _, cx| cx.notify());
        Self {
            bench,
            cards,
            target,
            calls: CallsOf::default(),
            _cards: observed,
        }
    }

    pub(crate) fn show(&mut self, calls: CallsOf, cx: &mut Context<Self>) {
        if self.calls.open != calls.open || self.calls.settled != calls.settled {
            self.calls = calls;
            cx.notify();
        }
    }
}

impl Render for InChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.calls.open.is_empty() && self.calls.settled.is_empty() {
            return div().into_any_element();
        }
        let t = Tokens::get(cx).clone();
        let Some(bench) = self.bench.upgrade() else {
            return div().into_any_element();
        };
        let marks: Vec<AnyElement> = (self.calls.open.iter())
            .map(|call| asker_mark(bench.read(cx), call, FACE, cx))
            .collect();
        let notices = (self.calls.settled.iter()).filter_map(|call| notice(call, &t));
        let cards: Vec<AnyElement> = (self.calls.open.iter())
            .zip(marks)
            .map(|(call, mark)| {
                panel(&t)
                    .child(card(
                        call,
                        mark,
                        Seat::Chat,
                        Through {
                            target: &self.target,
                            cards: &self.cards,
                            bench: &self.bench,
                        },
                        window,
                        cx,
                    ))
                    .into_any_element()
            })
            .collect();
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(800.))
                    .px(px(22.))
                    .pt(SPACE_2)
                    .pb(SPACE_2)
                    .flex()
                    .flex_col()
                    .gap(SPACE_2)
                    .children(notices)
                    .children(cards),
            )
            .into_any_element()
    }
}

/// The title bar's count of the calls of the project in front, beside its name, which opens
/// them; none while nothing waits.
pub(crate) fn chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let Some(Strip::Project(_)) = bench.strip() else {
        return None;
    };
    let project = bench.project()?.path.to_string_lossy().into_owned();
    let count = bench.open_calls(&bench.place(), &project).len();
    if count == 0 {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let this = cx.entity().downgrade();
    let cards = bench.calls.clone();
    let words = match count {
        1 => "1 call".to_owned(),
        count => format!("{count} calls"),
    };
    let hover = t.surface_hover;
    let chip = Popover::new("calls-chip")
        .anchor(Anchor::TopLeft)
        .offset(SPACE_1H)
        .trigger_with(move |open, _, _| {
            div()
                .id("calls-chip-button")
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .rounded(RADIUS_2)
                .cursor_pointer()
                .text_size(theme::ui_text(12.))
                .text_color(t.text_primary)
                .map(|chip| match open {
                    true => chip.bg(t.surface_chosen),
                    false => chip.hover(move |style| style.bg(hover)),
                })
                .child(div().size(px(7.)).rounded_full().bg(t.warn))
                .child(words.clone())
                .into_any_element()
        })
        .content(move |_, window, cx| {
            let Some(bench) = this.upgrade() else {
                return div().into_any_element();
            };
            let dismiss = crate::ui::list_picker::dismiss(cx.entity());
            popover(&bench, &cards, &project, dismiss, window, cx)
        });
    let dot = div()
        .px(SPACE_1)
        .text_size(theme::ui_text(12.))
        .text_color(Tokens::get(cx).text_disabled)
        .child("·");
    Some(
        div()
            .flex()
            .items_center()
            .child(dot)
            .child(chip)
            .into_any_element(),
    )
}

/// The calls of the project in front: the open one as its card, the rest a line each under it.
fn popover(
    bench: &Entity<Workbench>,
    cards: &Entity<Cards>,
    project: &str,
    dismiss: crate::ui::list_picker::Dismiss,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (calls, place, name) = {
        let read = bench.read(cx);
        let calls: Vec<PersonCall> = (read.open_calls(&read.place(), project).into_iter())
            .cloned()
            .collect();
        let name = read
            .project()
            .map(|project| project.name.clone())
            .unwrap_or_default();
        (calls, read.place(), name)
    };
    let opened = cards.read(cx).opened.clone();
    let Some(open) = (opened.as_ref())
        .and_then(|id| calls.iter().find(|call| &call.id == id))
        .or(calls.first())
        .cloned()
    else {
        return div().into_any_element();
    };
    let target = Target::new(&bench.read(cx).asks, place);
    let weak = bench.downgrade();
    let mark = asker_mark(bench.read(cx), &open, FACE, cx);
    let through = Through {
        target: &target,
        cards,
        bench: &weak,
    };
    let shown = card(&open, mark, Seat::Popover(dismiss), through, window, cx);
    let rest: Vec<AnyElement> = (calls.iter())
        .filter(|call| call.id != open.id)
        .map(|call| {
            let mark = asker_mark(bench.read(cx), call, FACE, cx);
            let (cards, id) = (cards.clone(), call.id.clone());
            let hover = t.surface_hover;
            BaseButton::new(SharedString::from(format!("call-row-{}", call.id)))
                .w_full()
                .h(px(30.))
                .pl(SPACE_2)
                .pr(px(10.))
                .flex()
                .items_center()
                .gap(px(9.))
                .rounded(RADIUS_2)
                .text_size(theme::ui_text(13.))
                .text_color(t.text_primary)
                .hover(move |style| style.bg(hover))
                .on_click(move |_, _, cx| {
                    cards.update(cx, |cards, cx| {
                        cards.opened = Some(id.clone());
                        cx.notify();
                    });
                })
                .child(mark)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(call.question.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(theme::ui_text(11.5))
                        .text_color(t.text_tertiary)
                        .child(format!("@{}", call.asker.name)),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(26.))
                        .flex()
                        .justify_end()
                        .text_size(theme::ui_text(11.5))
                        .font_features(theme::tabular())
                        .text_color(t.text_tertiary)
                        .child(ui::time::age(call.raised_at)),
                )
                .into_any_element()
        })
        .collect();
    let head = div()
        .px(px(10.))
        .pt(SPACE_1H)
        .pb(SPACE_1)
        .flex()
        .items_center()
        .gap(px(10.))
        .child(ui::caps("Calls", cx))
        .child(div().flex_1().h(px(1.)).bg(t.line))
        .child(
            div()
                .text_size(theme::ui_text(11.))
                .text_color(t.text_tertiary)
                .child(name),
        );
    call_keys(
        div().id("calls-popover").key_context("QuestionDock"),
        &open.id,
        &weak,
    )
    .w(px(CARD_WIDTH))
    .p(SPACE_1H)
    .bg(t.surface_overlay)
    .border_1()
    .border_color(t.line)
    .rounded(RADIUS_4)
    .shadow(t.shadow_float())
    .child(head)
    .child(shown)
    .when(!rest.is_empty(), |pop| {
        pop.child(div().mx(px(10.)).my(SPACE_1).h(px(1.)).bg(t.line))
            .children(rest)
    })
    .into_any_element()
}

/// The keys a card takes, bound as the question rows' are, for call `id`.
fn call_keys<E: InteractiveElement>(keyed: E, id: &str, bench: &WeakEntity<Workbench>) -> E {
    use crate::chat::{Choose, Confirm, NextChoice, PreviousChoice, Tick};
    fn on<A: gpui_kit::Action>(
        id: &str,
        bench: &WeakEntity<Workbench>,
        press: impl Fn(&A) -> Press + 'static,
    ) -> impl Fn(&A, &mut Window, &mut App) + 'static {
        let (id, bench) = (id.to_owned(), bench.clone());
        move |action, window, cx| {
            let taken = bench.update(cx, |bench, cx| {
                bench.press_call(&id, press(action), window, cx)
            });
            if !matches!(taken, Ok(true)) {
                cx.propagate();
            }
        }
    }
    keyed
        .on_action(on(id, bench, |choose: &Choose| Press::Choose(choose.0)))
        .on_action(on(id, bench, |_: &PreviousChoice| Press::Up))
        .on_action(on(id, bench, |_: &NextChoice| Press::Down))
        .on_action(on(id, bench, |_: &Tick| Press::Tick))
        .on_action(on(id, bench, |_: &Confirm| Press::Pick))
}

/// The amber dot on another project's row in the tree while calls of it wait on the person.
pub(crate) fn waits_on_person(bench: &Workbench, place: &Place, project: &str) -> bool {
    !bench.open_calls(place, project).is_empty()
}
