//! One goal: who leads it, what happened and what was decided, the calls only the person can make
//! on it, what runs and comes next, and its slices. Its assertions, how the project's slices land
//! and the rules its agents follow fold under one quiet Details line.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_core::server::board::BoardCard;
use sikemux_core::server::board::BoardSettingsParams;
use sikemux_core::server::call;
use sikemux_view::slice::{Gate, GoalEta, NextGate, Passer, SliceState, SliceView, WaitOn};

use super::{goal_name, stands_words, state_mark, wait_eta, wait_words, At, GoalsPane};
use crate::board::panels::{landing_choice, landing_of, landing_picks};
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_5, SPACE_6, TEXT_2XS,
    TEXT_SM, TEXT_XL, TEXT_XS,
};
use crate::ui;

/// Goal `id`: its name and when it should be through, who leads it, what happened and what was
/// decided beside its slices, its calls, and folded under them the rest.
pub(super) fn goal(pane: &GoalsPane, id: u64, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(goal) = pane.card(id) else {
        return div().into_any_element();
    };
    let slices = pane.slices(id);
    let calls = pane.needs_of(goal, cx);
    let project = (pane.here.as_deref())
        .zip(pane.bench.upgrade())
        .map(|(folder, bench)| super::home::project_name(bench.read(cx), folder));
    let account = div()
        .flex()
        .flex_col()
        .flex_grow(1.)
        .flex_basis(px(380.))
        .min_w_0()
        .children(super::happened::what_happened(pane, id, cx))
        .children(super::happened::decided(pane, goal, cx));
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    crumb("goals-back", "Home", cx)
                        .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Home, cx))),
                )
                .children(project.map(|project| ui::meta(project, cx).pl(SPACE_1).pb(SPACE_1))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2H)
                .child(
                    ui::text(goal_name(goal), TEXT_XL, FontWeight::SEMIBOLD)
                        .flex_shrink_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_primary),
                )
                .children(goal.eta.as_ref().and_then(|eta| through_pill(eta, cx)))
                .child(div().flex_1())
                .when_some(goal.ticket.as_ref(), |head, ticket| {
                    head.child(crate::board::ticket_line(ticket, cx))
                })
                .child(super::on_board(goal.id, cx)),
        )
        .child(byline(goal, &slices, cx))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(px(28.))
                .child(account)
                .when(!slices.is_empty(), |columns| {
                    columns.child(slices_part(pane, &slices, cx))
                }),
        )
        .when(!calls.is_empty(), |view| {
            view.child(heading("Your calls", Some(calls.len()), None, cx))
                .child(div().flex().flex_col().gap(SPACE_2).children(
                    (calls.iter()).map(|item| super::home::call_card(pane, item, false, false, cx)),
                ))
        })
        .child(details(pane, goal, cx))
        .into_any_element()
}

/// When the goal should be through, as a pill beside its name, such as "through by 16:20, then
/// you"; when it was due once the core has no estimate left.
fn through_pill(eta: &GoalEta, cx: &Context<GoalsPane>) -> Option<AnyElement> {
    let t = Tokens::get(cx);
    let parts = match eta.at {
        Some(at) => {
            let mut parts = vec![
                ("through by ".to_owned(), false),
                (ui::time::clock(at), true),
            ];
            if eta.then_you {
                parts.push((", then you".to_owned(), false));
            }
            parts
        }
        None if eta.slip.is_some() => vec![
            ("was due ".to_owned(), false),
            (ui::time::clock(eta.promised), true),
        ],
        None => return None,
    };
    let strong = gpui_kit::HighlightStyle {
        color: Some(t.text_primary),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..gpui_kit::HighlightStyle::default()
    };
    Some(
        div()
            .flex_none()
            .px(SPACE_2H)
            .py(px(3.))
            .rounded_full()
            .border_1()
            .border_color(t.border)
            .bg(t.surface_sunken)
            .text_size(theme::ui_text(TEXT_XS))
            .font_features(theme::tabular())
            .text_color(t.text_secondary)
            .child(super::emphasised(parts, strong))
            .into_any_element(),
    )
}

/// The goal's slices, a line each with its mark and where it stands.
fn slices_part(
    pane: &GoalsPane,
    slices: &[(&BoardCard, &SliceView)],
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_grow(1.)
        .flex_basis(px(260.))
        .min_w(px(240.))
        .child(heading("Slices", Some(slices.len()), None, cx))
        .children(
            slices
                .iter()
                .map(|(card, slice)| slice_row(pane, card, slice, cx)),
        )
        .into_any_element()
}

