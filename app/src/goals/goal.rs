//! One goal: who leads it, what needs the person, what happened and what was decided, what runs
//! and comes next. Under that, quieter and only where a goal has them, its slices and assertions,
//! how the project's slices land and the rules its agents follow.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_core::server::board::BoardCard;
use sikemux_core::server::board::BoardSettingsParams;
use sikemux_core::server::call;
use sikemux_view::slice::{Gate, NextGate, Passer, SliceState, SliceView};

use super::{
    eta_words, goal_name, state_count, state_mark, wait_words, waiting_words, At, GoalsPane,
};
use crate::board::panels::{landing_choice, landing_of, landing_picks};
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_5, SPACE_6, TEXT_2XS, TEXT_SM,
    TEXT_XL, TEXT_XS,
};
use crate::ui;

const STATES: [SliceState; 5] = [
    SliceState::Done,
    SliceState::Moving,
    SliceState::NeedsYou,
    SliceState::Failed,
    SliceState::Waiting,
];

/// Goal `id`: its title and team, what needs the person, what happened, what was decided, what
/// runs and comes next, and then whatever slices and assertions it has.
pub(super) fn goal(pane: &GoalsPane, id: u64, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(goal) = pane.card(id) else {
        return div().into_any_element();
    };
    let slices = pane.slices(id);
    let needs_you = pane.needs_of(goal, cx);
    let kept_assertions = goal
        .assertions
        .iter()
        .any(|assertion| assertion.retired.is_none());
    let below = !slices.is_empty() || kept_assertions;
    div()
        .flex()
        .flex_col()
        .child(
            crumb("goals-back", "Home", cx)
                .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Home, cx))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    ui::text(goal_name(goal), TEXT_XL, FontWeight::SEMIBOLD)
                        .flex_shrink_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_primary),
                )
                .child(div().flex_1())
                .when_some(goal.ticket.as_ref(), |head, ticket| {
                    head.child(crate::board::ticket_line(ticket, cx))
                })
                .child(super::on_board(goal.id, cx)),
        )
        .child(team(goal, &slices, cx))
        .when(!needs_you.is_empty(), |view| {
            view.child(heading("Needs you", Some(needs_you.len()), None, cx))
                .children((needs_you.iter()).map(|item| super::home::need_line(pane, item, cx)))
        })
        .children(super::happened::what_happened(pane, id, cx))
        .children(super::happened::decided(pane, goal, cx))
        .children(running_and_next(pane, goal, &slices, cx))
        .when(below, |view| view.child(quieter(pane, goal, &slices, cx)))
        .into_any_element()
}

/// What the goal also keeps: its slices and assertions, how they land and the rules its agents
/// follow, below what the person reads first.
fn quieter(
    pane: &GoalsPane,
    goal: &BoardCard,
    slices: &[(&BoardCard, &SliceView)],
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let counts = STATES
        .iter()
        .map(|state| (count(slices, *state), *state))
        .filter(|(count, _)| *count > 0)
        .map(|(count, state)| state_count(count, state))
        .collect::<Vec<_>>()
        .join(" · ");
    div()
        .mt(SPACE_6)
        .flex()
        .flex_col()
        .when(!slices.is_empty(), |view| {
            view.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .child(strip(slices, gpui_kit::relative(1.), cx))
                    .child(ui::meta(counts, cx)),
            )
        })
        .child(super::assertions::section_of(pane, goal, cx))
        .when(!slices.is_empty(), |view| {
            view.child(quiet_section("Slices", slices.len(), cx))
                .children(
                    slices
                        .iter()
                        .map(|(card, slice)| slice_row(pane, card, slice, cx)),
                )
        })
        .children(landing(pane, cx))
        .child(super::rules::rules(pane, goal, cx))
        .into_any_element()
}

/// One line of what runs or comes next, with its mark.
fn running_line(mark: AnyElement, words: String, cx: &Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .py(SPACE_1)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(theme::ui_text(TEXT_SM))
                .text_color(t.text_secondary)
                .child(words),
        )
        .into_any_element()
}

