//! What the window's rails show while it is on the goals: Home, Overnight and Environments at the
//! top of the side rail, and the goals of the project in front on the tab rail, each with its state's mark
//! and how many things in it wait on the person.

use crate::theme::SPACE_2H;
use gpui_kit::div;
use gpui_kit::InteractiveElement;
use gpui_kit::ParentElement;
use gpui_kit::StatefulInteractiveElement;
use gpui_kit::{AnyElement, Context, IntoElement, SharedString, Styled};

use super::{goal_mark, goal_name, goals_of, state_mark, At, GoalsPane};
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::ui::{self, Row};
use crate::workbench::Workbench;

/// The goals, while the window shows them.
/// Home, with how many things wait on the person as the core counts them, and Overnight, with
/// how many happened.
pub fn places(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<Vec<AnyElement>> {
    let goals = bench.goals_in_front()?.clone();
    let t = Tokens::get(cx).clone();
    let pane = goals.read(cx);
    let needed = bench.shown().map_or(0, |view| view.inbox.needed);
    let (at, happened) = (pane.lit(), pane.happened(bench).len());
    let place = |id: &'static str, title: &'static str, icon: Icon, to: At, count: usize| {
        let goals = goals.clone();
        Row::new(id, title)
            .mark(16., ui::icon(icon, 14.).text_color(t.text_secondary))
            .chosen(at == to)
            .status((count > 0).then(|| ui::meta(count.to_string(), cx)))
            .on_click(move |_, _, cx| goals.update(cx, |goals, cx| goals.go(to, cx)))
            .into_any_element()
    };
    Some(vec![
        place("goals-home", "Home", Icon::Inbox, At::Home, needed),
        place(
            "goals-overnight",
            "Overnight",
            Icon::Clock,
            At::Overnight,
            happened,
        ),
        place(
            "goals-environments",
            "Environments",
            Icon::Rocket,
            At::Environments,
            pane.environments_set(),
        ),
    ])
}

/// The goals of the project in front, each with its state's mark and how many things in it
/// wait on the person; the one open is chosen.
pub fn goal_rows(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<Vec<AnyElement>> {
    let goals = bench.goals_in_front()?.clone();
    let t = Tokens::get(cx).clone();
    let pane = goals.read(cx);
    let here = pane.here.as_deref();
    let needs = GoalsPane::needs(bench);
    let cards = pane.cards();
    let rows = goals_of(cards).into_iter().map(|goal| {
        let id = goal.id;
        let waiting = (needs.iter())
            .filter_map(|item| item.card.as_ref())
            .filter(|card| Some(card.project.as_str()) == here)
            .filter(|card| card.id == id || card.goal == Some(id))
            .count();
        let open = matches!(pane.lit(), At::Goal(at) | At::Slice(at)
            if at == id || goal.goal_slices.contains(&at));
        let goals = goals.clone();
        Row::new(
            SharedString::from(format!("rail-goal-{id}")),
            goal_name(goal),
        )
        .mark(16., state_mark(goal_mark(goal), cx))
        .chosen(open)
        .status((waiting > 0).then(|| ui::meta(waiting.to_string(), cx).text_color(t.warn)))
        .on_click(move |_, _, cx| goals.update(cx, |goals, cx| goals.go(At::Goal(id), cx)))
        .into_any_element()
    });
    Some(rows.collect())
}

/// While the window is on the goals, the rail lists only the goals of the project in front.
pub(crate) fn goals_page(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let panel = (bench.goals.as_ref())
        .filter(|_| bench.showing_goals())
        .and_then(|goals| {
            goals
                .read(cx)
                .shows_panel()
                .then(|| goals.update(cx, |goals, cx| goals.panel(cx)))
        });
    if let Some(panel) = panel {
        let panel = panel.cached(gpui_kit::StyleRefinement::default().size_full());
        return Some(panel.into_any_element());
    }
    let rows = goal_rows(bench, cx)?;
    let list = match (rows.is_empty(), bench.project()) {
        (false, _) => div()
            .id("goals-rail-scroll")
            .flex_1()
            .min_h_0()
            .p(SPACE_2H)
            .overflow_y_scroll()
            .children(rows)
            .into_any_element(),
        (true, Some(project)) => crate::agents::rail_empty(
            Icon::Steps,
            format!("No goals in {}", project.name),
            None,
            cx,
        ),
        (true, None) => crate::agents::rail_empty(
            Icon::Steps,
            "No project open",
            Some(("project.open", "open one")),
            cx,
        ),
    };
    Some(
        crate::agents::page_body()
            .child(crate::agents::page_header("Goals", bench, cx))
            .child(list)
            .into_any_element(),
    )
}
