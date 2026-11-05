//! Home's list of every goal, as the core works them out: the stats over it, its filter tabs and
//! find box, and the rows, of which only those in view are drawn. Filtering, ranking and which row
//! is chosen are the only things decided here.

use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, linear_color_stop, linear_gradient, list, px, relative, AnyElement, App, AppContext,
    ClickEvent, Context, Div, Entity, FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement,
    IntoElement, ListAlignment, ListState, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Task, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::NoParams;
use sikemux_wire::goals::{
    DeskCounts, GoalAgent, GoalClock, GoalNext, GoalRow, GoalsView, LooseNeed,
};
use sikemux_wire::inbox::{InboxItem, InboxKind, InboxNeed};
use sikemux_wire::protocol::{AgentGroup, AgentStatus, NoticeGoal};
use sikemux_wire::slice::SliceState;

use super::home::project_name;
use super::{state_dot, At, FindGoal, GoalsPane, NextGoal, OpenChosen, PreviousGoal};
use crate::core;
use crate::icons::Icon;
use crate::theme::{self, Tokens, RADIUS_2, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3};
use crate::theme::{SPACE_3H, TEXT_2XS_H, TEXT_XS, TEXT_XS_H};
use crate::ui::{self, Row};
use crate::workbench::Workbench;

/// How large the numbers over the list stand.
const STAT_SIZE: f32 = 30.;
/// A goal's row, two lines with room around them.
const ROW_HEIGHT: f32 = 50.;
const TITLE_SIZE: f32 = 13.5;
/// The columns at a row's end, all rows lining up.
const BAR_W: f32 = 54.;
const CREW_W: f32 = 112.;
const DESK_W: f32 = 112.;
const CLOCK_W: f32 = 46.;
const MARK_SIZE: f32 = 16.;
const MARKS_SHOWN: usize = 4;
const FIND_W: f32 = 190.;
/// How long after one read the goals are read again, however often the core says they moved.
const READ_SPACING: Duration = Duration::from_millis(250);
/// How often the clocks on the rows are drawn again.
const CLOCK_TICK: Duration = Duration::from_secs(30);

/// The tabs over the list.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Filter {
    #[default]
    Active,
    Needs,
    Moving,
    Waiting,
    Done,
}

/// What a stat ranks the list by, in place of its sections.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Rank {
    Agents,
    Terminals,
    Pages,
    Simulators,
    Files,
}

impl Rank {
    fn of(self, row: &GoalRow) -> usize {
        match self {
            Rank::Agents => row.agents.len(),
            Rank::Terminals => row.desk.terminals,
            Rank::Pages => row.desk.pages,
            Rank::Simulators => row.desk.simulators,
            Rank::Files => row.desk.files,
        }
    }
}

