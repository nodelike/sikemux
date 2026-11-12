//! What the window's rails show while it is on the goals: Home and Overnight at the top of the
//! side rail, and the goals of the project in front on the tab rail, each with its state's mark
//! and how many things in it wait on the person.

use crate::theme::SPACE_2H;
use crate::workbench::actions;
use gpui_kit::div;
use gpui_kit::App;
use gpui_kit::AppContext as _;
use gpui_kit::Entity;
use gpui_kit::InteractiveElement;
use gpui_kit::ParentElement;
use gpui_kit::StatefulInteractiveElement;
use gpui_kit::{AnyElement, Context, IntoElement, SharedString, Styled};
use sikemux_view::prefs::TabRailPage;
use sikemux_view::prefs::WindowView;
use sikemux_wire::protocol::NoticeGoal;

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
    let panel = (bench.goals.pane.as_ref())
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
            Some((actions::OpenProject::ID, "open one")),
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

impl Workbench {
    /// Shows the workbench in place of the goals, as anything opened in one of its tabs needs.
    pub fn show_workbench(&mut self, cx: &mut Context<Self>) {
        if self.showing_goals() {
            self.toggle_goals_view(cx);
        }
    }

    /// The agent whose chat the goals show, while they do.
    pub fn chat_in_goals(&self, cx: &App) -> Option<String> {
        let goals = self.goals.pane.as_ref()?;
        let chat = goals.read(cx).chat_shown()?.to_owned();
        self.showing_goals().then_some(chat)
    }

    /// Switches the window between the workbench and the goals, which open on Home.
    pub fn toggle_goals_view(&mut self, cx: &mut Context<Self>) {
        self.window_view = match self.window_view {
            WindowView::Workbench => WindowView::Goals,
            WindowView::Goals => WindowView::Workbench,
        };
        if let (WindowView::Goals, Some(goals)) = (self.window_view, &self.goals.pane) {
            goals.update(cx, |goals, cx| goals.home(cx));
        }
        self.keep_rails(cx);
        cx.notify();
    }

    pub fn open_goal(&mut self, goal: &NoticeGoal, cx: &mut Context<Self>) {
        if !self.showing_goals() {
            self.toggle_goals_view(cx);
        }
        self.goals_view(cx)
            .update(cx, |goals, cx| goals.open_goal(goal, cx));
    }

    /// Opens slice `card` of the project at `project` in the goals.
    pub fn open_slice(&mut self, project: &str, card: u64, cx: &mut Context<Self>) {
        if !self.showing_goals() {
            self.toggle_goals_view(cx);
        }
        self.goals_view(cx)
            .update(cx, |goals, cx| goals.open_slice(project, card, cx));
    }

    /// Shows the environments of the project in front.
    pub fn open_environments(&mut self, cx: &mut Context<Self>) {
        if !self.showing_goals() {
            self.toggle_goals_view(cx);
        }
        self.goals_view(cx)
            .update(cx, |goals, cx| goals.go(At::Environments, cx));
    }

    pub fn environments_lit(&self, cx: &App) -> bool {
        (self.goals_in_front()).is_some_and(|goals| goals.read(cx).lit() == At::Environments)
    }

    pub fn showing_goals(&self) -> bool {
        self.window_view == WindowView::Goals
    }

    /// The goals' pane, while the window shows the goals rather than the workbench.
    pub fn goals_in_front(&self) -> Option<&Entity<crate::goals::GoalsPane>> {
        self.goals.pane.as_ref().filter(|_| self.showing_goals())
    }

    /// The goals view, made the first time it shows.
    pub(crate) fn goals_view(&mut self, cx: &mut Context<Self>) -> Entity<crate::goals::GoalsPane> {
        if let Some(goals) = &self.goals.pane {
            return goals.clone();
        }
        let (asks, bench) = (self.asks.clone(), cx.entity().downgrade());
        let here = (self.project()).map(|project| project.path.to_string_lossy().into_owned());
        let goals = cx.new(|cx| crate::goals::GoalsPane::new(asks, bench, here, cx));
        self.goals.moved =
            Some(cx.subscribe(&goals, |_, _, _: &crate::goals::Moved, cx| cx.notify()));
        self.goals.pane = Some(goals.clone());
        goals
    }

    /// Narrows the agent rail to a goal's agents and shows the workbench with the first in front.
    pub fn focus_goal(
        &mut self,
        title: String,
        agents: Vec<String>,
        project: &str,
        card: u64,
        cx: &mut Context<Self>,
    ) {
        let first = agents.first().cloned();
        self.rails.goal_focus = Some(crate::rail::GoalFocus {
            project: project.to_owned(),
            card,
            title,
            agents,
        });
        self.rails.page = TabRailPage::Agents;
        self.rails.tab_open = true;
        self.show_workbench(cx);
        if let Some(agent) = first {
            self.bring_agent(&agent, cx).detach();
        }
        cx.notify();
    }

    pub fn clear_goal_focus(&mut self, cx: &mut Context<Self>) {
        self.rails.goal_focus = None;
        cx.notify();
    }

    /// Keeps the rail's goal up with who works it now; a goal gone takes the narrowing with it.
    pub fn refocus_goal(&mut self, goals: &sikemux_view::goals::GoalsView, cx: &mut Context<Self>) {
        let Some(focus) = self.rails.goal_focus.as_mut() else {
            return;
        };
        let row =
            (goals.goals.iter()).find(|row| row.card == focus.card && row.project == focus.project);
        match row {
            Some(row) if row.agents == focus.agents && row.title == focus.title => return,
            Some(row) => {
                focus.agents = row.agents.clone();
                focus.title = row.title.clone();
            }
            None => self.rails.goal_focus = None,
        }
        cx.notify();
    }
}
