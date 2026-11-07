//! The pane's first screen, Home: the calls only the person can make, as cards, then a line for
//! each goal and agent at work, over the composer that starts more; the same cards on a goal's
//! page; and what happened while the person was away, by goal. The core ranks, places and counts
//! them; the pane only groups them under the goal each item is on.

use crate::board::ChangesBoards;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, Anchor, AnyElement, App, ClickEvent, Context, Div, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window,
};
use gpui_kit::{AppContext, Entity, KeyDownEvent, Subscription};
use sikemux_core::server::board::BoardCardParams;
use sikemux_core::server::board::BoardNoteParams;
use sikemux_core::server::board::BoardPassParams;
use sikemux_core::server::board::HandOnParams as BoardHandOnParams;
use sikemux_core::server::call;
use sikemux_core::server::questions::AnswerParams;
use sikemux_view::inbox::{InboxItem, InboxKind, InboxNeed, LookPicture, RaisedFor, SwarmDecision};
use sikemux_view::slice::{Gate, Proof};

use super::evidence::minute;
use super::goal::{crumb, row};
use super::room::Chosen;
use super::{empty, goal_name, At, GoalsPane};
use crate::chat::{NewerPrompt, OlderPrompt, START};
use crate::theme::{
    self, Tokens, RADIUS_2, RADIUS_3, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3,
};
use crate::theme::{SPACE_3H, SPACE_5, TEXT_2XS, TEXT_MD, TEXT_SM, TEXT_XL, TEXT_XS};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// The least a row's words keep before its end drops under them.
const ROW_SAYS_MIN_W: Pixels = px(180.);
/// How much of a question shows before the rest waits behind Show all.
const ASKED_LINES: usize = 6;
const ASKED_CHARS: usize = 420;

/// Something that finished or failed while the person was away.
#[derive(Clone, Copy)]
pub(super) struct Happened<'a> {
    done: bool,
    item: &'a InboxItem,
}

/// The name of the project at `folder`, or the folder's last part when no project is there.
pub(super) fn project_name(bench: &Workbench, folder: &str) -> String {
    let named = (bench.projects().iter())
        .find(|project| project.path.to_string_lossy() == folder)
        .map(|project| project.name.clone());
    named.unwrap_or_else(|| {
        let last = folder.rsplit('/').find(|part| !part.is_empty());
        last.unwrap_or(folder).to_owned()
    })
}

impl GoalsPane {
    /// Whether an inbox item is about the project the view shows: its card is on that project's
    /// board, or its agent works in it.
    fn shows(&self, item: &InboxItem, bench: &Workbench) -> bool {
        let Some(here) = self.here.as_deref() else {
            return false;
        };
        if let Some(card) = &item.card {
            return card.project == here;
        }
        let id = bench.project().map(|project| project.id.as_str());
        item.project
            .as_deref()
            .is_some_and(|project| project == here || Some(project) == id)
    }

    /// What in every project waits on the person, in the core's order, besides the questions
    /// agents asked; the same things the core counts as needing the person.
    pub(super) fn needs(bench: &Workbench) -> Vec<InboxItem> {
        bench.shown().map_or(Vec::new(), |view| {
            (view.inbox.items.iter())
                .filter(|item| item.need.is_some() && item.kind != InboxKind::Question)
                .cloned()
                .collect()
        })
    }