/// What the goal also keeps, folded under one quiet line: its assertions, how the project's
/// slices land and the rules its agents follow.
fn details(pane: &GoalsPane, goal: &BoardCard, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let open = pane.details;
    div()
        .mt(SPACE_6)
        .flex()
        .flex_col()
        .child(
            div()
                .id("goal-details")
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .py(SPACE_1)
                .cursor_pointer()
                .text_color(t.text_tertiary)
                .hover(move |line| line.text_color(t.text_secondary))
                .child(crate::motion::turn(
                    "goal-details-chevron",
                    crate::motion::fold_angle(open),
                    crate::motion::DETAIL_FOLD,
                    ui::icon(crate::icons::Icon::Chevron, 10.).text_color(t.text_tertiary),
                ))
                .child(ui::text("Details", TEXT_XS, FontWeight::NORMAL))
                .on_click(cx.listener(|pane, _, _, cx| {
                    pane.details = !pane.details;
                    cx.notify();
                })),
        )
        .when(open, |view| {
            view.child(super::assertions::section_of(pane, goal, cx))
                .children(landing(pane, cx))
                .child(super::rules::rules(pane, goal, cx))
        })
        .into_any_element()
}

/// The name over a part of the goal page: spaced capitals with how many it holds, a hairline on
/// to something about it at the far end.
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
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(ui::caps(&title, cx))
        .child(div().flex_1().h(px(1.)).bg(t.line))
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

/// Who leads the goal, with the agents that own its slices, how many slices it has and when it
/// started; then why it runs late, when it does.
fn byline(
    goal: &BoardCard,
    slices: &[(&BoardCard, &SliceView)],
    cx: &Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let mut owners: Vec<&BoardCard> = Vec::new();
    for (card, _) in slices {
        let other = card.owner.is_some() && card.owner != goal.owner;
        if other && !owners.iter().any(|known| known.owner == card.owner) {
            owners.push(card);
        }
    }
    let names = owners
        .iter()
        .filter_map(|card| card.owner_name.as_ref().map(|name| format!("@{name}")))
        .collect::<Vec<_>>();
    let marks = goal
        .owner_cli
        .iter()
        .chain(owners.iter().filter_map(|card| card.owner_cli.as_ref()))
        .map(|cli| ui::mark(cli, 14.));
    let mut about = Vec::new();
    if !names.is_empty() {
        about.push(format!("with {}", names.join(", ")));
    }
    match slices.len() {
        0 if goal.goal_state == Some(SliceState::Moving) => about.push("at work now".to_owned()),
        0 => {}
        1 => about.push("1 slice".to_owned()),
        many => about.push(format!("{many} slices")),
    }
    about.push(format!("started {}", ui::time::clock(goal.at)));
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
    div()
        .pt(SPACE_1H)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(SPACE_1H)
                .child(div().flex().gap(SPACE_0).children(marks))
                .when_some(
                    goal.owner_name.clone().zip(goal.owner.clone()),
                    |line, (lead, agent)| {
                        line.child(
                            div()
                                .id("goal-lead")
                                .cursor_pointer()
                                .text_color(t.text_secondary)
                                .hover(|name| name.underline())
                                .on_click(
                                    cx.listener(move |pane, _, _, cx| pane.open_agent(&agent, cx)),
                                )
                                .child(ui::text(format!("@{lead}"), TEXT_SM, FontWeight::MEDIUM)),
                        )
                        .child(
                            ui::text("leads", TEXT_SM, FontWeight::NORMAL)
                                .text_color(t.text_tertiary),
                        )
                    },
                )
                .child(
                    ui::text(
                        format!("· {}", about.join(" · ")),
                        TEXT_SM,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_tertiary),
                ),
        )
        .children(slip.map(|slip| ui::meta(slip, cx).text_color(t.warn)))
        .into_any_element()
}

/// A gate a slice titled `title` that `owner` holds waits at, as a short sentence: who does what
/// to it.
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
    let wait = next
        .wait
        .as_ref()
        .and_then(|wait| match (&wait.on, next.passer) {
            (WaitOn::Agent { .. }, Passer::Owner) => wait_eta(wait),
            _ => wait_words(wait, owner),
        });
    match wait {
        Some(wait) => format!("{step} ({wait})"),
        None => step,
    }
}

/// One slice: its mark and title, then its ticket and where it stands in a word or two, which the
/// title gives way to.
fn slice_row(
    pane: &GoalsPane,
    card: &BoardCard,
    slice: &SliceView,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    let stands = stands_words(card, slice);
    let line = row(SharedString::from(format!("slice-{id}")), cx)
        .on_click(cx.listener(move |pane, _, _, cx| pane.go(At::Slice(id), cx)))
        .child(state_mark(slice.state, cx))
        .child(
            ui::text(card.title.clone(), TEXT_SM, FontWeight::NORMAL)
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(if slice.state == SliceState::Done {
                    t.text_secondary
                } else {
                    t.text_primary
                }),
        )
        .child(div().flex_1())
        .when_some(card.ticket.as_ref(), |row, ticket| {
            row.child(ui::mono(ticket.key.clone(), TEXT_2XS, cx))
        })
        .child(
            ui::text(stands, TEXT_XS, FontWeight::NORMAL)
                .flex_none()
                .font_features(theme::tabular())
                .text_color(t.text_tertiary),
        );
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