/// One line of the list.
#[derive(Clone, PartialEq)]
enum Entry {
    Section(&'static str, usize),
    /// A goal, by its place in the core's view.
    Goal(usize),
    /// Something that waits on the person about no goal, by the inbox's id.
    Need(String),
    /// An agent working no goal, by its place in the core's view.
    Agent(usize),
}

/// The row the panel beside the list shows.
#[derive(Clone, PartialEq)]
pub(super) enum Chosen {
    Goal(String, u64),
    Need(String),
    Agent(String),
}

#[derive(Default, Clone, Copy)]
struct Counts {
    active: usize,
    needs: usize,
    moving: usize,
    waiting: usize,
    done: usize,
}

/// The numbers over the list, for what the view is narrowed to.
#[derive(Default, Clone, Copy)]
struct Stats {
    agents: usize,
    working: usize,
    desk: DeskCounts,
}

pub(super) struct Room {
    pub(super) view: GoalsView,
    read: bool,
    reading: bool,
    /// The core said the goals moved during a read, or while they did not show.
    stale: bool,
    filter: Filter,
    rank: Option<Rank>,
    /// The project a click in the side rail narrowed the list to, by folder.
    scope: Option<String>,
    /// The project in front changing means the person picked it here, not on the workbench.
    pub(super) picks_project: bool,
    find: String,
    find_input: Option<(Entity<InputState>, Subscription)>,
    pub(super) chosen: Option<Chosen>,
    entries: Vec<Entry>,
    counts: Counts,
    stats: Stats,
    rows: ListState,
    pub(super) keys: FocusHandle,
    _reading: Task<()>,
    _ticking: Task<()>,
}

impl Room {
    pub(super) fn new(cx: &mut Context<GoalsPane>) -> Self {
        let rows = ListState::new(0, ListAlignment::Top, px(ROW_HEIGHT * 6.));
        let ticking = cx.spawn(async move |pane, cx| loop {
            cx.background_executor().timer(CLOCK_TICK).await;
            let alive = pane.update(cx, |pane, cx| {
                if pane.room_shows(cx) {
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        });
        Self {
            view: GoalsView::default(),
            read: false,
            reading: false,
            stale: false,
            filter: Filter::default(),
            rank: None,
            scope: None,
            picks_project: false,
            find: String::new(),
            find_input: None,
            chosen: None,
            entries: Vec::new(),
            counts: Counts::default(),
            stats: Stats::default(),
            rows,
            keys: cx.focus_handle(),
            _reading: Task::ready(()),
            _ticking: ticking,
        }
    }

    pub(super) fn goal(&self, project: &str, card: u64) -> Option<&GoalRow> {
        (self.view.goals.iter()).find(|row| row.card == card && row.project == project)
    }

    pub(super) fn agent(&self, id: &str) -> Option<&GoalAgent> {
        self.view.agents.iter().find(|agent| agent.id == id)
    }

    fn entry_chosen(&self, entry: &Entry) -> Option<Chosen> {
        match entry {
            Entry::Section(..) => None,
            Entry::Goal(at) => {
                let row = self.view.goals.get(*at)?;
                Some(Chosen::Goal(row.project.clone(), row.card))
            }
            Entry::Need(id) => Some(Chosen::Need(id.clone())),
            Entry::Agent(at) => Some(Chosen::Agent(self.view.agents.get(*at)?.id.clone())),
        }
    }
}

fn needs_you(state: SliceState) -> bool {
    matches!(state, SliceState::NeedsYou | SliceState::Failed)
}

fn listed(filter: Filter, state: SliceState) -> bool {
    match filter {
        Filter::Active => state != SliceState::Done,
        Filter::Needs => needs_you(state),
        Filter::Moving => state == SliceState::Moving,
        Filter::Waiting => state == SliceState::Waiting,
        Filter::Done => state == SliceState::Done,
    }
}

/// What a project's agents go by, by its folder.
fn folder_of(bench: &Workbench, project: Option<&str>) -> Option<String> {
    let id = project?;
    (bench.projects().iter())
        .find(|project| project.id == id)
        .map(|project| project.path.to_string_lossy().into_owned())
}

impl GoalsPane {
    /// Whether the list is on the screen, so reading the goals again is worth it.
    pub(super) fn room_shows(&self, cx: &App) -> bool {
        self.lit() == At::Home
            && (self.bench.upgrade()).is_some_and(|bench| bench.read(cx).showing_goals())
    }

    /// The core says the goals moved: reads them again when `wanted` beyond the list, or once
    /// the list shows.
    pub fn goals_moved(&mut self, on_screen: bool, wanted: bool, cx: &mut Context<Self>) {
        if wanted || (on_screen && self.lit() == At::Home) {
            self.read_goals(cx);
        } else {
            self.room.stale = true;
        }
    }

    /// Reads the goals, or once more after the read under way.
    pub(super) fn read_goals(&mut self, cx: &mut Context<Self>) {
        if self.room.reading {
            self.room.stale = true;
            return;
        }
        self.room.reading = true;
        self.room.stale = false;
        let asked = core::Target::here(&self.asks).request_typed::<call::Goals>(NoParams {});
        self.room._reading = cx.spawn(async move |pane, cx| {
            let answer = asked.await;
            let again = pane.update(cx, |pane, cx| {
                pane.room.reading = false;
                match answer {
                    Ok(view) => pane.take_goals(view, cx),
                    Err(error) => pane.said(&format!("Couldn't read the goals: {error}"), cx),
                }
                pane.room.stale
            });
            if again.unwrap_or(false) {
                cx.background_executor().timer(READ_SPACING).await;
                let _ = pane.update(cx, |pane, cx| {
                    pane.room.reading = false;
                    pane.read_goals(cx);
                });
            }
        });
    }

    /// Reads the goals if they moved while they did not show.
    pub(super) fn read_goals_if_stale(&mut self, cx: &mut Context<Self>) {
        if self.room.stale || !self.room.read {
            self.read_goals(cx);
        }
    }

    fn take_goals(&mut self, view: GoalsView, cx: &mut Context<Self>) {
        let moved = self.room.view != view;
        self.room.view = view;
        self.room.read = true;
        if moved {
            self.relist(cx);
            let goals = &self.room.view;
            let _ = self
                .bench
                .update(cx, |bench, cx| bench.refocus_goal(goals, cx));
        }
    }

    /// Works out the rows, counts and numbers again, keeping what is in view where it is.
    pub(super) fn relist(&mut self, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let bench = bench.read(cx);
        let room = &self.room;
        let scope = room.scope.as_deref();
        let find = room.find.trim().trim_start_matches('@').to_lowercase();
        let agent_named = |id: &String| {
            room.agent(id).is_some_and(|agent| {
                (agent.name.as_deref()).is_some_and(|name| name.to_lowercase().contains(&find))
            })
        };
        let in_scope = |row: &GoalRow| scope.is_none_or(|scope| row.project == scope);
        let found = |row: &GoalRow| {
            find.is_empty()
                || row.title.to_lowercase().contains(&find)
                || row.agents.iter().any(agent_named)
        };
        let scoped: Vec<&GoalRow> = room.view.goals.iter().filter(|row| in_scope(row)).collect();
        let loose: Vec<&LooseNeed> = (room.view.loose_needs.iter())
            .filter(|need| {
                scope.is_none_or(|scope| {
                    let named = need.project.as_deref();
                    named == Some(scope) || folder_of(bench, named).as_deref() == Some(scope)
                })
            })
            .collect();
        let agent_scoped = |agent: &GoalAgent| {
            scope.is_none_or(|scope| {
                folder_of(bench, agent.project.as_deref()).as_deref() == Some(scope)
            })
        };
        let no_goal: Vec<usize> = (room.view.no_goal.iter())
            .filter_map(|id| room.view.agents.iter().position(|agent| agent.id == *id))
            .filter(|at| agent_scoped(&room.view.agents[*at]))
            .collect();

        let count = |state: SliceState| scoped.iter().filter(|row| row.state == state).count();
        let needs = count(SliceState::NeedsYou) + count(SliceState::Failed) + loose.len();
        let counts = Counts {
            active: scoped
                .iter()
                .filter(|row| row.state != SliceState::Done)
                .count()
                + loose.len(),
            needs,
            moving: count(SliceState::Moving),
            waiting: count(SliceState::Waiting),
            done: count(SliceState::Done) + room.view.done_earlier,
        };
        let mut stats = Stats::default();
        for agent in room.view.agents.iter().filter(|agent| agent_scoped(agent)) {
            stats.agents += 1;
            stats.working += usize::from(agent.group == AgentGroup::Working);
            stats.desk.add(&agent.desk);
        }

        let goal_at = |at: &usize| &room.view.goals[*at];
        let shown: Vec<usize> = (0..room.view.goals.len())
            .filter(|at| {
                let row = goal_at(at);
                in_scope(row) && listed(room.filter, row.state) && found(row)
            })
            .collect();
        let need_found = |need: &&LooseNeed| {
            find.is_empty()
                || need.what.to_lowercase().contains(&find)
                || (need.name.as_deref()).is_some_and(|name| name.to_lowercase().contains(&find))
        };
        let loose_ids: Vec<String> = match room.filter {
            Filter::Active | Filter::Needs => (loose.iter())
                .copied()
                .filter(need_found)
                .map(|need| need.id.clone())
                .collect(),
            _ => Vec::new(),
        };
        let agent_found = |at: &usize| {
            let agent = &room.view.agents[*at];
            find.is_empty()
                || agent.task.to_lowercase().contains(&find)
                || (agent.name.as_deref()).is_some_and(|name| name.to_lowercase().contains(&find))
        };
        let loose_agents: Vec<usize> = match room.filter {
            Filter::Active => no_goal.iter().copied().filter(agent_found).collect(),
            _ => Vec::new(),
        };

        let mut entries =
            Vec::with_capacity(shown.len() + loose_ids.len() + loose_agents.len() + 5);
        match room.rank {
            Some(rank) => {
                let mut ranked = shown;
                ranked.sort_by_key(|at| std::cmp::Reverse(rank.of(goal_at(at))));
                entries.extend(ranked.into_iter().map(Entry::Goal));
                entries.extend(loose_ids.into_iter().map(Entry::Need));
                entries.extend(loose_agents.into_iter().map(Entry::Agent));
            }
            None => {
                let of = |state: SliceState| {
                    (shown.iter().copied()).filter(move |at| goal_at(at).state == state)
                };
                let needing = of(SliceState::NeedsYou).count()
                    + of(SliceState::Failed).count()
                    + loose_ids.len();
                if needing > 0 {
                    entries.push(Entry::Section("Needs you", needing));
                    entries.extend(of(SliceState::NeedsYou).map(Entry::Goal));
                    entries.extend(loose_ids.into_iter().map(Entry::Need));
                    entries.extend(of(SliceState::Failed).map(Entry::Goal));
                }
                for (name, state) in [
                    ("Moving", SliceState::Moving),
                    ("Waiting", SliceState::Waiting),
                ] {
                    let count = of(state).count();
                    if count > 0 {
                        entries.push(Entry::Section(name, count));
                        entries.extend(of(state).map(Entry::Goal));
                    }
                }
                if !loose_agents.is_empty() {
                    entries.push(Entry::Section("No goal", loose_agents.len()));
                    entries.extend(loose_agents.into_iter().map(Entry::Agent));
                }
                let count = of(SliceState::Done).count();
                if count > 0 {
                    entries.push(Entry::Section("Done", count));
                    entries.extend(of(SliceState::Done).map(Entry::Goal));
                }
            }
        }

        crate::git::relist(&self.room.rows, &self.room.entries, &entries);
        self.room.entries = entries;
        self.room.counts = counts;
        self.room.stats = stats;
        let kept = (self.room.chosen.as_ref()).filter(|chosen| {
            (self.room.entries.iter())
                .any(|entry| self.room.entry_chosen(entry).as_ref() == Some(*chosen))
        });
        if kept.is_none() {
            self.room.chosen =
                (self.room.entries.iter()).find_map(|entry| self.room.entry_chosen(entry));
        }
        cx.notify();
    }

    /// The section the top of the list is in, once rows have gone under its name.
    fn section_on_top(&self) -> Option<(&'static str, usize)> {
        let top = self.room.rows.logical_scroll_top();
        if top.item_ix == 0 && top.offset_in_item <= px(0.) {
            return None;
        }
        (self.room.entries.get(..=top.item_ix)?.iter().rev()).find_map(|entry| match entry {
            Entry::Section(name, count) => Some((*name, *count)),
            _ => None,
        })
    }

    /// Follows a project brought to the front from the side rail: the list narrows to it, until
    /// the person takes that away.
    pub(super) fn narrow_to(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        self.room.scope = folder;
        self.relist(cx);
    }

    /// Narrows the list to the project just brought to the front, if the person picked it here.
    pub(super) fn project_picked(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        if self.room.picks_project && self.at == At::Home && self.chat.is_none() {
            self.narrow_to(folder, cx);
        }
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.room.filter = filter;
        self.relist(cx);
    }

    fn set_rank(&mut self, rank: Option<Rank>, cx: &mut Context<Self>) {
        self.room.rank = if self.room.rank == rank { None } else { rank };
        self.relist(cx);
    }

    /// Chooses the row `by` rows from the chosen one.
    fn step_chosen(&mut self, by: isize, cx: &mut Context<Self>) {
        let rows: Vec<(usize, Chosen)> = (self.room.entries.iter().enumerate())
            .filter_map(|(at, entry)| Some((at, self.room.entry_chosen(entry)?)))
            .collect();
        let now = rows
            .iter()
            .position(|(_, chosen)| Some(chosen) == self.room.chosen.as_ref());
        let next = match now {
            Some(now) => now
                .saturating_add_signed(by)
                .min(rows.len().saturating_sub(1)),
            None => 0,
        };
        if let Some((at, chosen)) = rows.into_iter().nth(next) {
            self.room.chosen = Some(chosen);
            self.room.rows.scroll_to_reveal_item(at);
            cx.notify();
        }
    }

    pub(super) fn next_goal(&mut self, _: &NextGoal, _: &mut Window, cx: &mut Context<Self>) {
        self.step_chosen(1, cx);
    }

    pub(super) fn previous_goal(
        &mut self,
        _: &PreviousGoal,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_chosen(-1, cx);
    }

    pub(super) fn find_goal(&mut self, _: &FindGoal, window: &mut Window, cx: &mut Context<Self>) {
        self.find_field(window, cx)
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Opens what the chosen row is about, as a second click on it does.
    pub(super) fn open_chosen(&mut self, _: &OpenChosen, _: &mut Window, cx: &mut Context<Self>) {
        match self.room.chosen.clone() {
            Some(Chosen::Goal(project, card)) => self.open_row(&project, card, cx),
            Some(Chosen::Need(id)) => {
                if let Some(item) = self.inbox_item(&id, cx) {
                    self.open(&item, cx);
                }
            }
            Some(Chosen::Agent(agent)) => self.open_agent(&agent, cx),
            None => {}
        }
    }

    /// Opens a goal's page, or for one that is a single slice, the chat of whoever works it.
    pub(super) fn open_row(&mut self, project: &str, card: u64, cx: &mut Context<Self>) {
        let worker = (self.room.goal(project, card)).and_then(|row| row.worker.clone());
        match worker {
            Some(agent) => self.open_agent(&agent, cx),
            None => self.open_goal(
                &NoticeGoal {
                    project: project.to_owned(),
                    card,
                },
                cx,
            ),
        }
    }

    pub(super) fn inbox_item(&self, id: &str, cx: &App) -> Option<InboxItem> {
        let bench = self.bench.upgrade()?;
        let bench = bench.read(cx);
        let view = bench.shown()?;
        view.inbox.items.iter().find(|item| item.id == id).cloned()
    }

    /// The find box, made the first time the window has one to give it.
    fn find_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some((input, _)) = &self.room.find_input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a goal or @agent"));
        let typing = cx.subscribe_in(
            &input,
            window,
            |pane, input, event, window, cx| match event {
                InputEvent::Change => {
                    pane.room.find = input.read(cx).value().to_string();
                    pane.relist(cx);
                }
                InputEvent::PressEnter { .. } => {
                    pane.open_chosen(&OpenChosen, window, cx);
                }
                _ => {}
            },
        );
        self.room.find_input = Some((input.clone(), typing));
        input
    }

    /// Takes the find box's keys that act on the list: Escape empties it and gives the keys back.
    fn find_key(
        &mut self,
        event: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                if let Some((input, _)) = &self.room.find_input {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                self.room.find.clear();
                self.relist(cx);
                self.room.keys.focus(window, cx);
            }
            "down" => {
                cx.stop_propagation();
                self.step_chosen(1, cx);
            }
            "up" => {
                cx.stop_propagation();
                self.step_chosen(-1, cx);
            }
            _ => {}
        }
    }

    /// A key the list has no use for starts a new message: it goes to the composer below.
    fn type_through(
        &mut self,
        event: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys = &event.keystroke;
        let held = keys.modifiers.platform || keys.modifiers.control || keys.modifiers.function;
        if held || keys.key_char.is_none() {
            return;
        }
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let Some(start) = bench.read(cx).composing(crate::chat::START) else {
            return;
        };
        let keystroke = keys.clone();
        start.draft.read(cx).focus_handle(cx).focus(window, cx);
        cx.stop_propagation();
        window.defer(cx, move |window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        });
    }
}

/// The six numbers over the list. Clicking one ranks the list by it; again goes back.
pub(super) fn stats(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let (counts, stats, rank) = (pane.room.counts, pane.room.stats, pane.room.rank);
    let sub = |words: String, tint: Option<Hsla>| {
        ui::text(words, TEXT_2XS_H, FontWeight::NORMAL)
            .mt(px(1.))
            .text_color(tint.unwrap_or(t.text_tertiary))
    };
    let stat = |id: &'static str,
                value: usize,
                label: &'static str,
                under: Div,
                ranks: Option<Rank>,
                first: bool,
                cx: &mut Context<GoalsPane>| {
        let lit = ranks.is_some() && ranks == rank;
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .when(!first, |stat| {
                stat.pl(px(18.)).border_l_1().border_color(t.border)
            })
            .child(
                ui::text(value.to_string(), STAT_SIZE, FontWeight::NORMAL)
                    .line_height(relative(1.1))
                    .letter_spacing(-0.03)
                    .font_features(theme::tabular())
                    .text_color(t.text_primary),
            )
            .child(
                ui::text(label, TEXT_XS, FontWeight::NORMAL)
                    .mt(px(5.))
                    .text_color(if lit {
                        t.text_primary
                    } else {
                        t.text_secondary
                    })
                    .when(lit, |label| label.underline()),
            )
            .child(under)
            .on_click(cx.listener(move |pane, _, _, cx| pane.set_rank(ranks, cx)))
    };
    let needing = (counts.needs > 0).then_some(t.warn);
    let working = (stats.working > 0).then_some(t.live);
    div()
        .flex()
        .flex_none()
        .pt(px(22.))
        .pb(px(18.))
        .px(px(28.))
        .child(stat(
            "stat-goals",
            counts.active,
            "Goals",
            sub(format!("{} need you", counts.needs), needing),
            None,
            true,
            cx,
        ))
        .child(stat(
            "stat-agents",
            stats.agents,
            "Agents",
            sub(format!("{} working", stats.working), working),
            Some(Rank::Agents),
            false,
            cx,
        ))
        .child(stat(
            "stat-terminals",
            stats.desk.terminals,
            "Terminals",
            sub("running".into(), None),
            Some(Rank::Terminals),
            false,
            cx,
        ))
        .child(stat(
            "stat-pages",
            stats.desk.pages,
            "Browser tabs",
            sub("open".into(), None),
            Some(Rank::Pages),
            false,
            cx,
        ))
        .child(stat(
            "stat-sims",
            stats.desk.simulators,
            "Sims",
            sub("attached".into(), None),
            Some(Rank::Simulators),
            false,
            cx,
        ))
        .child(stat(
            "stat-files",
            stats.desk.files,
            "Files",
            sub("on desks".into(), None),
            Some(Rank::Files),
            false,
            cx,
        ))
}

/// The filter tabs, how much happened while the person was away and the find box.
pub(super) fn tools(
    pane: &mut GoalsPane,
    happened: usize,
    window: &mut Window,
    cx: &mut Context<GoalsPane>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let counts = pane.room.counts;
    let tabs = [
        (
            "tab-active",
            Filter::Active,
            "All active",
            counts.active,
            None,
        ),
        (
            "tab-needs",
            Filter::Needs,
            "Needs you",
            counts.needs,
            Some(SliceState::NeedsYou),
        ),
        (
            "tab-moving",
            Filter::Moving,
            "Moving",
            counts.moving,
            Some(SliceState::Moving),
        ),
        (
            "tab-waiting",
            Filter::Waiting,
            "Waiting",
            counts.waiting,
            Some(SliceState::Waiting),
        ),
        (
            "tab-done",
            Filter::Done,
            "Done",
            counts.done,
            Some(SliceState::Done),
        ),
    ];
    let chosen = pane.room.filter;
    let scope = pane.room.scope.clone().map(|folder| {
        let name = (pane.bench.upgrade())
            .map(|bench| project_name(bench.read(cx), &folder))
            .unwrap_or(folder);
        div()
            .id("goals-scope")
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_1)
            .h(px(26.))
            .px(SPACE_2)
            .rounded(RADIUS_2)
            .cursor_pointer()
            .text_color(t.text_secondary)
            .bg(t.surface_hover)
            .child(ui::text(name, TEXT_XS_H, FontWeight::NORMAL))
            .child(ui::icon(Icon::Close, 10.).text_color(t.text_tertiary))
            .on_click(cx.listener(|pane, _, _, cx| pane.narrow_to(None, cx)))
    });
    let find = pane.find_field(window, cx);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(SPACE_1)
        .px(px(22.))
        .pb(SPACE_2H)
        .children(tabs.map(|(id, filter, label, count, state)| {
            let on = filter == chosen;
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(7.))
                .h(px(26.))
                .px(SPACE_2H)
                .rounded(RADIUS_2)
                .cursor_pointer()
                .text_color(if on { t.text_primary } else { t.text_tertiary })
                .when(on, |tab| tab.bg(t.surface_chosen))
                .hover(move |tab| {
                    let tab = tab.text_color(if on { t.text_primary } else { t.text_secondary });
                    if on {
                        tab
                    } else {
                        tab.bg(t.surface_hover)
                    }
                })
                .children(state.map(|state| state_dot(state, 9., cx)))
                .child(ui::text(label, TEXT_XS_H, FontWeight::NORMAL))
                .child(
                    ui::text(count.to_string(), TEXT_2XS_H, FontWeight::NORMAL)
                        .font_features(theme::tabular())
                        .text_color(t.text_tertiary),
                )
                .on_click(cx.listener(move |pane, _, _, cx| pane.set_filter(filter, cx)))
        }))
        .child(div().flex_1())
        .when(happened > 0, |tools| {
            tools.child(
                div()
                    .id("goals-overnight")
                    .flex_none()
                    .mr(SPACE_2)
                    .cursor_pointer()
                    .text_color(t.text_tertiary)
                    .hover(move |link| link.text_color(t.text_secondary))
                    .child(ui::text(
                        format!("{happened} happened while you were away"),
                        TEXT_2XS_H,
                        FontWeight::NORMAL,
                    ))
                    .on_click(cx.listener(|pane, _, _, cx| pane.go(At::Overnight, cx))),
            )
        })
        .children(scope)
        .child(
            div()
                .w(px(FIND_W))
                .flex_none()
                .capture_key_down(
                    cx.listener(|pane, event, window, cx| pane.find_key(event, window, cx)),
                )
                .child(ui::Field::new(&find).small()),
        )
}