    /// What in the project the view shows finished or failed while the person was away, in time
    /// order.
    pub(super) fn happened(&self, bench: &Workbench) -> Vec<Happened<'_>> {
        let Some(digest) = self.digest.as_ref() else {
            return Vec::new();
        };
        let finished = (digest.finished.iter())
            .filter(|item| self.shows(item, bench))
            .map(|item| Happened { done: true, item });
        let failed = (digest.failed.iter())
            .filter(|item| self.shows(item, bench))
            .map(|item| Happened { done: false, item });
        let mut happened: Vec<Happened> = finished.chain(failed).collect();
        happened.sort_by_key(|happened| happened.item.at);
        happened
    }

    /// What agents decided in the project the view shows and nobody overruled.
    pub(super) fn decided_here(&self) -> Vec<&SwarmDecision> {
        let decided = self.digest.iter().flat_map(|digest| digest.decided.iter());
        decided
            .filter(|swarm| Some(swarm.project.as_str()) == self.here.as_deref())
            .collect()
    }

    /// How many of those were decided while the person was away.
    pub(super) fn decided_away(&self) -> usize {
        let Some(digest) = self.digest.as_ref() else {
            return 0;
        };
        let Some(since) = digest.since else {
            return 0;
        };
        let away = |at: u64| at >= since && digest.until.is_none_or(|until| at <= until);
        (self.decided_here().iter())
            .filter(|swarm| away(swarm.decision.learned.at))
            .count()
    }

    /// Answers question `question` with the answer at `choice` among those it takes in one click.
    pub(super) fn answer(&mut self, question: String, choice: usize, cx: &mut Context<Self>) {
        let answered = crate::core::Target::here(&self.asks).request_typed::<call::AnswerQuestion>(
            AnswerParams {
                id: question,
                choice: Some(choice),
                ..AnswerParams::default()
            },
        );
        cx.spawn(async move |pane, cx| {
            if let Err(error) = answered.await {
                let _ = pane.update(cx, |pane, cx| pane.said(&error, cx));
            }
        })
        .detach();
    }

    /// The title of the goal an item is on, when it is on one here.
    fn goal_title(&self, item: &InboxItem) -> Option<String> {
        Some(goal_name(self.goal_of(item.card.as_ref()?)?))
    }
}

/// Home: the calls and the goals, over the composer that starts work.
pub(super) fn home(pane: &mut GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let Some(bench) = pane.bench.upgrade() else {
        return div().into_any_element();
    };
    let composer = (bench.read(cx).composing(START)).map(|start| start.view.clone());
    let list = pane
        .list(cx)
        .cached(StyleRefinement::default().w_full().flex_1().min_h_0());
    let home = div().size_full().relative().flex().flex_col();
    crate::chat::takes_drops(home, START, pane.bench.clone())
        .child(list)
        .children(composer.map(|composer| {
            div()
                .flex_none()
                .px(px(16.))
                .pb(px(8.))
                .on_action(
                    cx.listener(|pane, _: &OlderPrompt, window, cx| pane.recall(true, window, cx)),
                )
                .on_action(
                    cx.listener(|pane, _: &NewerPrompt, window, cx| pane.recall(false, window, cx)),
                )
                .child(composer.into_element().isolated())
        }))
        .when(cx.has_active_drag(), |home| {
            let hint = crate::chat::drop_hint("Drop files or folders to go with the work", cx);
            home.child(gpui_kit::deferred(hint))
        })
        .into_any_element()
}

/// What kind of call something waiting on the person is, in a word or two.
fn call_kind(item: &InboxItem) -> Option<&'static str> {
    if let Some(raised) = &item.raised {
        return Some(match raised.reason {
            RaisedFor::Irreversible => "can't be undone",
            RaisedFor::Money => "money",
            RaisedFor::Taste => "taste",
            RaisedFor::Access => "access",
        });
    }
    match (item.kind, item.need) {
        (InboxKind::Gate, _) if item.gate == Some(Gate::LooksRight) => Some("taste"),
        (InboxKind::Gate, _) if item.environment.is_some() => Some("leaves this Mac"),
        (InboxKind::Ship, _) => Some("leaves this Mac"),
        (InboxKind::Permission, _) => Some("access"),
        (InboxKind::PullRequest, _) => Some("review"),
        (_, _) if item.stalled.is_some() => Some("stalled"),
        (_, Some(InboxNeed::Fix)) => Some("failed"),
        (_, Some(InboxNeed::Unblock)) => Some("unblock"),
        _ => None,
    }
}

/// Who puts the call to the person.
fn call_who(item: &InboxItem) -> Option<String> {
    let by = (item.raised.as_ref()).and_then(|raised| raised.by.as_ref());
    match (by, item.kind, &item.name) {
        (Some(by), ..) => Some(format!("raised by @{by}")),
        (None, InboxKind::Question, Some(name)) => Some(format!("asked by @{name}")),
        (None, _, Some(name)) => Some(format!("@{name}")),
        (None, _, None) => None,
    }
}

