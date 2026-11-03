//! The pane's first screen: what needs the person, by what for, and the goals with work under
//! way; and what happened while the person was away, by goal. The core ranks, sorts and counts
//! both; the pane only groups them under the goal each item is on.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_core::server::call;
use sikemux_core::server::{AnswerParams, BoardView, CardView};
use sikemux_wire::inbox::{InboxItem, InboxKind, InboxNeed};
use sikemux_wire::slice::SliceState;

use super::evidence::minute;
use super::goal::{count, crumb, row, section, strip};
use super::{empty, goals_of, slices_of, At, GoalsPane};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_4, SPACE_5, TEXT_2XS, TEXT_SM, TEXT_XL, TEXT_XS,
};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// Something that finished or failed while the person was away.
#[derive(Clone, Copy)]
pub(super) struct Happened<'a> {
    done: bool,
    item: &'a InboxItem,
}

const NEEDS: [(InboxNeed, &str); 3] = [
    (InboxNeed::Decide, "Decide"),
    (InboxNeed::Unblock, "Unblock"),
    (InboxNeed::Fix, "Fix"),
];

/// The name of the project at `folder`, or the folder's last part when no project is there.
fn project_name(bench: &Workbench, folder: &str) -> String {
    let named = (bench.projects().iter())
        .find(|project| project.path.to_string_lossy() == folder)
        .map(|project| project.name.clone());
    named.unwrap_or_else(|| {
        let last = folder.rsplit('/').find(|part| !part.is_empty());
        last.unwrap_or(folder).to_owned()
    })
}

/// What Home lists from the inbox and the digest.
#[derive(Default)]
struct Inboxed {
    questions: Vec<(InboxItem, String)>,
    needs: Vec<(InboxItem, String)>,
    happened: usize,
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
    fn questions(bench: &Workbench) -> Vec<InboxItem> {
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
    fn answer(&mut self, question: String, choice: usize, cx: &mut Context<Self>) {
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

    /// What waits on the person and how many things happened while they were away, read from the
    /// window.
    fn read_inbox(&self, cx: &gpui_kit::App) -> Inboxed {
        let Some(bench) = self.bench.upgrade() else {
            return Inboxed::default();
        };
        let bench = bench.read(cx);
        let named = |item: InboxItem| {
            let folder =
                (item.card.as_ref().map(|card| card.project.as_str())).or(item.project.as_deref());
            let project = folder.map(|folder| project_name(bench, folder));
            (item.clone(), project.unwrap_or_default())
        };
        Inboxed {
            questions: Self::questions(bench).into_iter().map(named).collect(),
            needs: Self::needs(bench).into_iter().map(named).collect(),
            happened: self.happened(bench).len(),
        }
    }

    /// The title of the goal an item is on, when it is on one here.
    fn goal_title(&self, item: &InboxItem) -> Option<String> {
        Some(self.goal_of(item.card.as_ref()?)?.title.clone())
    }
}

/// Home, laid out as a new chat opens: an agent asking what is being made in the project, over
/// what needs the person and what is moving, with the composer that starts work at the bottom.
pub(super) fn home(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let mark = pane.start_launcher(cx).map_or_else(
        || ui::mark("claude", 32.),
        |launcher| ui::mark(&launcher.provider, 32.),
    );
    let project = (pane.bench.upgrade())
        .and_then(|bench| Some(bench.read(cx).project()?.name.clone()))
        .unwrap_or_default();
    let welcome = crate::home::welcome_over(mark, project, lists(pane, cx), cx)
        .max_w(px(crate::chat::composer::WIDTH));
    div()
        .size_full()
        .p(px(ui::AGENT_INSET))
        .flex()
        .flex_col()
        .child(
            div()
                .id("goals-home")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    crate::home::centred(welcome)
                        .min_h_full()
                        .py(SPACE_5)
                        .px(SPACE_4),
                ),
        )
        .child(super::start::start_box(pane, cx))
        .into_any_element()
}

/// What needs the person, grouped by what they are needed for, then the goals moving and done.
fn lists(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Inboxed {
        questions,
        needs,
        happened,
    } = pane.read_inbox(cx);
    let needed = needs.len();
    let goals: Vec<(&BoardView, &CardView)> = pane
        .board()
        .into_iter()
        .flat_map(|board| {
            goals_of(&board.cards)
                .into_iter()
                .map(move |goal| (board, goal))
        })
        .collect();
    let (moving, finished): (Vec<_>, Vec<_>) = goals.iter().partition(|(board, goal)| {
        let slices = slices_of(&board.cards, goal.id);
        count(&slices, SliceState::Done) < slices.len()
    });
    div()
        .flex()
        .flex_col()
        .when(happened > 0, |view| {
            view.child(
                div()
                    .id("goals-overnight")
                    .flex()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(t.text_secondary)
                    .hover(move |style| style.text_color(t.text_primary))
                    .child(ui::text(
                        format!("{happened} happened while you were away"),
                        TEXT_2XS,
                        FontWeight::NORMAL,
                    ))
                    .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Overnight, cx))),
            )
        })
        .when(!questions.is_empty(), |view| {
            view.child(section("Waiting on you", questions.len(), cx))
                .children((questions.iter()).map(|(item, project)| question_row(item, project, cx)))
        })
        .when(!needs.is_empty(), |view| {
            view.child(section("Needs you", needed, cx))
                .children(NEEDS.iter().filter_map(|(need, name)| {
                    let listed: Vec<&(InboxItem, String)> = needs
                        .iter()
                        .filter(|(item, _)| item.need == Some(*need))
                        .collect();
                    (!listed.is_empty()).then(|| {
                        div()
                            .flex()
                            .flex_col()
                            .pt(SPACE_2)
                            .child(ui::caps(name, cx))
                            .children(
                                (listed.into_iter())
                                    .map(|(item, project)| needs_row(pane, item, project, cx)),
                            )
                    })
                }))
        })
        .children(super::rules::offer(pane, false, cx).map(|offer| {
            div()
                .flex()
                .flex_col()
                .child(section("Rules from past sessions", pane.offered(), cx))
                .child(offer)
        }))
        .when(!moving.is_empty(), |view| {
            view.child(section("Moving", moving.len(), cx))
                .children(moving.iter().map(|(board, goal)| goal_row(board, goal, cx)))
        })
        .when(!finished.is_empty(), |view| {
            view.child(section("Done", finished.len(), cx)).children(
                finished
                    .iter()
                    .map(|(board, goal)| goal_row(board, goal, cx)),
            )
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

/// One thing waiting on the person: what it needs, the goal or agent it is about, its project,
/// and how long it has waited.
fn needs_row(
    pane: &GoalsPane,
    item: &InboxItem,
    project: &str,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let about = pane.goal_title(item).or_else(|| item.name.clone());
    let tint = match item.need {
        Some(InboxNeed::Fix) => t.danger,
        _ => t.warn,
    };
    let opened = item.clone();
    row(SharedString::from(format!("need-{}", item.id)), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.open(&opened, cx)))
        .child(ui::icon(need_icon(item.kind), 14.).text_color(tint))
        .child(
            ui::text(item.what.clone(), TEXT_SM, FontWeight::MEDIUM)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_primary),
        )
        .children(
            item.hold
                .as_ref()
                .map(|hold| held_action(&hold.id, &hold.what, cx)),
        )
        .children(about.map(|about| ui::meta(about, cx).flex_shrink_1().min_w_0().truncate()))
        .child(
            ui::meta(project.to_owned(), cx)
                .flex_shrink_1()
                .min_w_0()
                .truncate(),
        )
        .child(div().flex_1())
        .children(item.hold.as_ref().map(|hold| ship_buttons(&hold.id, cx)))
        .child(ui::meta(ui::time::age(item.at), cx))
        .into_any_element()
}