/// The rows. Only those in view are drawn, under the name of the section the top one is in.
pub(super) fn rows(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let rows = list(pane.room.rows.clone(), move |at, _, cx| {
        view.update(cx, |pane, cx| entry_row(pane, at, cx))
    })
    .size_full();
    let clear = crate::see_through::clear(cx);
    div()
        .id("goals-rows")
        .key_context("GoalsList")
        .track_focus(&pane.room.keys)
        .relative()
        .flex_1()
        .min_h_0()
        .px(px(14.))
        .on_action(cx.listener(GoalsPane::next_goal))
        .on_action(cx.listener(GoalsPane::previous_goal))
        .on_action(cx.listener(GoalsPane::find_goal))
        .on_action(cx.listener(GoalsPane::open_chosen))
        .on_key_down(cx.listener(|pane, event, window, cx| pane.type_through(event, window, cx)))
        .when(pane.room.entries.is_empty(), |rows| {
            rows.child(
                div()
                    .py(SPACE_3)
                    .px(SPACE_2H)
                    .child(ui::meta(empty_words(pane), cx)),
            )
        })
        .child(rows)
        .when_some(pane.section_on_top().filter(|_| !clear), |rows, section| {
            rows.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(14.))
                    .right(px(14.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(t.surface, 0.75),
                        linear_color_stop(t.surface.opacity(0.), 1.),
                    ))
                    .child(section_line(section.0, section.1, cx)),
            )
        })
}