/// What the call asks, in full.
fn call_says(item: &InboxItem) -> String {
    match item.kind {
        InboxKind::Question => (item.asked.clone())
            .or_else(|| item.detail.clone())
            .unwrap_or_else(|| item.what.clone()),
        InboxKind::Gate if item.gate == Some(Gate::LooksRight) => match &item.card {
            Some(card) => format!("Does {} look right?", card.title),
            None => "Does it look right?".to_owned(),
        },
        _ => item.what.clone(),
    }
}

impl GoalsPane {
    /// Where a call is from, for Home: the goal the core put it on, else its project.
    pub(super) fn call_from(&self, item: &InboxItem, cx: &App) -> Option<String> {
        let goal = (self.room.view.goals.iter()).find(|row| row.needs.contains(&item.id));
        if let Some(goal) = goal {
            return Some(goal.title.clone());
        }
        if let Some(named) = &item.project_name {
            return Some(named.clone());
        }
        let folder =
            (item.card.as_ref().map(|card| card.project.as_str())).or(item.project.as_deref())?;
        let bench = self.bench.upgrade()?;
        Some(project_name(bench.read(cx), folder))
    }
}

/// One thing only the person can decide, as a card: where it is from, what kind of call it is and
/// who asks, the question in full, what backs it and the answers it takes. On Home a click
/// chooses it for the panel beside the list; on a goal's page, which names the goal, it opens.
pub(super) fn call_card(
    pane: &GoalsPane,
    item: &InboxItem,
    home: bool,
    chosen: bool,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let from = home.then(|| pane.call_from(item, cx)).flatten();
    let meta = [from, call_kind(item).map(str::to_owned), call_who(item)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    let tint = match item.need {
        Some(InboxNeed::Fix) => t.danger,
        _ => t.warn,
    };
    let under = match (item.kind, &item.detail) {
        (InboxKind::Question | InboxKind::Ship, _) => None,
        (_, Some(detail)) if *detail != item.what => Some(detail.clone()),
        _ => None,
    };
    let if_wrong = item.raised.as_ref().map(|raised| match raised.pick {
        Some(_) => format!("If wrong: {}", raised.breaks),
        None => format!("Without it: {}", raised.breaks),
    });
    let assumed = (item.assumption.as_ref())
        .map(|assumption| format!("Goes on assuming {assumption} until you answer."));
    let opened = item.clone();
    let picks = item.id.clone();
    let card = div()
        .id(SharedString::from(format!("call-{}", item.id)))
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .py(SPACE_3)
        .px(SPACE_3H)
        .rounded(RADIUS_3)
        .border_1()
        .border_color(t.border)
        .cursor_pointer()
        .when(chosen, |card| card.bg(t.surface_chosen))
        .when(!chosen, |card| {
            card.hover(move |card| card.bg(t.surface_hover))
        })
        .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
            if !home {
                return pane.open(&opened, cx);
            }
            pane.room.keys.focus(window, cx);
            pane.room.chosen = Some(Chosen::Need(picks.clone()));
            cx.notify();
            if event.click_count() == 2 {
                pane.open(&opened, cx);
            }
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(div().size(px(6.)).flex_none().rounded_full().bg(tint))
                .child(ui::meta(meta, cx).flex_1().min_w_0().truncate())
                .child(ui::meta(ui::time::age(item.at), cx).flex_none()),
        )
        .child(asked(pane, item, cx))
        .children(under.map(|under| ui::meta(under, cx).whitespace_normal()))
        .children(pictures(pane, item, cx))
        .children(item.hold.as_ref().map(|hold| {
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(
                    ui::mono(hold.what.clone(), TEXT_XS, cx)
                        .whitespace_normal()
                        .py(SPACE_1H)
                        .px(SPACE_2)
                        .rounded(RADIUS_2)
                        .bg(t.surface_sunken)
                        .text_color(t.text_secondary),
                )
                .children(
                    hold.why
                        .clone()
                        .map(|why| ui::meta(why, cx).whitespace_normal()),
                )
        }))
        .child(match note_line(pane, item, cx) {
            Some(note) => note.into_any_element(),
            None => div()
                .pt(SPACE_1)
                .flex()
                .flex_wrap()
                .gap(SPACE_1H)
                .children(call_buttons(pane, item, cx))
                .into_any_element(),
        })
        .children(
            (if_wrong.into_iter().chain(assumed))
                .map(|words| ui::meta(words, cx).whitespace_normal()),
        );
    super::with_agent_menu(card, item.agent_id.clone(), &pane.bench)
}