/// What moves now and what it waits on, then the gates its slices reach next in order; nothing
/// when nothing runs and nothing is next.
fn running_and_next(
    pane: &GoalsPane,
    goal: &BoardCard,
    slices: &[(&BoardCard, &SliceView)],
    cx: &mut Context<GoalsPane>,
) -> Option<AnyElement> {
    let mut lines: Vec<AnyElement> = Vec::new();
    if let Some(name) = pane.working_on(goal) {
        lines.push(running_line(
            state_mark(SliceState::Moving, cx),
            format!("{name} is working"),
            cx,
        ));
    }
    for (card, slice) in slices
        .iter()
        .filter(|(_, slice)| slice.state == SliceState::Moving)
    {
        let words = match waiting_words(card, slice) {
            Some(waits) => format!("{}: {waits}", card.title),
            None => card.title.clone(),
        };
        lines.push(running_line(state_mark(SliceState::Moving, cx), words, cx));
    }
    let steps: Vec<String> = goal
        .next
        .iter()
        .take(3)
        .filter_map(|next| step(pane.cards(), next))
        .collect();
    for step in steps {
        lines.push(running_line(state_mark(SliceState::Waiting, cx), step, cx));
    }
    let slip = goal.eta.as_ref().and_then(|eta| {
        let slip = eta.slip?.words();
        Some(match eta.at {
            Some(_) => format!(
                "ETA slipped: {slip}, first said {}",
                ui::time::clock(eta.promised)
            ),
            None => format!("ETA slipped: {slip}"),
        })
    });
    let t = Tokens::get(cx).clone();
    (!lines.is_empty()).then(|| {
        let eta = goal.eta.as_ref().and_then(eta_words);
        div()
            .flex()
            .flex_col()
            .child(heading("Running and next", None, eta, cx))
            .children(lines)
            .children(slip.map(|slip| ui::meta(slip, cx).text_color(t.warn)))
            .into_any_element()
    })
}

/// The name over a part of the goal page: spaced capitals on a hairline, with how many it holds
/// and something about it at the far end.
pub(super) fn heading(
    title: &str,
    count: Option<usize>,
    about: Option<String>,
    cx: &Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx);
    let title = match count {
        Some(count) => format!("{title} · {count}"),
        None => title.to_owned(),
    };
    div()
        .mt(SPACE_6)
        .mb(SPACE_1H)
        .pb(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .border_b_1()
        .border_color(t.border)
        .child(ui::caps(&title, cx))
        .child(div().flex_1())
        .children(about.map(|about| ui::meta(about, cx)))
        .into_any_element()
}