fn empty_words(pane: &GoalsPane) -> &'static str {
    if !pane.room.read {
        return "reading the goals…";
    }
    match pane.room.find.is_empty() && pane.room.view.goals.is_empty() {
        true => "No goals yet. Say below what should get done.",
        false => "Nothing here.",
    }
}

fn section_line(name: &'static str, count: usize, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_baseline()
        .gap(px(7.))
        .pt(SPACE_3H)
        .pb(SPACE_1H)
        .px(SPACE_2H)
        .child(ui::text(name, TEXT_XS_H, FontWeight::MEDIUM).text_color(t.text_primary))
        .child(
            ui::text(count.to_string(), TEXT_2XS_H, FontWeight::NORMAL).text_color(t.text_tertiary),
        )
}

fn entry_row(pane: &GoalsPane, at: usize, cx: &mut Context<GoalsPane>) -> AnyElement {
    match pane.room.entries.get(at) {
        Some(Entry::Section(name, count)) => section_line(name, *count, cx).into_any_element(),
        Some(Entry::Goal(goal)) => match pane.room.view.goals.get(*goal) {
            Some(row) => goal_row(pane, row, cx),
            None => div().into_any_element(),
        },
        Some(Entry::Need(id)) => {
            match (pane.room.view.loose_needs.iter()).find(|need| need.id == *id) {
                Some(need) => need_row(pane, need, cx),
                None => div().into_any_element(),
            }
        }
        Some(Entry::Agent(agent)) => match pane.room.view.agents.get(*agent) {
            Some(agent) => agent_row(pane, agent, cx),
            None => div().into_any_element(),
        },
        None => div().into_any_element(),
    }
}