/// What the call asks, in full; a long question shows its first lines, with a way to read the rest.
fn asked(pane: &GoalsPane, item: &InboxItem, cx: &mut Context<GoalsPane>) -> Div {
    let t = Tokens::get(cx).clone();
    let says = call_says(item);
    let long = says.lines().count() > ASKED_LINES || says.chars().count() > ASKED_CHARS;
    let whole = pane.unclamped.as_deref() == Some(item.id.as_str());
    let id = item.id.clone();
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(
            div()
                .text_size(theme::ui_text(TEXT_MD))
                .font_weight(FontWeight::MEDIUM)
                .line_height(relative(1.35))
                .text_color(t.text_primary)
                .when(long && !whole, |text| text.line_clamp(ASKED_LINES))
                .child(says),
        )
        .when(long, |shown| {
            let words = if whole { "Show less" } else { "Show all" };
            shown.child(
                ui::sign_in::link(SharedString::from(format!("asked-{id}")), words, cx).on_click(
                    cx.listener(move |pane, _, _, cx| {
                        cx.stop_propagation();
                        pane.unclamped = (!whole).then(|| id.clone());
                        pane.room.reflow();
                        cx.notify();
                    }),
                ),
            )
        })
}

/// The pictures a goal waiting at Looks right collected, slice by slice with how it was before
/// how it is now. A click shows one full size, or while a note is open pins the note to it.
fn pictures(pane: &GoalsPane, item: &InboxItem, cx: &mut Context<GoalsPane>) -> Option<Div> {
    let look = item
        .look
        .as_ref()
        .filter(|_| item.gate == Some(Gate::LooksRight))?;
    let folder = item.card.as_ref()?.project.clone();
    let shown: Vec<&LookPicture> = (look.slices.iter())
        .flat_map(|slice| {
            let of = move |before: bool| {
                (look.pictures.iter())
                    .filter(move |picture| picture.card == slice.card && picture.before == before)
            };
            of(true).chain(of(false))
        })
        .collect();
    if shown.is_empty() {
        return None;
    }
    let note = (pane.call_note.as_ref()).filter(|note| note.item == item.id);
    let pinned = note.and_then(|note| note.pinned);
    let noting = note.is_some();
    Some(
        div()
            .flex()
            .flex_wrap()
            .gap(SPACE_1H)
            .children(shown.into_iter().map(|picture| {
                let (slice, evidence) = (picture.card, &picture.evidence);
                let spot = (slice, evidence.id);
                let still = match &evidence.proof {
                    Proof::Picture {
                        path,
                        seconds: None,
                        ..
                    } => Some(path.clone()),
                    _ => None,
                };
                let folder = folder.clone();
                div()
                    .id(SharedString::from(format!(
                        "call-picture-{slice}-{}",
                        evidence.id
                    )))
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .cursor_pointer()
                    .child(super::thumb_face(evidence, pinned == Some(spot), cx))
                    .child(ui::meta(
                        if picture.before { "before" } else { "after" },
                        cx,
                    ))
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        cx.stop_propagation();
                        if noting {
                            return pane.pin_call_note(spot, cx);
                        }
                        match &still {
                            Some(path) => pane.show_picture(path, window, cx),
                            None => pane.open_slice(&folder, slice, cx),
                        }
                    }))
            })),
    )
}

/// The note the person writes on a Looks right card about what needs work, while it is open.
pub(super) struct CallNote {
    /// The inbox item whose card it is on.
    item: String,
    /// The picture it is pinned to, by the slice that holds it and its id.
    pinned: Option<(u64, u64)>,
    input: Entity<InputState>,
    _sending: Subscription,
}

