//! Home's list, as the core works it out: a line saying how many calls wait on the person, the
//! calls themselves as cards, then one line for each goal and agent at work, with idle chats and
//! finished goals folded into one quiet line. Narrowing to a project or found words, and which row
//! is chosen, are the only things decided here.

use crate::board::ChangesBoards;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, list, px, AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, FocusHandle,
    Focusable, FontWeight, HighlightStyle, Hsla, InteractiveElement, IntoElement, ListAlignment,
    ListOffset, ListState, ParentElement, SharedString, Stateful, StatefulInteractiveElement,
    Styled, Subscription, Task, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_view::goals::{GoalAgent, GoalClock, GoalNext, GoalRow, GoalsView};
use sikemux_view::inbox::{InboxItem, InboxNeed};
use sikemux_view::memory::Seeding;
use sikemux_view::slice::SliceState;
use sikemux_wire::protocol::{AgentGroup, AgentStatus, NoticeGoal};

use super::home::{call_card, project_name};
use super::{state_dot, At, FindGoal, GoalsPane, NextGoal, OpenChosen, PreviousGoal};
use crate::core;
use crate::icons::Icon;
use crate::theme::{self, Tokens, RADIUS_2, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3};
use crate::theme::{SPACE_0, SPACE_5, TEXT_2XS_H, TEXT_SM, TEXT_XL, TEXT_XS, TEXT_XS_H};
use crate::ui::{self, Row};
use crate::workbench::Workbench;

/// A goal's line, its title over what it says.
const ROW_HEIGHT: f32 = 42.;
/// How many of a goal's slices its line shows a dot for, and the room each takes.
const MAX_DOTS: usize = 8;
const DOT_STEP: f32 = 17.;
/// The column at a line's end, all lines lining up.
const END_W: f32 = 96.;
const FIND_W: f32 = 190.;
const FIND_MIN_W: f32 = 140.;
const SCOPE_MIN_W: f32 = 56.;
/// How long after one read the goals are read again, however often the core says they moved.
const READ_SPACING: Duration = Duration::from_millis(500);
/// How often the clocks on the rows are drawn again.
const CLOCK_TICK: Duration = Duration::from_secs(30);

/// One line of the list, by what stays the same while the rows around it change.
#[derive(Clone, PartialEq)]
enum Entry {
    Section(&'static str),
    /// Something only the person can decide, by the inbox's id.
    Call(String),
    /// A goal, by its project's folder and card.
    Goal(String, u64),
    /// An agent working no goal, by id.
    Agent(String),
    /// The quiet line that holds idle chats and finished goals.
    Folded,
}

/// The row the panel beside the list shows.
#[derive(Clone, PartialEq)]
pub(super) enum Chosen {
    Goal(String, u64),
    Need(String),
    Agent(String),
}

/// What the line over the list counts, for what the view is narrowed to.
#[derive(Default, Clone, Copy)]
struct Counts {
    calls: usize,
    goals: usize,
    working: usize,
    idle: usize,
    done: usize,
}

pub(super) struct Room {
    pub(super) view: GoalsView,
    read: bool,
    reading: bool,
    /// The core said the goals moved during a read, or while they did not show.
    stale: bool,
    /// The project a click in the side rail narrowed the list to, by folder.
    scope: Option<String>,
    /// The project in front changing means the person picked it here, not on the workbench.
    pub(super) picks_project: bool,
    find: String,
    find_input: Option<(Entity<InputState>, Subscription)>,
    pub(super) chosen: Option<Chosen>,
    /// The idle chats and finished goals show under their quiet line.
    unfolded: bool,
    entries: Vec<Entry>,
    counts: Counts,
    rows: ListState,
    pub(super) keys: FocusHandle,
    _reading: Task<()>,
    _ticking: Task<()>,
}

impl Room {
    pub(super) fn new(cx: &mut Context<GoalsPane>) -> Self {
        let rows = ListState::new(0, ListAlignment::Top, px(ROW_HEIGHT * 10.));
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
            scope: None,
            picks_project: false,
            find: String::new(),
            find_input: None,
            chosen: None,
            unfolded: false,
            entries: Vec::new(),
            counts: Counts::default(),
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

    /// Measures the rows again, as after a card's words changed without the list doing so.
    pub(super) fn reflow(&self) {
        self.rows.remeasure();
    }

    fn entry_chosen(&self, entry: &Entry) -> Option<Chosen> {
        match entry {
            Entry::Section(..) | Entry::Folded => None,
            Entry::Call(id) => Some(Chosen::Need(id.clone())),
            Entry::Goal(project, card) => Some(Chosen::Goal(project.clone(), *card)),
            Entry::Agent(id) => Some(Chosen::Agent(id.clone())),
        }
    }
}

/// What a project's agents go by, by its folder.
fn folder_of(bench: &Workbench, project: Option<&str>) -> Option<String> {
    let id = project?;
    (bench.projects().iter())
        .find(|project| project.id == id)
        .map(|project| project.path.to_string_lossy().into_owned())
}

/// Whether an agent is at work, which keeps its line among the goals rather than folded away.
fn at_work(agent: &GoalAgent) -> bool {
    matches!(agent.state, SliceState::Moving | SliceState::NeedsYou)
}

impl GoalsPane {
    /// Whether the list is on the screen, so reading the goals again is worth it.
    pub(super) fn room_shows(&self, cx: &App) -> bool {
        self.lit() == At::Home
            && (self.bench.upgrade()).is_some_and(|bench| bench.read(cx).showing_goals())
    }