/// A word on what a need is for, as a goal's next line starts.
pub(super) fn need_word(need: InboxNeed) -> &'static str {
    match need {
        InboxNeed::Decide => "Decide",
        InboxNeed::Unblock => "Unblock",
        InboxNeed::Fix => "Fix",
    }
}

/// What a goal's row says comes next.
pub(super) fn next_words(next: &GoalNext) -> String {
    match next {
        GoalNext::Need { need, what } => format!("{} · {what}", need_word(*need)),
        GoalNext::Failed { slice } => format!("Fix · {slice} failed"),
        GoalNext::Gate { gate, slice, owner } => {
            super::goal::step_words(gate, slice, owner.as_deref())
        }
        GoalNext::Working { agent } => format!("@{agent} is working"),
        GoalNext::Done => "Done".to_owned(),
    }
}

pub(super) fn clock_words(clock: GoalClock) -> String {
    match clock {
        GoalClock::Since { at } => ui::time::age(at),
        GoalClock::Until { at } if at <= ui::time::now_ms() => "due".to_owned(),
        GoalClock::Until { at } => ui::time::left(at),
        GoalClock::Late { .. } => "late".to_owned(),
        GoalClock::Finished { at } => ui::time::ago(at),
    }
}

/// The ink for what a row says is next.
pub(super) fn next_ink(state: SliceState, cx: &App) -> Hsla {
    let t = Tokens::get(cx);
    match state {
        SliceState::NeedsYou => t.warn,
        SliceState::Failed => t.danger,
        _ => t.text_secondary,
    }
}