impl GoalsPane {
    /// Opens the line on call `item`'s card that says what needs work.
    fn write_call_note(&mut self, item: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("What needs work, for the goal's agents")
        });
        let sending = cx.subscribe_in(&input, window, |pane, _, event, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                pane.send_call_note(cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.call_note = Some(CallNote {
            item: item.to_owned(),
            pinned: None,
            input,
            _sending: sending,
        });
        self.room.reflow();
        cx.notify();
    }

    /// Pins the open note to one picture, or unpins it from the one it is on.
    fn pin_call_note(&mut self, spot: (u64, u64), cx: &mut Context<Self>) {
        if let Some(note) = self.call_note.as_mut() {
            note.pinned = (note.pinned != Some(spot)).then_some(spot);
            cx.notify();
        }
    }

    fn close_call_note(&mut self, cx: &mut Context<Self>) {
        self.call_note = None;
        self.room.reflow();
        cx.notify();
    }

    /// Sends the open note: to the slice holding the picture it is pinned to, else to the goal.
    fn send_call_note(&mut self, cx: &mut Context<Self>) {
        let Some(note) = self.call_note.as_ref() else {
            return;
        };
        let text = note.input.read(cx).value().trim().to_owned();
        let goal = (self.inbox_item(&note.item, cx)).and_then(|item| item.card);
        let (Some(goal), false, false) = (goal, text.is_empty(), self.busy) else {
            return;
        };
        let params = BoardNoteParams {
            project: goal.project,
            card: note.pinned.map_or(goal.id, |(slice, _)| slice),
            text,
            evidence: note.pinned.map(|(_, evidence)| evidence),
        };
        self.ask::<call::BoardNote>(
            params,
            |pane, cx| {
                pane.close_call_note(cx);
                crate::toast::show(&pane.bench, crate::toast::success("Note sent"), cx);
                cx.emit(super::Moved);
            },
            cx,
        );
    }
}

/// The open note on a card, in place of its buttons: the line, what it goes to, Send and Cancel.
fn note_line(
    pane: &GoalsPane,
    item: &InboxItem,
    cx: &mut Context<GoalsPane>,
) -> Option<Stateful<Div>> {
    let note = (pane.call_note.as_ref()).filter(|note| note.item == item.id)?;
    let goes_to = match note.pinned {
        Some(_) => "On the picture you picked; click it again to unpin",
        None => "To the goal's agents. Click a picture to pin the note to it",
    };
    let send = Button::new(
        SharedString::from(format!("note-send-{}", item.id)),
        "Send",
        Variant::Primary,
    )
    .disabled(pane.busy)
    .on_click(cx.listener(|pane, _, _, cx| {
        cx.stop_propagation();
        pane.send_call_note(cx);
    }));
    let cancel = Button::new(
        SharedString::from(format!("note-cancel-{}", item.id)),
        "Cancel",
        Variant::Outline,
    )
    .on_click(cx.listener(|pane, _, _, cx| {
        cx.stop_propagation();
        pane.close_call_note(cx);
    }));
    Some(
        div()
            .id(SharedString::from(format!("note-{}", item.id)))
            .on_click(|_, _, cx| cx.stop_propagation())
            .pt(SPACE_1)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .child(
                div()
                    .capture_key_down(cx.listener(|pane, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            pane.close_call_note(cx);
                        }
                    }))
                    .child(ui::Field::new(&note.input).small()),
            )
            .child(ui::meta(goes_to, cx))
            .child(div().flex().gap(SPACE_1H).child(send).child(cancel)),
    )
}