    /// The core says the goals moved: reads them again when `wanted` beyond the list, or once
    /// the list or a goal's page, which lists what the core placed on it, shows.
    pub fn goals_moved(&mut self, on_screen: bool, wanted: bool, cx: &mut Context<Self>) {
        if wanted || (on_screen && matches!(self.lit(), At::Home | At::Goal(_))) {
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
        let asked = cx.background_spawn(asked);
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

    /// Takes the goals as the core answered. The core reads them again when what waits on the
    /// person moves, so the cards are measured again even when the goals stand as they did.
    fn take_goals(&mut self, view: GoalsView, cx: &mut Context<Self>) {
        let moved = self.room.view != view;
        self.room.view = view;
        self.room.read = true;
        if let Some(panel) = &self.panel {
            panel.update(cx, |_, cx| cx.notify());
        }
        if moved {
            self.relist(cx);
            let goals = &self.room.view;
            let _ = self
                .bench
                .update(cx, |bench, cx| bench.refocus_goal(goals, cx));
        } else {
            self.room.reflow();
            cx.notify();
        }
    }

    /// Works out the lines and counts again, keeping what is in view where it is.
    pub(super) fn relist(&mut self, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let bench = bench.read(cx);
        let room = &self.room;
        let scope = room.scope.as_deref();
        let find = room.find.trim().trim_start_matches('@').to_lowercase();
        let has = |words: &str| words.to_lowercase().contains(&find);
        let named = |name: Option<&str>| name.is_some_and(has);
        let agent_named = |id: &String| {
            room.agent(id)
                .is_some_and(|agent| named(agent.name.as_deref()))
        };
        let in_scope = |row: &GoalRow| scope.is_none_or(|scope| row.project == scope);
        let found = |row: &GoalRow| {
            find.is_empty() || has(&row.title) || row.agents.iter().any(agent_named)
        };
        let agent_scoped = |agent: &GoalAgent| {
            scope.is_none_or(|scope| {
                folder_of(bench, agent.project.as_deref()).as_deref() == Some(scope)
            })
        };
        let agent_found =
            |agent: &GoalAgent| find.is_empty() || has(&agent.task) || named(agent.name.as_deref());
        let scoped: Vec<&GoalRow> = room.view.goals.iter().filter(|row| in_scope(row)).collect();
        let loose = (room.view.loose_needs.iter()).filter(|need| {
            scope.is_none_or(|scope| {
                let named = need.project.as_deref();
                named == Some(scope) || folder_of(bench, named).as_deref() == Some(scope)
            })
        });
        let placed: std::collections::HashSet<&str> = (scoped.iter())
            .flat_map(|row| row.needs.iter().map(String::as_str))
            .chain(loose.map(|need| need.id.as_str()))
            .collect();
        let items = bench
            .shown()
            .map_or(&[][..], |view| view.inbox.items.as_slice());
        let calls: Vec<&InboxItem> = (items.iter())
            .filter(|item| item.need.is_some() && placed.contains(item.id.as_str()))
            .collect();
        let call_found = |item: &&&InboxItem| {
            find.is_empty()
                || has(&item.what)
                || (item.asked.as_deref().or(item.detail.as_deref())).is_some_and(has)
                || named(item.name.as_deref())
        };
        let no_goal: Vec<&GoalAgent> = (room.view.no_goal.iter())
            .filter_map(|id| room.agent(id))
            .filter(|agent| agent_scoped(agent))
            .collect();
        let (working, idle): (Vec<&GoalAgent>, Vec<&GoalAgent>) =
            no_goal.into_iter().partition(|agent| at_work(agent));
        let (open, done): (Vec<&GoalRow>, Vec<&GoalRow>) =
            (scoped.iter().copied()).partition(|row| row.state != SliceState::Done);
        let done_earlier = if scope.is_none() {
            room.view.done_earlier
        } else {
            0
        };
        let counts = Counts {
            calls: calls.len(),
            goals: open.len(),
            working: (room.view.agents.iter())
                .filter(|agent| agent_scoped(agent) && agent.group == AgentGroup::Working)
                .count(),
            idle: idle.len(),
            done: done.len() + done_earlier,
        };

        let goal_entry = |row: &&GoalRow| Entry::Goal(row.project.clone(), row.card);
        let agent_entry = |agent: &&GoalAgent| Entry::Agent(agent.id.clone());
        let mut entries = Vec::new();
        let found_calls: Vec<Entry> = (calls.iter())
            .filter(call_found)
            .map(|item| Entry::Call(item.id.clone()))
            .collect();
        entries.extend(found_calls);
        let lines: Vec<Entry> = (open.iter())
            .filter(|row| found(row))
            .map(goal_entry)
            .chain(
                (working.iter())
                    .filter(|agent| agent_found(agent))
                    .map(agent_entry),
            )
            .collect();
        if !lines.is_empty() {
            entries.push(Entry::Section("Goals"));
            entries.extend(lines);
        }
        if counts.idle + counts.done > 0 {
            entries.push(Entry::Folded);
            if room.unfolded || !find.is_empty() {
                entries.extend(
                    (idle.iter())
                        .filter(|agent| agent_found(agent))
                        .map(agent_entry),
                );
                entries.extend(done.iter().filter(|row| found(row)).map(goal_entry));
            }
        }

        let top = self.room.rows.logical_scroll_top();
        let anchor = self.room.entries.get(top.item_ix).cloned();
        crate::ui::relist(&self.room.rows, &self.room.entries, &entries);
        self.room.rows.remeasure();
        if top.item_ix > 0 || top.offset_in_item > px(0.) {
            let kept = anchor.and_then(|anchor| entries.iter().position(|entry| *entry == anchor));
            self.room.rows.scroll_to(match kept {
                Some(item_ix) => ListOffset {
                    item_ix,
                    offset_in_item: top.offset_in_item,
                },
                None => ListOffset {
                    item_ix: top.item_ix.min(entries.len().saturating_sub(1)),
                    offset_in_item: px(0.),
                },
            });
        }
        self.room.entries = entries;
        self.room.counts = counts;
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

    fn toggle_folded(&mut self, cx: &mut Context<Self>) {
        self.room.unfolded = !self.room.unfolded;
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

    /// Opens a goal's page, or for one an agent works alone, that agent's chat.
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

/// Home's line of what waits on the person, the find box, and the calls and goals under them.
pub(super) fn draw(
    pane: &mut GoalsPane,
    window: &mut Window,
    cx: &mut Context<GoalsPane>,
) -> AnyElement {
    let happened = (pane.bench.upgrade()).map_or(0, |bench| {
        pane.happened(bench.read(cx)).len() + pane.decided_away()
    });
    let reading_rules = matches!(
        pane.memory
            .as_ref()
            .and_then(|memory| memory.seeding.as_ref()),
        Some(Seeding::Reading { .. })
    );
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(head(pane, happened, window, cx))
        .when(reading_rules, |list| {
            list.children(
                super::rules::offer(pane, false, cx)
                    .map(|reading| div().px(px(28.)).child(reading)),
            )
        })
        .child(rows(pane, cx))
        .into_any_element()
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// How many calls wait on the person, and quietly beside it how many agents work, on how many
/// goals, and when all of it should be through; then how much happened while they were away and
/// the find box.
fn head(
    pane: &mut GoalsPane,
    happened: usize,
    window: &mut Window,
    cx: &mut Context<GoalsPane>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let counts = pane.room.counts;
    let says = match counts.calls {
        0 => div().child("Nothing needs you"),
        calls => div()
            .flex()
            .gap(px(5.))
            .child(div().text_color(t.accent).child(calls.to_string()))
            .child(if calls == 1 {
                "call for you"
            } else {
                "calls for you"
            }),
    };
    let mut quiet = vec![
        (plural(counts.working, "agent", "agents"), true),
        (" working on ".to_owned(), false),
        (plural(counts.goals, "goal", "goals"), true),
    ];
    if let Some(through) = pane.room.view.through {
        quiet.push((" · everything through by ".to_owned(), false));
        quiet.push((ui::time::clock(through.at), true));
        if through.then_you {
            quiet.push((", then you".to_owned(), false));
        }
    }
    let strong = HighlightStyle {
        color: Some(t.text_secondary),
        font_weight: Some(FontWeight::MEDIUM),
        ..HighlightStyle::default()
    };
    let scope = pane.room.scope.clone().map(|folder| {
        let name = (pane.bench.upgrade())
            .map(|bench| project_name(bench.read(cx), &folder))
            .unwrap_or(folder);
        div()
            .id("goals-scope")
            .flex()
            .flex_shrink_1()
            .min_w(px(SCOPE_MIN_W))
            .items_center()
            .gap(SPACE_1)
            .h(px(26.))
            .px(SPACE_2)
            .rounded(RADIUS_2)
            .cursor_pointer()
            .text_color(t.text_secondary)
            .bg(t.surface_hover)
            .child(
                ui::text(name, TEXT_XS_H, FontWeight::NORMAL)
                    .min_w_0()
                    .truncate(),
            )
            .child(ui::icon(Icon::Close, 10.).text_color(t.text_tertiary))
            .on_click(cx.listener(|pane, _, _, cx| pane.narrow_to(None, cx)))
    });
    let find = pane.find_field(window, cx);
    div()
        .flex()
        .flex_wrap()
        .flex_none()
        .items_end()
        .gap_x(SPACE_3)
        .gap_y(SPACE_2)
        .pt(SPACE_5)
        .pb(SPACE_3)
        .px(px(28.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(220.))
                .gap(SPACE_0)
                .child(
                    says.text_size(theme::ui_text(TEXT_XL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text_primary),
                )
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_SM))
                        .text_color(t.text_tertiary)
                        .child(super::emphasised(quiet, strong)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(SPACE_1)
                .when(happened > 0, |ends| {
                    ends.child(
                        div()
                            .id("goals-overnight")
                            .flex_none()
                            .mx(SPACE_2)
                            .cursor_pointer()
                            .text_color(t.text_tertiary)
                            .hover(move |link| link.text_color(t.text_secondary))
                            .tooltip(ui::tooltip(format!(
                                "{happened} happened while you were away"
                            )))
                            .child(ui::text(
                                format!("{happened} happened"),
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
                        .min_w(px(FIND_MIN_W))
                        .flex_shrink_1()
                        .capture_key_down(
                            cx.listener(|pane, event, window, cx| pane.find_key(event, window, cx)),
                        )
                        .child(ui::Field::new(&find).small().hint(Some("/".into()))),
                ),
        )
}

/// The lines. Only those in view are drawn.
fn rows(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> impl IntoElement {
    let view = cx.entity();
    let rows = list(pane.room.rows.clone(), move |at, _, cx| {
        view.update(cx, |pane, cx| entry_row(pane, at, cx))
    })
    .size_full();
    div()
        .id("goals-rows")
        .key_context("GoalsList")
        .track_focus(&pane.room.keys)
        .relative()
        .flex_1()
        .min_h_0()
        .px(px(20.))
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

/// The name over a part of the list, in spaced capitals on a hairline.
fn section_line(name: &'static str, cx: &App) -> Div {
    div()
        .pt(SPACE_5)
        .pb(SPACE_1H)
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(ui::caps(name, cx))
        .child(div().flex_1().h(px(1.)).bg(Tokens::get(cx).line))
}

fn entry_row(pane: &GoalsPane, at: usize, cx: &mut Context<GoalsPane>) -> AnyElement {
    match pane.room.entries.get(at) {
        Some(Entry::Section(name)) => section_line(name, cx).into_any_element(),
        Some(Entry::Call(id)) => match pane.inbox_item(id, cx) {
            Some(item) => {
                let chosen = pane.room.chosen.as_ref() == Some(&Chosen::Need(id.clone()));
                div()
                    .pb(SPACE_2)
                    .child(call_card(pane, &item, true, chosen, cx))
                    .into_any_element()
            }
            None => div().into_any_element(),
        },
        Some(Entry::Goal(project, card)) => match pane.room.goal(project, *card) {
            Some(row) => goal_row(pane, row, cx),
            None => div().into_any_element(),
        },
        Some(Entry::Agent(agent)) => match pane.room.agent(agent) {
            Some(agent) => agent_row(pane, agent, cx),
            None => div().into_any_element(),
        },
        Some(Entry::Folded) => folded_line(pane, cx),
        None => div().into_any_element(),
    }
}

/// What a goal's line says of it, plainly: what moves, or what it waits on. What the person is
/// asked shows on its card, so a goal waiting on them only says so.
pub(super) fn goal_says(row: &GoalRow) -> Option<String> {
    Some(match row.next.as_ref()? {
        GoalNext::Need {
            need: InboxNeed::Decide,
            ..
        } => "waits on your call".to_owned(),
        GoalNext::Need {
            need: InboxNeed::Unblock,
            ..
        } => "waits on you to unblock it".to_owned(),
        next => next_words(next),
    })
}

/// What comes next for a goal, in a line.
pub(super) fn next_words(next: &GoalNext) -> String {
    match next {
        GoalNext::Need { what, .. } => what.clone(),
        GoalNext::Failed { slice } => format!("{slice} failed"),
        GoalNext::Gate { gate, slice, owner } => {
            super::goal::step_words(gate, slice, owner.as_deref())
        }
        GoalNext::Agent { agent, idle: false } => format!("@{agent} at work"),
        GoalNext::Agent { agent, idle: true } => format!("@{agent} idle"),
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

/// The ink for what a line says is next.
pub(super) fn next_ink(state: SliceState, cx: &App) -> Hsla {
    let t = Tokens::get(cx);
    match state {
        SliceState::NeedsYou => t.warn,
        SliceState::Failed => t.danger,
        _ => t.text_secondary,
    }
}

/// What a line shows at its end: when it should be through, in the body ink, or where it is.
enum End {
    Through(String),
    Where(String),
}

/// A line of the list: its mark, its title with what it says in a plain phrase under it, a dot
/// for each of its slices, and at its end when it should be through or where it is.
fn line(
    id: SharedString,
    title: String,
    state: SliceState,
    says: Option<String>,
    slices: Vec<SliceState>,
    end: Option<End>,
    cx: &App,
) -> Row {
    let t = Tokens::get(cx);
    let done = state == SliceState::Done;
    let dots = (!slices.is_empty()).then(|| {
        let width = px((slices.len().min(MAX_DOTS) as f32) * DOT_STEP);
        super::state_strip(slices.into_iter().take(MAX_DOTS), width, cx)
    });
    let row = Row::new(id, title)
        .mark(14., mark(state, cx))
        .centered()
        .height(ROW_HEIGHT)
        .padding_right(SPACE_2H.into())
        .children(dots)
        .child(
            div()
                .w(px(END_W))
                .flex_none()
                .flex()
                .justify_end()
                .children(end.map(|end| {
                    let (words, ink) = match end {
                        End::Through(at) => (at, t.text_secondary),
                        End::Where(name) => (name, t.text_tertiary),
                    };
                    ui::text(words, TEXT_XS, FontWeight::NORMAL)
                        .font_features(theme::tabular())
                        .truncate()
                        .text_color(ink)
                })),
        );
    let row = match done {
        true => row.quiet(),
        false => row,
    };
    match says {
        Some(says) => row.subtitle(
            ui::text(says, TEXT_XS, FontWeight::NORMAL)
                .min_w_0()
                .truncate()
                .text_color(if done {
                    t.text_tertiary
                } else {
                    next_ink(state, cx)
                }),
        ),
        None => row,
    }
}

fn mark(state: SliceState, cx: &App) -> AnyElement {
    let dot = state_dot(state, 9., cx);
    match state {
        SliceState::Done => div().opacity(0.5).child(dot).into_any_element(),
        _ => dot,
    }
}

/// A line with its click: the first chooses it, a second opens what it is about.
fn spaced(id: SharedString, row: impl IntoElement) -> Stateful<Div> {
    div().id(id).child(row)
}

fn goal_row(pane: &GoalsPane, row: &GoalRow, cx: &mut Context<GoalsPane>) -> AnyElement {
    let chosen = pane.room.chosen == Some(Chosen::Goal(row.project.clone(), row.card));
    let end = match row.clock {
        Some(GoalClock::Until { at }) => Some(End::Through(ui::time::clock(at))),
        Some(GoalClock::Late { .. }) => Some(End::Through("late".to_owned())),
        Some(GoalClock::Finished { at }) => Some(End::Where(ui::time::ago(at))),
        _ => (pane.bench.upgrade())
            .map(|bench| End::Where(project_name(bench.read(cx), &row.project))),
    };
    let picks = Chosen::Goal(row.project.clone(), row.card);
    let (folder, card) = (row.project.clone(), row.card);
    let id = SharedString::from(format!("goal-{}-{}", row.project, row.card));
    let shown = line(
        id.clone(),
        row.title.clone(),
        row.state,
        goal_says(row),
        row.slices.iter().map(|slice| slice.state).collect(),
        end,
        cx,
    )
    .chosen(chosen)
    .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
        pane.room.keys.focus(window, cx);
        pane.room.chosen = Some(picks.clone());
        cx.notify();
        if event.click_count() == 2 {
            pane.open_row(&folder, card, cx);
        }
    }));
    let owner = row.agents.first().cloned();
    super::with_agent_menu(spaced(id, shown), owner, &pane.bench)
}

/// A line for an agent working no goal.
fn agent_row(pane: &GoalsPane, agent: &GoalAgent, cx: &mut Context<GoalsPane>) -> AnyElement {
    let chosen = pane.room.chosen == Some(Chosen::Agent(agent.id.clone()));
    let project = (pane.bench.upgrade()).and_then(|bench| {
        let bench = bench.read(cx);
        (bench.projects().iter())
            .find(|project| Some(project.id.as_str()) == agent.project.as_deref())
            .map(|project| project.name.clone())
    });
    let who = agent.name.as_ref().map(|name| format!("@{name}"));
    let says = [agent.left.clone(), who]
        .into_iter()
        .flatten()
        .reduce(|left, who| format!("{left} · {who}"));
    let picks = Chosen::Agent(agent.id.clone());
    let opened = agent.id.clone();
    let id = SharedString::from(format!("loose-{}", agent.id));
    let shown = line(
        id.clone(),
        agent.task.clone(),
        agent.state,
        says,
        Vec::new(),
        project.map(End::Where),
        cx,
    )
    .chosen(chosen)
    .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
        pane.room.keys.focus(window, cx);
        pane.room.chosen = Some(picks.clone());
        cx.notify();
        if event.click_count() == 2 {
            pane.open_agent(&opened, cx);
        }
    }));
    super::with_agent_menu(spaced(id, shown), Some(agent.id.clone()), &pane.bench)
}

/// The quiet line that holds idle chats and finished goals, which opens to list them.
fn folded_line(pane: &GoalsPane, cx: &mut Context<GoalsPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let counts = pane.room.counts;
    let words = [
        (counts.idle > 0).then(|| plural(counts.idle, "idle chat", "idle chats")),
        (counts.done > 0).then(|| format!("{} done", counts.done)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let open = pane.room.unfolded || !pane.room.find.trim().is_empty();
    div()
        .id("goals-folded")
        .group("goals-folded")
        .mt(SPACE_1)
        .py(SPACE_2)
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .cursor_pointer()
        .text_color(t.text_tertiary)
        .hover(move |line| line.text_color(t.text_secondary))
        .child(crate::motion::turn(
            "goals-folded-chevron",
            crate::motion::fold_angle(open),
            crate::motion::DETAIL_FOLD,
            ui::icon(Icon::Chevron, 10.).text_color(t.text_tertiary),
        ))
        .child(ui::text(words, TEXT_XS, FontWeight::NORMAL))
        .on_click(cx.listener(|pane, _, _, cx| pane.toggle_folded(cx)))
        .into_any_element()
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