/// The tint across a row that needs the person, fading out before its end.
fn row_tint(tint: Hsla) -> gpui_kit::Background {
    linear_gradient(
        95.,
        linear_color_stop(tint.opacity(0.11), 0.),
        linear_color_stop(tint.opacity(0.), 0.62),
    )
}

/// The two lines every row has: its title, and what is next and where, over its state's mark.
fn two_lines(
    id: SharedString,
    title: impl Into<SharedString>,
    state: SliceState,
    next: Option<String>,
    project: Option<String>,
    chosen: bool,
    cx: &App,
) -> Row {
    let t = Tokens::get(cx);
    let title: SharedString = title.into();
    let done = state == SliceState::Done;
    let ink = if done {
        t.text_tertiary
    } else {
        t.text_primary
    };
    let under = div()
        .flex()
        .min_w_0()
        .mt(px(2.))
        .gap(px(4.))
        .text_size(theme::ui_text(TEXT_XS))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_color(t.text_tertiary)
        .children(next.as_ref().map(|next| {
            div()
                .flex_shrink_1()
                .min_w_0()
                .truncate()
                .text_color(next_ink(state, cx))
                .child(next.clone())
        }))
        .when(next.is_some() && project.is_some(), |under| {
            under.child("·")
        })
        .children(project.map(|project| div().flex_none().child(project)));
    let row = Row::new(id, title.clone())
        .title_view(Some(
            ui::text(title, TITLE_SIZE, FontWeight::NORMAL)
                .min_w_0()
                .truncate()
                .text_color(ink),
        ))
        .mark(14., mark(state, cx))
        .centered()
        .height(ROW_HEIGHT)
        .chosen(chosen)
        .subtitle(under)
        .padding_right(SPACE_2H.into());
    match state {
        SliceState::NeedsYou => row.ground(row_tint(t.warn)),
        SliceState::Failed => row.ground(row_tint(t.danger)),
        _ => row,
    }
}