/// What can be done about a call right here, with one primary at most: passing the gate, the
/// lead's pick, allowing a held action once, or the one way on.
fn call_buttons(
    pane: &GoalsPane,
    item: &InboxItem,
    cx: &mut Context<GoalsPane>,
) -> Vec<AnyElement> {
    if let (InboxKind::Gate, Some(gate), Some(card)) = (item.kind, item.gate, &item.card) {
        let (project, id, environment) = (card.project.clone(), card.id, item.environment.clone());
        let folder = project.clone();
        let looks = gate == Gate::LooksRight;
        let noted = looks.then(|| item.id.clone());
        let pass_words = match (&environment, looks) {
            (_, true) => "Looks right".to_owned(),
            (Some(environment), false) => format!("Pass {environment}"),
            (None, false) => "Approve".to_owned(),
        };
        let pass = Button::new(
            SharedString::from(format!("approve-{}", item.id)),
            pass_words,
            Variant::Primary,
        )
        .on_click(cx.listener(move |pane, _, _, cx| {
            cx.stop_propagation();
            let params = BoardPassParams {
                project: project.clone(),
                card: id,
                gate,
                environment: environment.clone(),
            };
            pane.ask_moved::<call::BoardPass>(params, |_| {}, cx);
        }));
        let other = Button::new(
            SharedString::from(format!("evidence-{}", item.id)),
            if looks {
                "Needs work…"
            } else {
                "See evidence"
            },
            Variant::Outline,
        )
        .on_click(cx.listener(move |pane, _, window, cx| {
            cx.stop_propagation();
            match &noted {
                Some(item) => pane.write_call_note(item, window, cx),
                None => pane.open_slice(&folder, id, cx),
            }
        }));
        return vec![pass.into_any_element(), other.into_any_element()];
    }
    if let Some(hold) = &item.hold {
        let here = crate::core::Target::here(&pane.asks);
        return vec![crate::approvals::ship_buttons(
            hold,
            false,
            here,
            pane.bench.clone(),
        )];
    }
    if item.stalled.is_some() {
        return stall_buttons(item, cx).into_iter().collect();
    }
    if let (Some(question), false) = (&item.question_id, item.answers.is_empty()) {
        let pick = item.pick.as_deref();
        let picked_by = match item.raised {
            Some(_) => "lead's pick",
            None => "agent's pick",
        };
        let mut answers: Vec<(usize, &String)> = item.answers.iter().enumerate().collect();
        answers.sort_by_key(|(_, answer)| Some(answer.as_str()) != pick);
        return answers
            .into_iter()
            .map(|(choice, answer)| {
                let question = question.clone();
                let picked = Some(answer.as_str()) == pick;
                Button::new(
                    SharedString::from(format!("answer-{question}-{choice}")),
                    answer.clone(),
                    if picked {
                        Variant::Primary
                    } else {
                        Variant::Outline
                    },
                )
                .when(picked, |button| {
                    button.trail(ui::text(picked_by, TEXT_2XS, FontWeight::NORMAL).opacity(0.7))
                })
                .on_click(cx.listener(move |pane, _, _, cx| {
                    cx.stop_propagation();
                    pane.answer(question.clone(), choice, cx);
                }))
                .into_any_element()
            })
            .collect();
    }
    let (label, variant) = match item.need {
        _ if item.kind == InboxKind::SeededRules => ("Review", Variant::Primary),
        Some(InboxNeed::Fix) => ("See why", Variant::Primary),
        Some(InboxNeed::Unblock) => ("Unblock", Variant::Primary),
        _ if item.kind == InboxKind::Question => ("Answer", Variant::Primary),
        _ => ("Review", Variant::Primary),
    };
    let opened = item.clone();
    vec![Button::new(
        SharedString::from(format!("act-{}", item.id)),
        label,
        variant,
    )
    .on_click(cx.listener(move |pane, _, _, cx| {
        cx.stop_propagation();
        if opened.kind == InboxKind::SeededRules {
            pane.room.chosen = Some(Chosen::Need(opened.id.clone()));
            return cx.notify();
        }
        pane.open(&opened, cx)
    }))
    .into_any_element()]
}

/// A row whose end drops under what it says when the pane is too narrow for both.
fn wrapping(line: Stateful<Div>) -> Stateful<Div> {
    line.h_auto().min_h(px(34.)).py(px(3.)).flex_wrap()
}

/// What a wrapping row says, which keeps room to be read.
fn row_says() -> Div {
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .flex_1()
        .min_w(ROW_SAYS_MIN_W)
}

/// The end of a wrapping row: what it does, and how it stands.
fn row_does() -> Div {
    div().flex().flex_none().items_center().gap(SPACE_2)
}