/// Who passes Landed in the project, which the person sets here as on the board.
fn landing(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> Option<AnyElement> {
    let project = pane.here.clone()?;
    let (key, shown) = landing_choice(pane.board_of(&project)?.settings.landing);
    let entity = cx.entity();
    let pick = ui::select(
        "goals-landing",
        shown,
        landing_picks(),
        Some(key.into()),
        pane.busy,
        move |picked, _, cx| {
            let params = BoardSettingsParams {
                project: project.clone(),
                landing: Some(landing_of(picked.as_ref())),
                ..BoardSettingsParams::default()
            };
            entity.update(cx, |pane, cx| {
                pane.ask_moved::<call::BoardSettings>(params, |_| {}, cx)
            })
        },
        cx,
    );
    Some(
        div()
            .pt(SPACE_5)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(ui::meta("Slices land", cx))
            .child(pick)
            .into_any_element(),
    )
}

pub(super) fn count(slices: &[(&BoardCard, &SliceView)], state: SliceState) -> usize {
    slices
        .iter()
        .filter(|(_, slice)| slice.state == state)
        .count()
}

/// The goal's lead, then the agents that own its slices.
fn team(
    goal: &BoardCard,
    slices: &[(&BoardCard, &SliceView)],
    cx: &Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx);
    let mut owners: Vec<&BoardCard> = Vec::new();
    for (card, _) in slices {
        let other = card.owner.is_some() && card.owner != goal.owner;
        if other && !owners.iter().any(|known| known.owner == card.owner) {
            owners.push(card);
        }
    }
    let names = owners
        .iter()
        .filter_map(|card| card.owner_name.clone())
        .collect::<Vec<_>>();
    let marks = goal
        .owner_cli
        .iter()
        .chain(owners.iter().filter_map(|card| card.owner_cli.as_ref()))
        .map(|cli| ui::mark(cli, 12.));
    div()
        .pt(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(div().flex().gap(SPACE_0).children(marks))
        .when_some(
            goal.owner_name.clone().zip(goal.owner.clone()),
            |line, (lead, agent)| {
                line.child(
                    div()
                        .flex()
                        .gap(px(3.))
                        .child(
                            div()
                                .id("goal-lead")
                                .cursor_pointer()
                                .text_color(t.text_secondary)
                                .hover(|name| name.underline())
                                .on_click(
                                    cx.listener(move |pane, _, _, cx| pane.open_agent(&agent, cx)),
                                )
                                .child(ui::text(lead, TEXT_2XS, FontWeight::MEDIUM)),
                        )
                        .child(ui::meta("leads", cx)),
                )
            },
        )
        .when(!names.is_empty(), |line| {
            line.child(ui::meta(format!("with {}", names.join(", ")), cx))
        })
        .into_any_element()
}

/// One segment per slice, in the colour of its state.
pub(super) fn strip(
    slices: &[(&BoardCard, &SliceView)],
    width: impl Into<gpui_kit::Length>,
    cx: &Context<GoalsPane>,
) -> AnyElement {
    super::state_strip(slices.iter().map(|(_, slice)| slice.state), width, cx)
}

/// A gate a slice waits at, as a short sentence: who does what to which slice.
pub(super) fn step(cards: &[BoardCard], next: &NextGate) -> Option<String> {
    let card = cards.iter().find(|card| card.id == next.card)?;
    Some(step_words(next, &card.title, card.owner_name.as_deref()))
}

/// [`step`], of a slice titled `title` that `owner` holds.
pub(super) fn step_words(next: &NextGate, title: &str, owner: Option<&str>) -> String {
    let you = next.passer == Passer::Person;
    let verb = match (next.gate, you) {
        (Gate::Built, true) => "build",
        (Gate::Built, false) => "builds",
        (Gate::LooksRight, true) => "look at",
        (Gate::LooksRight, false) => "looks at",
        (Gate::Landed, true) => "land",
        (Gate::Landed, false) => "lands",
    };
    let who = match next.passer {
        Passer::Person => "you".to_owned(),
        Passer::Core => "Sikemux".to_owned(),
        Passer::Owner => match owner {
            Some(owner) => format!("@{owner}"),
            None => return format!("{title} waits for an owner"),
        },
    };
    let step = match &next.environment {
        Some(environment) if you => format!("you pass {environment} on {title}"),
        Some(environment) => format!("{who} deploys {title} to {environment}"),
        None => format!("{who} {verb} {title}"),
    };
    match next.wait.as_ref().and_then(|wait| wait_words(wait, owner)) {
        Some(wait) => format!("{step} ({wait})"),
        None => step,
    }
}

/// One slice: its state, title and what it waits on, then its ticket and owner.
fn slice_row(
    pane: &GoalsPane,
    card: &BoardCard,
    slice: &SliceView,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    let line = row(SharedString::from(format!("slice-{id}")), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.go(At::Slice(id), cx)))
        .child(state_mark(slice.state, cx))
        .child(
            ui::text(card.title.clone(), TEXT_XS, FontWeight::NORMAL)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_secondary),
        )
        .children(
            waiting_words(card, slice)
                .map(|words| ui::meta(words, cx).flex_shrink_1().min_w_0().truncate()),
        )
        .child(div().flex_1())
        .when_some(card.ticket.as_ref(), |row, ticket| {
            row.child(ui::mono(ticket.key.clone(), TEXT_2XS, cx))
        })
        .when(card.owner.is_some(), |row| {
            row.child(crate::board::owner(card, cx))
        });
    super::with_agent_menu(line, card.owner.clone(), &pane.bench)
}

/// A line of a list that opens what it names.
pub(super) fn row(id: SharedString, cx: &Context<GoalsPane>) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    div()
        .id(id)
        .h(px(34.))
        .px(SPACE_2)
        .mx(-SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .hover(move |style| style.bg(t.surface_hover))
}

/// A part of the goal page below what is read first, named more quietly than it.
pub(super) fn quiet_section(title: &str, count: usize, cx: &Context<GoalsPane>) -> AnyElement {
    div()
        .pt(SPACE_5)
        .pb(SPACE_1)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(ui::caps(title, cx))
        .child(ui::meta(count.to_string(), cx))
        .into_any_element()
}

/// A step back up, as `Name /` over a title.
pub(super) fn crumb(
    id: &'static str,
    name: impl Into<SharedString>,
    cx: &Context<GoalsPane>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Tokens::get(cx).clone();
    div()
        .id(id)
        .pb(SPACE_1)
        .flex()
        .gap(SPACE_1)
        .cursor_pointer()
        .text_color(t.text_tertiary)
        .hover(move |style| style.text_color(t.text_secondary))
        .child(
            ui::text(name, TEXT_2XS, FontWeight::NORMAL)
                .min_w_0()
                .truncate(),
        )
        .child(ui::text("/", TEXT_2XS, FontWeight::NORMAL).flex_none())
}
