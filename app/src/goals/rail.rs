//! The goals view's own rail: Home with how many things need the person, Overnight with how many
//! happened while they were away, every project's goals under its name, each with its state's
//! mark, and the hosts at the bottom with how many agents each runs.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled,
};
use sikemux_wire::slice::SliceState;

use super::{goals_of, state_mark, At, GoalsPane};
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1, SPACE_2H, SPACE_4};
use crate::ui::{self, Group, Row, Tone};

const WIDTH: f32 = 240.;

/// A project's name, as the last part of its folder.
pub(super) fn project_name(folder: &str) -> &str {
    folder
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(folder)
}

pub(super) fn rail(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let needed = pane.needed(cx);
    let happened = pane.happened().len();
    let count = |count: usize| (count > 0).then(|| ui::meta(count.to_string(), cx));
    let place = |id: &'static str, title: &'static str, icon: Icon, at: At, count| {
        Row::new(id, title)
            .mark(16., ui::icon(icon, 14.).text_color(t.text_secondary))
            .chosen(pane.at == at)
            .status(count)
            .on_click(cx.listener(move |pane, _, _, cx| pane.go(at, cx)))
    };
    let projects: Vec<AnyElement> =
        pane.boards()
            .iter()
            .filter(|board| !goals_of(&board.cards).is_empty())
            .map(|board| {
                let goals = goals_of(&board.cards).into_iter().map(|goal| {
                    let (project, id) = (board.project.clone(), goal.id);
                    let open = pane.here.as_deref() == Some(board.project.as_str())
                        && matches!(pane.at, At::Goal(at) | At::Slice(at) if at == id
                        || pane.card(at).is_some_and(|card| card.parent == Some(id)));
                    Row::new(
                        SharedString::from(format!("rail-goal-{}-{id}", board.project)),
                        goal.title.clone(),
                    )
                    .mark(
                        16.,
                        state_mark(goal.goal_state.unwrap_or(SliceState::Waiting), cx),
                    )
                    .chosen(open)
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        pane.go_to(project.clone(), At::Goal(id), cx)
                    }))
                    .into_any_element()
                });
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .pt(SPACE_2H)
                            .pb(SPACE_1)
                            .pl(px(8.))
                            .child(ui::meta(project_name(&board.project).to_owned(), cx)),
                    )
                    .children(goals)
                    .into_any_element()
            })
            .collect();
    ui::panel(cx)
        .w(px(WIDTH))
        .flex_none()
        .h_full()
        .child(
            div()
                .id("goals-rail-scroll")
                .flex_1()
                .min_h_0()
                .pt(SPACE_2H)
                .px(SPACE_2H)
                .pb(SPACE_4)
                .overflow_y_scroll()
                .child(place(
                    "goals-home",
                    "Home",
                    Icon::Inbox,
                    At::Home,
                    count(needed),
                ))
                .child(place(
                    "goals-overnight",
                    "Overnight",
                    Icon::Clock,
                    At::Overnight,
                    count(happened),
                ))
                .child(Group::new("Goals", "no goals yet").children(projects)),
        )
        .child(hosts(pane, cx))
        .into_any_element()
}

/// This host and the account's other hosts, each with how many agents it runs.
fn hosts(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let listed: Vec<(SharedString, usize)> = pane
        .bench
        .upgrade()
        .map(|bench| {
            let bench = bench.read(cx);
            let here = bench
                .view
                .as_ref()
                .map_or(0, |view| view.workspace.agents.len());
            let others = bench
                .hosts
                .views
                .iter()
                .map(|(key, view)| (bench.host_name(key), view.workspace.agents.len()));
            std::iter::once((SharedString::from("This host"), here))
                .chain(others)
                .collect()
        })
        .unwrap_or_default();
    div()
        .flex_none()
        .px(SPACE_2H)
        .py(SPACE_2H)
        .border_t_1()
        .border_color(t.border)
        .children(listed.into_iter().map(|(name, agents)| {
            Row::new(SharedString::from(format!("goals-host-{name}")), name)
                .mark(
                    16.,
                    ui::icon(Icon::Laptop, 14.).text_color(t.text_secondary),
                )
                .quiet()
                .when(agents > 0, |row| {
                    row.detail(format!(
                        "{agents} {}",
                        if agents == 1 { "agent" } else { "agents" }
                    ))
                })
                .status(Some(ui::dot(Tone::Ok, cx)))
        }))
        .into_any_element()
}