/// Starts a stalled owner again, or hands its slice to another agent or a new one.
pub(super) fn stall_buttons(item: &InboxItem, cx: &mut Context<GoalsPane>) -> Option<AnyElement> {
    let stalled = item.stalled.as_ref()?;
    let card = item.card.as_ref()?;
    let (project, id) = (card.project.clone(), card.id);
    let goals = cx.entity().downgrade();
    let hand_to = stalled.hand_to.clone();
    let menu = {
        let project = project.clone();
        move |mut popup: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
            let entries = (hand_to.iter())
                .map(|agent| (format!("@{}", agent.name), Some(agent.agent_id.clone())))
                .chain([("New agent".to_owned(), None)]);
            for (label, to) in entries {
                let (goals, project) = (goals.clone(), project.clone());
                popup = popup.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let (project, to) = (project.clone(), to.clone());
                    let _ = goals.update(cx, |pane, cx| {
                        let launcher = (pane.bench.upgrade())
                            .and_then(|bench| pane.start_launcher(bench.read(cx)))
                            .map(|launcher| launcher.id)
                            .filter(|_| to.is_none());
                        let params = BoardHandOnParams {
                            project,
                            id,
                            to,
                            launcher,
                        };
                        pane.ask_moved::<call::BoardHandOn>(params, |_| {}, cx)
                    });
                }));
            }
            popup
        }
    };
    Some(
        div()
            .flex()
            .gap(SPACE_1)
            .when(stalled.can_resume, |buttons| {
                buttons.child(
                    Button::new(
                        SharedString::from(format!("stalled-resume-{id}")),
                        "Resume",
                        Variant::Outline,
                    )
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        cx.stop_propagation();
                        let params = BoardCardParams {
                            project: project.clone(),
                            id,
                        };
                        pane.ask_moved::<call::BoardResumeOwner>(params, |_| {}, cx)
                    })),
                )
            })
            .child(ui::menu_button(
                SharedString::from(format!("stalled-hand-on-{id}")),
                Anchor::TopRight,
                Button::new(
                    SharedString::from(format!("stalled-hand-on-button-{id}")),
                    "Hand on",
                    Variant::Outline,
                ),
                menu,
            ))
            .into_any_element(),
    )
}

pub(super) fn overnight(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let back = crumb("overnight-back", "Home", cx)
        .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Home, cx)));
    let since = pane.digest.as_ref().and_then(|digest| digest.since);
    let decided: Vec<SwarmDecision> = pane.decided_here().into_iter().cloned().collect();
    if since.is_none() && decided.is_empty() {
        return div()
            .flex()
            .flex_col()
            .child(back)
            .child(empty(
                Some("Nothing yet"),
                "What finishes and fails while you are away shows here once you have been.",
                cx,
            ))
            .into_any_element();
    }
    let Some(bench) = pane.bench.upgrade() else {
        return div().into_any_element();
    };
    let (happened, waiting) = {
        let bench = bench.read(cx);
        (
            pane.happened(bench),
            bench.shown().map_or(0, |view| view.inbox.needed),
        )
    };
    let finished = happened.iter().filter(|happened| happened.done).count();
    let failed = happened.len() - finished;
    let mut groups: Vec<(Option<String>, Vec<Happened>)> = Vec::new();
    for happened in &happened {
        let goal = pane.goal_title(happened.item);
        match groups.iter_mut().find(|(title, _)| *title == goal) {
            Some((_, listed)) => listed.push(*happened),
            None => groups.push((goal, vec![*happened])),
        }
    }
    groups.sort_by_key(|(goal, _)| goal.is_none());
    let number = |value: usize, label: &'static str| {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(
                ui::text(value.to_string(), TEXT_XL, FontWeight::SEMIBOLD)
                    .font_features(theme::tabular())
                    .text_color(t.text_primary),
            )
            .child(ui::meta(label, cx))
    };
    div()
        .flex()
        .flex_col()
        .child(back)
        .child(ui::text("Overnight", TEXT_XL, FontWeight::SEMIBOLD).text_color(t.text_primary))
        .children(since.map(|since| {
            div()
                .pt(SPACE_1H)
                .child(ui::meta(format!("since {}", minute(since)), cx))
        }))
        .child(
            div()
                .pt(SPACE_5)
                .flex()
                .gap(px(40.))
                .child(number(finished, "finished"))
                .child(number(failed, "failed"))
                .child(number(decided.len(), "decided"))
                .child(number(waiting, "wait on you")),
        )
        .when(happened.is_empty() && decided.is_empty(), |view| {
            view.child(
                div()
                    .pt(SPACE_5)
                    .child(ui::meta("Nothing finished or failed here since then.", cx)),
            )
        })
        .children(groups.into_iter().map(|(goal, listed)| {
            div()
                .flex()
                .flex_col()
                .pt(SPACE_5)
                .child(
                    div().pb(SPACE_1H).child(
                        ui::text(
                            goal.unwrap_or_else(|| "Other work".into()),
                            TEXT_SM,
                            FontWeight::SEMIBOLD,
                        )
                        .text_color(t.text_primary),
                    ),
                )
                .children(
                    listed
                        .into_iter()
                        .map(|happened| event_row(pane, happened, cx)),
                )
        }))
        .when(!decided.is_empty(), |view| {
            view.child(
                div()
                    .flex()
                    .flex_col()
                    .pt(SPACE_5)
                    .child(
                        div().pb(SPACE_1H).child(
                            ui::text("Decided without you", TEXT_SM, FontWeight::SEMIBOLD)
                                .text_color(t.text_primary),
                        ),
                    )
                    .children(decided.iter().map(|swarm| decided_row(swarm, cx))),
            )
        })
        .into_any_element()
}