/// What an agent asked the person, on one line with the agent, its project, the answers it takes
/// in one click and how long it has waited; it opens the agent's chat.
fn question_row(item: &InboxItem, project: &str, cx: &mut Context<GoalsPane>) -> AnyElement {
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
    row(SharedString::from(format!("question-{}", item.id)), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.open(&opened, cx)))
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
        )
        .child(div().flex_1())
        .children(answers)
        .child(ui::meta(ui::time::age(item.at), cx))
        .into_any_element()
}

/// The exact command or call a held action runs, which allowing it lets through once: clipped
/// on the row, whole on hover.
fn held_action(id: &str, what: &str, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx);
    ui::mono(what.to_owned(), TEXT_XS, cx)
        .id(SharedString::from(format!("held-action-{id}")))
        .flex_shrink_1()
        .min_w_0()
        .truncate()
        .text_color(t.text_secondary)
        .tooltip(ui::tooltip(what.to_owned()))
        .into_any_element()
}

/// Allows a held action once, or refuses it, without opening the agent.
fn ship_buttons(hold: &str, cx: &mut Context<GoalsPane>) -> AnyElement {
    let (allow, refuse) = (hold.to_owned(), hold.to_owned());
    div()
        .flex()
        .gap(SPACE_1)
        .child(
            Button::new(
                SharedString::from(format!("ship-allow-{hold}")),
                "Allow once",
                Variant::Outline,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                cx.stop_propagation();
                pane.answer_ship::<call::AllowShip>(allow.clone(), cx)
            })),
        )
        .child(
            Button::new(
                SharedString::from(format!("ship-refuse-{hold}")),
                "Refuse",
                Variant::Outline,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                cx.stop_propagation();
                pane.answer_ship::<call::RefuseShip>(refuse.clone(), cx)
            })),
        )
        .into_any_element()
}

/// A goal on one line: its title, how many slices are done, what comes next and
/// what that waits on, and its strip. It opens the goal, or a lone slice's worker's chat.
fn goal_row(board: &BoardView, goal: &CardView, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = goal.id;
    let slices = slices_of(&board.cards, id);
    let done = count(&slices, SliceState::Done);
    let worker = goal.slice.as_ref().and(goal.owner.clone());
    row(
        SharedString::from(format!("goal-{}-{id}", board.project)),
        cx,
    )
    .on_click(cx.listener(move |pane, _, _, cx| match &worker {
        Some(agent) => {
            let _ = pane
                .bench
                .update(cx, |bench, cx| bench.open_agent(agent, cx));
        }
        None => pane.go(At::Goal(id), cx),
    }))
    .child(
        ui::text(goal.title.clone(), TEXT_SM, FontWeight::MEDIUM)
            .flex_shrink_1()
            .min_w_0()
            .truncate()
            .text_color(t.text_primary),
    )
    .child(ui::meta(format!("{done} of {} done", slices.len()), cx))
    .children(
        (goal.next.first())
            .and_then(|next| super::goal::step(&board.cards, next))
            .map(|next| ui::meta(next, cx).flex_shrink_1().min_w_0().truncate()),
    )
    .child(div().flex_1())
    .child(strip(&slices, px(120.), cx))
    .into_any_element()
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