fn mark(state: SliceState, cx: &App) -> AnyElement {
    let dot = state_dot(state, 9., cx);
    match state {
        SliceState::Done => div().opacity(0.5).child(dot).into_any_element(),
        _ => dot,
    }
}

/// A row with the space kept round it, its second click opening what it is about.
fn spaced(id: SharedString, row: impl IntoElement) -> Stateful<Div> {
    div().id(id).pb(px(2.)).child(row)
}

fn goal_row(pane: &GoalsPane, row: &GoalRow, cx: &mut Context<GoalsPane>) -> AnyElement {
    let chosen = pane.room.chosen == Some(Chosen::Goal(row.project.clone(), row.card));
    let project = (pane.bench.upgrade()).map(|bench| project_name(bench.read(cx), &row.project));
    let next = row.next.as_ref().map(next_words);
    let picks = Chosen::Goal(row.project.clone(), row.card);
    let (folder, card) = (row.project.clone(), row.card);
    let id = SharedString::from(format!("goal-{}-{}", row.project, row.card));
    let line = two_lines(
        id.clone(),
        row.title.clone(),
        row.state,
        next,
        project,
        chosen,
        cx,
    )
    .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
        pane.room.keys.focus(window, cx);
        pane.room.chosen = Some(picks.clone());
        cx.notify();
        if event.click_count() == 2 {
            pane.open_row(&folder, card, cx);
        }
    }))
    .child(ends(
        slice_bar(row, cx).into_any_element(),
        crew(pane, &row.agents, cx).into_any_element(),
        desk_counts(&row.desk, cx).into_any_element(),
        row.clock.map(clock_words),
        cx,
    ));
    let owner = row.agents.first().cloned();
    super::with_agent_menu(spaced(id, line), owner, &pane.bench)
}

/// A row for something that waits on the person about no goal.
fn need_row(pane: &GoalsPane, need: &LooseNeed, cx: &mut Context<GoalsPane>) -> AnyElement {
    let chosen = pane.room.chosen == Some(Chosen::Need(need.id.clone()));
    let project = (pane.bench.upgrade()).and_then(|bench| {
        let bench = bench.read(cx);
        let folder = need.project.as_deref()?;
        let named = (bench.projects().iter())
            .find(|project| project.id == folder)
            .map(|project| project.name.clone());
        Some(named.unwrap_or_else(|| project_name(bench, folder)))
    });
    let state = match need.need {
        InboxNeed::Fix => SliceState::Failed,
        _ => SliceState::NeedsYou,
    };
    let who = (need.name.as_ref()).map_or(String::new(), |name| format!(" · @{name}"));
    let next = Some(format!("{}{who}", need_word(need.need)));
    let picks = Chosen::Need(need.id.clone());
    let opened = need.id.clone();
    let id = SharedString::from(format!("need-{}", need.id));
    let line = two_lines(
        id.clone(),
        need.what.clone(),
        state,
        next,
        project,
        chosen,
        cx,
    )
    .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
        pane.room.keys.focus(window, cx);
        pane.room.chosen = Some(picks.clone());
        cx.notify();
        if event.click_count() == 2 {
            if let Some(item) = pane.inbox_item(&opened, cx) {
                pane.open(&item, cx);
            }
        }
    }))
    .child(ends(
        div().into_any_element(),
        crew(pane, need.agent.as_slice(), cx).into_any_element(),
        div().into_any_element(),
        Some(ui::time::age(need.at)),
        cx,
    ));
    super::with_agent_menu(spaced(id, line), need.agent.clone(), &pane.bench)
}

/// What a need asks, in a line: what an agent asked, else what the core says it is.
pub(super) fn need_title(item: &InboxItem) -> String {
    match (item.kind, &item.detail) {
        (InboxKind::Question, Some(asked)) => asked.clone(),
        _ => item.what.clone(),
    }
}