/// Something agents decided while the person was away: when, what, who, and the way to overrule it.
fn decided_row(swarm: &SwarmDecision, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let decision = &swarm.decision;
    let id = format!("{}-{:?}-{}", swarm.project, swarm.goal, decision.id);
    let who = (decision.learned.name.as_ref()).map(|name| format!("@{name}"));
    let overruled = swarm.clone();
    let overrule = Button::new(
        SharedString::from(format!("overrule-{id}")),
        "Overrule",
        Variant::Outline,
    )
    .on_click(cx.listener(move |pane, _, _, cx| {
        cx.stop_propagation();
        let swarm = &overruled;
        pane.overrule(swarm.project.clone(), swarm.goal, &swarm.decision, cx);
    }));
    wrapping(row(SharedString::from(format!("decided-{id}")), cx))
        .child(
            row_says()
                .child(
                    ui::mono(minute(decision.learned.at), TEXT_2XS, cx)
                        .w(px(52.))
                        .flex_none(),
                )
                .child(
                    ui::text(decision.says(), TEXT_SM, FontWeight::MEDIUM)
                        .flex_shrink_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_primary),
                )
                .children(who.map(|who| ui::meta(who, cx).flex_none())),
        )
        .child(row_does().child(overrule))
        .when_some(decision.why.clone(), |line, why| {
            line.child(
                div()
                    .w_full()
                    .pl(px(60.))
                    .child(ui::meta(why, cx).whitespace_normal()),
            )
        })
        .into_any_element()
}

/// One thing that happened: when, what, and what opening it shows.
fn event_row(pane: &GoalsPane, happened: Happened, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Happened { done, item } = happened;
    let on_slice = item.card.as_ref().is_some_and(|inbox| {
        pane.board_of(&inbox.project)
            .and_then(|board| board.cards.iter().find(|card| card.id == inbox.id))
            .is_some_and(|card| card.slice.is_some())
    });
    let opens = match (on_slice, item.agent_id.is_some()) {
        (true, _) => Some("slice"),
        (false, true) => Some("chat"),
        (false, false) => None,
    };
    let subject = (item.card.as_ref().map(|card| card.title.clone()))
        .or_else(|| item.title.clone())
        .or_else(|| item.name.clone());
    let opened = item.clone();
    row(SharedString::from(format!("event-{}", item.id)), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.open(&opened, cx)))
        .child(
            ui::mono(minute(item.at), TEXT_2XS, cx)
                .w(px(52.))
                .flex_none(),
        )
        .child(div().size(px(7.)).flex_none().rounded_full().bg(if done {
            t.live
        } else {
            t.danger
        }))
        .children(subject.map(|subject| {
            ui::text(subject, TEXT_SM, FontWeight::MEDIUM)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary)
        }))
        .child(
            ui::meta(item.what.clone(), cx)
                .flex_shrink_1()
                .min_w_0()
                .truncate(),
        )
        .child(div().flex_1())
        .children(opens.map(|opens| ui::meta(opens, cx)))
        .into_any_element()
}
