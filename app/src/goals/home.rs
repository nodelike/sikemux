//! The pane's first screen, Home: every goal and agent over the composer that starts more; a line
//! for something waiting on the person, on a goal's page; and what happened while the person was
//! away, by goal. The core ranks, sorts and counts them; the pane only groups them under the goal
//! each item is on.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled, Window,
};
use sikemux_core::server::board::BoardCardParams;
use sikemux_core::server::board::HandOnParams as BoardHandOnParams;
use sikemux_core::server::call;
use sikemux_core::server::questions::AnswerParams;
use sikemux_view::inbox::{InboxItem, InboxKind, InboxNeed};

use super::evidence::minute;
use super::goal::{crumb, row};
use super::{empty, goal_name, At, GoalsPane};
use crate::chat::{NewerPrompt, OlderPrompt, START};
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_5, TEXT_2XS, TEXT_SM, TEXT_XL};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// The least a row's words keep before its end drops under them.
const ROW_SAYS_MIN_W: Pixels = px(180.);

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
        Self::waiting(bench, |item| item.kind != InboxKind::Question)
    }

    /// What agents in every project asked the person and wait on, one per agent.
    pub(super) fn questions(bench: &Workbench) -> Vec<InboxItem> {
        Self::waiting(bench, |item| item.kind == InboxKind::Question)
    }

    fn waiting(bench: &Workbench, kind: impl Fn(&InboxItem) -> bool) -> Vec<InboxItem> {
        bench.shown().map_or(Vec::new(), |view| {
            (view.inbox.items.iter())
                .filter(|item| item.need.is_some() && kind(item))
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

    /// Answers question `question` with the answer at `choice` among those it takes in one click.
    pub(super) fn answer(&mut self, question: String, choice: usize, cx: &mut Context<Self>) {
        let answered = crate::core::Target::here(&self.asks).request_typed::<call::AnswerQuestion>(
            AnswerParams {
                id: question,
                answer: None,
                choice: Some(choice),
                choices: None,
                fine: false,
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

/// Home: the numbers, the tabs and the rows, over the composer that starts work.
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

/// What something waiting on the person is, by its mark.
fn need_icon(kind: InboxKind) -> Icon {
    match kind {
        InboxKind::Gate => Icon::Eye,
        InboxKind::Question | InboxKind::Assumption => Icon::Message,
        InboxKind::Permission => Icon::Shield,
        InboxKind::Input => Icon::Terminal,
        InboxKind::PullRequest => Icon::PullRequest,
        InboxKind::Ship => Icon::Shield,
        InboxKind::SeededRules => Icon::Book,
        _ => Icon::Warning,
    }
}

/// Something on one goal's page that waits on the person: an agent's question with its answers,
/// or anything else as its line, without the goal or project the page already names.
pub(super) fn need_line(
    pane: &GoalsPane,
    item: &InboxItem,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    if item.kind == InboxKind::Question {
        return question_row(pane, item, "", cx);
    }
    let slice = (item.card.as_ref())
        .filter(|card| card.goal.is_some() && item.stalled.is_none())
        .map(|card| card.title.clone());
    let about = slice.or_else(|| item.name.as_ref().map(|name| format!("@{name}")));
    needs_row(pane, item, "", about, cx)
}

/// One thing waiting on the person: what it needs, what it is about, its project, and how long it
/// has waited.
fn needs_row(
    pane: &GoalsPane,
    item: &InboxItem,
    project: &str,
    about: Option<String>,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let tint = match item.need {
        Some(InboxNeed::Fix) => t.danger,
        _ => t.warn,
    };
    let opened = item.clone();
    let says = row_says()
        .child(ui::icon(need_icon(item.kind), 14.).text_color(tint))
        .child(
            ui::text(item.what.clone(), TEXT_SM, FontWeight::MEDIUM)
                .flex_shrink_1()
                .when(item.hold.is_some(), |what| what.flex_none())
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .children(
            item.hold
                .as_ref()
                .map(|hold| crate::approvals::held_action(&hold.id, &hold.what, cx)),
        )
        .children(
            about
                .filter(|_| item.heading != item.what)
                .map(|about| ui::meta(about, cx).flex_shrink_1().min_w_0().truncate()),
        )
        .child(
            ui::meta(project.to_owned(), cx)
                .flex_shrink_1()
                .min_w_0()
                .truncate(),
        );
    let does = row_does()
        .children(item.hold.as_ref().map(|hold| {
            let here = crate::core::Target::here(&pane.asks);
            crate::approvals::ship_buttons(&hold.id, false, here, pane.bench.clone())
        }))
        .children(stall_buttons(item, cx))
        .child(ui::meta(ui::time::age(item.at), cx));
    let line = wrapping(row(SharedString::from(format!("need-{}", item.id)), cx))
        .on_click(cx.listener(move |pane, _, _, cx| pane.open(&opened, cx)))
        .child(says)
        .child(does)
        .when_some(
            item.hold.as_ref().and_then(|hold| hold.why.clone()),
            |line, why| {
                line.child(
                    div()
                        .w_full()
                        .pl(px(22.))
                        .child(ui::meta(why, cx).whitespace_normal()),
                )
            },
        );
    super::with_agent_menu(line, item.agent_id.clone(), &pane.bench)
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
                        pane.ask::<call::BoardHandOn>(params, |_| {}, cx)
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
                        pane.ask::<call::BoardResumeOwner>(params, |_| {}, cx)
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

/// What an agent asked the person, on one line with the agent, its project, the answers it takes
/// in one click and how long it has waited; it opens the agent's chat.
fn question_row(
    pane: &GoalsPane,
    item: &InboxItem,
    project: &str,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let tint = match item.need {
        Some(InboxNeed::Unblock) => t.danger,
        _ => t.warn,
    };
    let asked = item.detail.clone().unwrap_or_default();
    let opened = item.clone();
    let answers: Vec<Button> = item.question_id.clone().map_or(Vec::new(), |question| {
        (item.answers.iter().enumerate())
            .map(|(choice, answer)| {
                let question = question.clone();
                Button::new(
                    SharedString::from(format!("answer-{question}-{choice}")),
                    answer.clone(),
                    Variant::Outline,
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    cx.stop_propagation();
                    pane.answer(question.clone(), choice, cx);
                }))
            })
            .collect()
    });
    let says = row_says()
        .child(ui::icon(Icon::Message, 14.).text_color(tint))
        .child(
            ui::text(asked, TEXT_SM, FontWeight::MEDIUM)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .children(item.name.clone().map(|name| ui::meta(name, cx)))
        .child(
            ui::meta(project.to_owned(), cx)
                .flex_shrink_1()
                .min_w_0()
                .truncate(),
        );
    let does = row_does()
        .children(answers)
        .child(ui::meta(ui::time::age(item.at), cx));
    let line = wrapping(row(SharedString::from(format!("question-{}", item.id)), cx))
        .on_click(cx.listener(move |pane, _, _, cx| pane.open(&opened, cx)))
        .child(says)
        .child(does)
        .when_some(
            item.hold.as_ref().and_then(|hold| hold.why.clone()),
            |line, why| {
                line.child(
                    div()
                        .w_full()
                        .pl(px(22.))
                        .child(ui::meta(why, cx).whitespace_normal()),
                )
            },
        );
    super::with_agent_menu(line, item.agent_id.clone(), &pane.bench)
}

pub(super) fn overnight(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let back = crumb("overnight-back", "Home", cx)
        .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Home, cx)));
    let Some(since) = pane.digest.as_ref().and_then(|digest| digest.since) else {
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
    };
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
        .child(
            div()
                .pt(SPACE_1H)
                .child(ui::meta(format!("since {}", minute(since)), cx)),
        )
        .child(
            div()
                .pt(SPACE_5)
                .flex()
                .gap(px(40.))
                .child(number(finished, "finished"))
                .child(number(failed, "failed"))
                .child(number(waiting, "wait on you")),
        )
        .when(happened.is_empty(), |view| {
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