/// A row for an agent working no goal.
fn agent_row(pane: &GoalsPane, agent: &GoalAgent, cx: &mut Context<GoalsPane>) -> AnyElement {
    let chosen = pane.room.chosen == Some(Chosen::Agent(agent.id.clone()));
    let project = (pane.bench.upgrade()).and_then(|bench| {
        let bench = bench.read(cx);
        (bench.projects().iter())
            .find(|project| Some(project.id.as_str()) == agent.project.as_deref())
            .map(|project| project.name.clone())
    });
    let state = agent_state(agent);
    let next = agent.name.as_ref().map(|name| format!("@{name}"));
    let picks = Chosen::Agent(agent.id.clone());
    let opened = agent.id.clone();
    let id = SharedString::from(format!("loose-{}", agent.id));
    let mut desk = DeskCounts::default();
    desk.add(&agent.desk);
    let line = two_lines(
        id.clone(),
        agent.task.clone(),
        state,
        next,
        project,
        chosen,
        cx,
    )
    .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
        pane.room.keys.focus(window, cx);
        pane.room.chosen = Some(picks.clone());
        cx.notify();
        if event.click_count() == 2 {
            pane.open_agent(&opened, cx);
        }
    }))
    .child(ends(
        div().into_any_element(),
        crew(pane, std::slice::from_ref(&agent.id), cx).into_any_element(),
        desk_counts(&desk, cx).into_any_element(),
        None,
        cx,
    ));
    super::with_agent_menu(spaced(id, line), Some(agent.id.clone()), &pane.bench)
}

/// How an agent on no goal stands, in the marks goals wear.
pub(super) fn agent_state(agent: &GoalAgent) -> SliceState {
    match agent.group {
        AgentGroup::Working => SliceState::Moving,
        AgentGroup::Blocked => SliceState::NeedsYou,
        AgentGroup::Done | AgentGroup::Idle => SliceState::Waiting,
    }
}

/// A row's four columns: its slices, its agents, what is on their desks and its clock.
fn ends(
    bar: AnyElement,
    crew: AnyElement,
    desk: AnyElement,
    clock: Option<String>,
    cx: &App,
) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(SPACE_3)
        .ml(SPACE_1)
        .child(div().w(px(BAR_W)).flex_none().child(bar))
        .child(div().w(px(CREW_W)).flex_none().child(crew))
        .child(div().w(px(DESK_W)).flex_none().child(desk))
        .child(
            div()
                .w(px(CLOCK_W))
                .flex_none()
                .flex()
                .justify_end()
                .children(clock.map(|clock| {
                    ui::text(clock, TEXT_XS, FontWeight::NORMAL)
                        .text_color(Tokens::get(cx).text_tertiary)
                })),
        )
}

/// A goal's slices as a slim line of segments, each in its state's colour.
fn slice_bar(row: &GoalRow, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .gap(px(2.))
        .h(px(3.))
        .children(row.slices.iter().map(|slice| {
            let tint = match slice.state {
                SliceState::Done => t.live.opacity(0.5),
                SliceState::Moving => t.live,
                SliceState::NeedsYou => t.warn,
                SliceState::Failed => t.danger,
                SliceState::Waiting => t.text_primary.opacity(0.12),
            };
            div().flex_1().h_full().rounded(px(2.)).bg(tint)
        }))
}

/// The first of a row's agents by their own marks, and how many more there are.
fn crew(pane: &GoalsPane, agents: &[String], cx: &App) -> Div {
    let t = Tokens::get(cx);
    let extra = agents.len().saturating_sub(MARKS_SHOWN);
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(2.))
        .children(
            (agents.iter().take(MARKS_SHOWN))
                .filter_map(|id| pane.room.agent(id))
                .map(|agent| agent_face(agent, MARK_SIZE, cx)),
        )
        .when(extra > 0, |crew| {
            crew.child(
                ui::text(format!("+{extra}"), TEXT_2XS_H, FontWeight::NORMAL)
                    .ml(SPACE_1H)
                    .text_color(t.text_tertiary),
            )
        })
}

/// An agent's own mark, held still so a long list draws nothing while its agents work, with a dot
/// when it waits on the person or stopped.
pub(super) fn agent_face(agent: &GoalAgent, size: f32, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let flag = match (agent.group, agent.status) {
        (AgentGroup::Blocked, _) => Some(t.warn),
        (_, AgentStatus::Stopped) => Some(t.danger),
        _ => None,
    };
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(ui::still_mark(
            &agent.cli,
            agent.face.as_ref(),
            agent.group,
            size,
            cx,
        ))
        .children(flag.map(|flag| {
            div()
                .absolute()
                .top(px(-2.))
                .right(px(-2.))
                .size(px(6.))
                .rounded_full()
                .border_1()
                .border_color(t.surface)
                .bg(flag)
        }))
        .into_any_element()
}

/// How much is on a row's desks, by icon, leaving out what there is none of.
fn desk_counts(desk: &DeskCounts, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let kinds = [
        (Icon::Terminal, desk.terminals),
        (Icon::Globe, desk.pages),
        (Icon::Phone, desk.simulators),
        (Icon::Editor, desk.files),
    ];
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(9.))
        .children(
            kinds
                .into_iter()
                .filter(|(_, count)| *count > 0)
                .map(|(glyph, count)| {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(3.))
                        .text_color(t.text_tertiary)
                        .child(ui::icon(glyph, 12.).text_color(t.text_tertiary))
                        .child(
                            ui::text(count.to_string(), TEXT_XS, FontWeight::NORMAL)
                                .font_features(theme::tabular()),
                        )
                }),
        )
}
