//! The agent board of a project: a lane for each card that has cards under it, listing them
//! as rows grouped by where they stand, and beside the board one card to read and change,
//! Auto's settings, the messages between the project's agents, or the board's triggers. The
//! core keeps the board and says which column each card is in; the pane only draws and asks.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window,
};
use serde::Serialize;
use sikemux_core::server::board::BoardAddParams;
use sikemux_core::server::board::BoardCard;
use sikemux_core::server::board::BoardColumn;
use sikemux_core::server::board::BoardParams;
use sikemux_core::server::board::BoardSettingsParams;
use sikemux_core::server::board::BoardUpdateParams;
use sikemux_core::server::board::CardStatus;
use sikemux_core::server::board::CardTicket;
use sikemux_core::server::board::ProjectBoard;
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::calls::NoParams;
use sikemux_core::server::hosts::LinkedHost;
use sikemux_core::server::messages::log::MessageRow;
use sikemux_core::server::messages::AgentMessagesParams;
use sikemux_view::slice::{GateState, Passer};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{self, Ask};
use crate::drag::{ghost, DraggedCard};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_2XS_H, TEXT_SM,
};
use crate::toast;
use crate::ui::{self, Button, Field, Group, IconButton, Row, Segmented, Tone, Variant};
use crate::workbench::Workbench;

pub mod panels;

/// The groups a lane lists its cards in, those waiting on the person first.
const GROUPS: [(BoardColumn, &str); 4] = [
    (BoardColumn::Needs, "Needs you"),
    (BoardColumn::Progress, "In progress"),
    (BoardColumn::Open, "Open"),
    (BoardColumn::Done, "Done"),
];
/// The width of a row's reason, its gates and its agent, so they line up down a lane.
const REASON_WIDTH: f32 = 200.;
const GATES_WIDTH: f32 = 108.;
const OWNER_WIDTH: f32 = 112.;

/// The column a card moving to `status` shows in until the core answers.
fn column_moving_to(status: CardStatus) -> BoardColumn {
    match status {
        CardStatus::Open => BoardColumn::Open,
        CardStatus::Progress => BoardColumn::Progress,
        CardStatus::Done => BoardColumn::Done,
    }
}

/// What a card dropped in `column` is moved to; none for the column the core alone fills.
fn status_dropped_in(column: BoardColumn) -> Option<CardStatus> {
    match column {
        BoardColumn::Open => Some(CardStatus::Open),
        BoardColumn::Progress => Some(CardStatus::Progress),
        BoardColumn::Needs => None,
        BoardColumn::Done => Some(CardStatus::Done),
    }
}

/// The first words of a title, for a chip that points at the card.
pub(crate) fn short_title(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.len() > 3 {
        format!("{}…", words[..3].join(" "))
    } else {
        title.to_owned()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Show {
    All,
    Tickets,
    Needs,
    Later,
}

/// What is open beside the board.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Open {
    Card(u64),
    New,
    Auto,
    Messages,
    Triggers,
}

/// The fields of the card open beside the board, or of a new one.
pub(crate) struct Fields {
    pub title: Entity<InputState>,
    pub details: Entity<TextareaState>,
    pub acceptance: Entity<TextareaState>,
    /// Where a new card goes, and what it waits on; a card that exists says its own.
    pub parent: u64,
    pub depends_on: Vec<u64>,
    _edits: Vec<Subscription>,
}

/// A lane of the board: the cards under one card, or those on their own.
/// Where each card sits in the board's list, and what sits under each, so drawing a card needs
/// no search of the board.
#[derive(Default)]
struct Places {
    at: HashMap<u64, usize>,
    /// Keyed by the card above and whether the cards counted are in Later.
    under: HashMap<(u64, bool), UnderCard>,
}

#[derive(Clone, Copy, Default)]
struct UnderCard {
    done: usize,
    total: usize,
}

impl Places {
    fn of(cards: &[BoardCard]) -> Self {
        let mut places = Self::default();
        for (at, card) in cards.iter().enumerate() {
            places.at.insert(card.id, at);
            if let Some(parent) = card.parent {
                let under = (places.under)
                    .entry((parent, card.aside_at.is_some()))
                    .or_default();
                under.total += 1;
                under.done += usize::from(card.status == CardStatus::Done);
            }
        }
        places
    }
}

const MAX_DRAWN_DONE: usize = 200;

struct Lane {
    key: String,
    parent: Option<u64>,
    cards: Vec<u64>,
}

/// The board pane of the project whose board was last in front.
#[derive(Default)]
pub struct BoardTab {
    pub pane: Option<Entity<BoardPane>>,
    /// The card to open beside the board of the project at the folder named, once it shows.
    pub(crate) card: Option<(String, u64)>,
}

/// What the board and the goals share: they change boards through the core, and say what failed.
pub(crate) trait ChangesBoards: Sized + 'static {
    fn asks(&self) -> &UnboundedSender<Ask>;
    fn bench(&self) -> &WeakEntity<Workbench>;
    fn set_busy(&mut self, busy: bool);
    /// Shows the board the core answered a change with.
    fn take_answer(&mut self, board: ProjectBoard);

    fn said(&self, error: &str, cx: &mut Context<Self>) {
        let note = toast::error(error.to_owned()).autohide(false);
        toast::show(self.bench(), note, cx);
    }

    /// Asks the core to change a board, then shows the board it answers.
    fn ask<C: Call<Answer = ProjectBoard> + 'static>(
        &mut self,
        params: C::Params,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        C::Params: Serialize,
    {
        let asked = core::Target::here(self.asks()).request_typed::<C>(params);
        self.set_busy(true);
        cx.notify();
        cx.spawn(async move |pane, cx| {
            let answer = asked.await;
            let _ = pane.update(cx, |pane, cx| {
                pane.set_busy(false);
                match answer {
                    Ok(board) => {
                        pane.take_answer(board);
                        then(pane, cx);
                    }
                    Err(error) => pane.said(&error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl ChangesBoards for BoardPane {
    fn asks(&self) -> &UnboundedSender<Ask> {
        &self.asks
    }

    fn bench(&self) -> &WeakEntity<Workbench> {
        &self.bench
    }

    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    fn take_answer(&mut self, board: ProjectBoard) {
        self.take(board);
    }
}

pub struct BoardPane {
    asks: UnboundedSender<Ask>,
    /// The project's folder, which the core's board calls take.
    pub folder: String,
    pub(crate) bench: WeakEntity<Workbench>,
    pub(crate) board: Option<ProjectBoard>,
    find: Option<Entity<InputState>>,
    words: String,
    show: Show,
    pub(crate) open: Option<Open>,
    pub(crate) fields: Option<Fields>,
    /// Cards on their way to another status, shown there until the core answers.
    places: Places,
    moving: HashMap<u64, CardStatus>,
    folded: HashSet<String>,
    /// Lanes whose done cards show, by lane.
    done_shown: HashSet<String>,
    pub(crate) messages: Option<Vec<MessageRow>>,
    pub(crate) busy: bool,
    /// The hosts linked with this one, once the Auto panel has asked.
    pub(crate) links: Vec<LinkedHost>,
    focus: FocusHandle,
    _typing: Option<Subscription>,
    _reading: Task<()>,
}

/// The project's board, which takes the keys when it was just opened.
pub(crate) fn pane(bench: &Workbench, window: &Window, cx: &mut Context<Workbench>) -> AnyElement {
    let Some(pane) = &bench.board.pane else {
        return crate::ui::pane(cx).into_any_element();
    };
    if bench.claims_keys(crate::keyboard::TakesKeys::Board) {
        let focus = pane.read(cx).focus_handle(cx);
        cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
    }
    crate::ui::pane(cx).child(pane.clone()).into_any_element()
}

impl BoardPane {
    pub fn new(
        asks: UnboundedSender<Ask>,
        folder: String,
        bench: WeakEntity<Workbench>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            asks,
            folder,
            bench,
            board: None,
            find: None,
            words: String::new(),
            show: Show::All,
            open: None,
            fields: None,
            places: Places::default(),
            moving: HashMap::new(),
            folded: HashSet::new(),
            done_shown: HashSet::new(),
            messages: None,
            busy: false,
            links: Vec::new(),
            focus: cx.focus_handle(),
            _typing: None,
            _reading: Task::ready(()),
        };
        pane.changed(cx);
        pane
    }

    /// Reads the board again, and the messages while they are open, as after the core
    /// says the board changed.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        let here = core::Target::here(&self.asks);
        let project = self.folder.clone();
        let read = here.request_typed::<call::Board>(BoardParams {
            project: project.clone(),
        });
        let messages = (self.open == Some(Open::Messages)).then(|| {
            here.request_typed::<call::AgentMessageRows>(AgentMessagesParams {
                project,
                limit: None,
            })
        });
        self._reading = cx.spawn(async move |pane, cx| {
            let board = read.await;
            let rows = match messages {
                Some(messages) => Some(messages.await),
                None => None,
            };
            let _ = pane.update(cx, |pane, cx| {
                match board {
                    Ok(board) => pane.take(board),
                    Err(error) => pane.said(&format!("Couldn't read the board: {error}"), cx),
                }
                if let Some(Ok(rows)) = rows {
                    pane.messages = Some(rows.rows);
                }
                cx.notify();
            });
        });
    }

    fn take(&mut self, board: ProjectBoard) {
        if let Some(Open::Card(id)) = self.open {
            if !board.cards.iter().any(|card| card.id == id) {
                self.open = None;
                self.fields = None;
            }
        }
        self.places = Places::of(&board.cards);
        self.board = Some(board);
    }

    fn move_card(&mut self, id: u64, to: CardStatus, cx: &mut Context<Self>) {
        let Some(card) = self.card(id) else {
            return;
        };
        if card.status == to && card.aside_at.is_none() {
            return;
        }
        self.moving.insert(id, to);
        self.ask::<call::BoardUpdate>(
            BoardUpdateParams {
                status: Some(to),
                ..self.card_change(id)
            },
            move |pane, _| {
                pane.moving.remove(&id);
            },
            cx,
        );
    }

    pub(crate) fn card(&self, id: u64) -> Option<&BoardCard> {
        self.cards().get(*self.places.at.get(&id)?)
    }

    /// How many of the cards the board shows now sit right under card `id`, and how many of
    /// those are done.
    fn under_card(&self, id: u64) -> UnderCard {
        let later = self.show == Show::Later;
        (self.places.under.get(&(id, later)).copied()).unwrap_or_default()
    }

    pub(crate) fn cards(&self) -> &[BoardCard] {
        self.board
            .as_ref()
            .map_or(&[], |board| board.cards.as_slice())
    }

    fn column_of(&self, card: &BoardCard) -> BoardColumn {
        self.moving
            .get(&card.id)
            .map_or(card.column, |status| column_moving_to(*status))
    }

    fn shown(&self, card: &BoardCard) -> bool {
        let wanted = self.words.trim().to_lowercase();
        let matches = |text: &str| text.to_lowercase().contains(&wanted);
        (card.aside_at.is_some() == (self.show == Show::Later))
            && (self.show != Show::Tickets || card.ticket.is_some())
            && (self.show != Show::Needs || self.column_of(card) == BoardColumn::Needs)
            && (wanted.is_empty()
                || matches(&card.title)
                || matches(&card.details)
                || card.owner_name.as_deref().is_some_and(matches)
                || card
                    .ticket
                    .as_ref()
                    .is_some_and(|ticket| matches(&ticket.key)))
    }

    /// The finished cards too old to draw, so a long list of them does not slow every frame.
    /// A search draws all it finds.
    fn undrawn_done(&self) -> HashSet<u64> {
        if !self.words.trim().is_empty() {
            return HashSet::new();
        }
        let mut done: Vec<&BoardCard> = (self.cards().iter())
            .filter(|card| self.column_of(card) == BoardColumn::Done && self.shown(card))
            .collect();
        done.sort_by_key(|card| std::cmp::Reverse(card.updated_at));
        (done.into_iter().skip(MAX_DRAWN_DONE))
            .map(|card| card.id)
            .collect()
    }

    /// One lane per card that has cards under it, holding them, then one for the cards
    /// on their own; the newest work first.
    fn lanes(&self) -> Vec<Lane> {
        let cards = self.cards();
        let parents: HashSet<u64> = cards.iter().filter_map(|card| card.parent).collect();
        let mut shown: Vec<&BoardCard> = cards.iter().filter(|card| self.shown(card)).collect();
        shown.sort_by_key(|card| std::cmp::Reverse(card.updated_at));
        let mut lanes: Vec<Lane> = Vec::new();
        let mut alone = Lane {
            key: "alone".into(),
            parent: None,
            cards: Vec::new(),
        };
        let mut places: HashMap<u64, usize> = HashMap::new();
        let mut lane_of = |parent: u64, lanes: &mut Vec<Lane>| -> usize {
            *places.entry(parent).or_insert_with(|| {
                lanes.push(Lane {
                    key: format!("card-{parent}"),
                    parent: Some(parent),
                    cards: Vec::new(),
                });
                lanes.len() - 1
            })
        };
        for card in shown {
            if parents.contains(&card.id) {
                lane_of(card.id, &mut lanes);
            } else if let Some(parent) = card.parent.filter(|parent| self.card(*parent).is_some()) {
                let place = lane_of(parent, &mut lanes);
                lanes[place].cards.push(card.id);
            } else {
                alone.cards.push(card.id);
            }
        }
        if !alone.cards.is_empty() {
            lanes.push(alone);
        }
        lanes
    }

    /// Opens a card, a new one, Auto or the messages beside the board, or closes it.
    pub(crate) fn select(
        &mut self,
        open: Option<Open>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open = open;
        self.fields = match open {
            Some(Open::Card(id)) => self
                .card(id)
                .cloned()
                .map(|card| self.fields_for(Some(&card), window, cx)),
            Some(Open::New) => Some(self.fields_for(None, window, cx)),
            _ => None,
        };
        if open == Some(Open::Messages) {
            self.messages = None;
            self.changed(cx);
        }
        if open == Some(Open::Auto) {
            self.read_links(cx);
        }
        cx.notify();
    }

    /// Opens card `id` beside the board, once the board read holds it.
    pub(crate) fn reveal(&mut self, id: u64, cx: &mut Context<Self>) {
        self.open = Some(Open::Card(id));
        self.fields = None;
        cx.notify();
    }

    fn fields_for(
        &self,
        card: Option<&BoardCard>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Fields {
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("What needs doing")
                .default_value(card.map_or(String::new(), |card| card.title.clone()))
        });
        let area = |text: String,
                    placeholder: &'static str,
                    rows: usize,
                    window: &mut Window,
                    cx: &mut Context<Self>| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(rows, rows * 3)
                    .placeholder(placeholder)
                    .default_value(text)
            })
        };
        let details = area(
            card.map_or(String::new(), |card| card.details.clone()),
            "Details: what done looks like, files, constraints",
            6,
            window,
            cx,
        );
        let acceptance = area(
            card.map_or(String::new(), |card| card.acceptance.clone()),
            "What the change must do to merge. Only you write these; Auto's reviewer checks the change against them",
            3,
            window,
            cx,
        );
        let mut edits = vec![cx.subscribe_in(
            &title,
            window,
            |pane, title, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } if pane.open == Some(Open::New) => pane.add(cx),
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    let typed = title.read(cx).value().trim().to_owned();
                    pane.commit("title", typed, cx);
                    let _ = window;
                }
                _ => {}
            },
        )];
        for (field, key) in [(&details, "details"), (&acceptance, "acceptance")] {
            edits.push(cx.subscribe_in(
                field,
                window,
                move |pane, field, event: &InputEvent, _, cx| {
                    if let InputEvent::Blur = event {
                        let typed = field.read(cx).value().trim().to_owned();
                        pane.commit(key, typed, cx);
                    }
                },
            ));
        }
        if card.is_none() {
            title.update(cx, |title, cx| title.focus(window, cx));
        }
        Fields {
            title,
            details,
            acceptance,
            parent: 0,
            depends_on: Vec::new(),
            _edits: edits,
        }
    }

    /// Saves what was typed into the open card's `key`, when it changed.
    fn commit(&mut self, key: &'static str, typed: String, cx: &mut Context<Self>) {
        let Some(Open::Card(id)) = self.open else {
            return;
        };
        let Some(card) = self.card(id) else {
            return;
        };
        let now = match key {
            "title" => &card.title,
            "details" => &card.details,
            _ => &card.acceptance,
        };
        if *now == typed || (key == "title" && typed.is_empty()) {
            return;
        }
        let mut params = self.card_change(id);
        match key {
            "title" => params.title = Some(typed),
            "details" => params.details = Some(typed),
            _ => params.acceptance = Some(typed),
        }
        self.ask::<call::BoardUpdate>(params, |_, _| {}, cx);
    }

    /// Changes how the project's board runs on its own: what `change` sets, the rest as it
    /// is.
    fn read_links(&self, cx: &mut Context<Self>) {
        let asked =
            core::Target::here(self.asks()).request_typed::<call::RemotePairing>(NoParams {});
        cx.spawn(async move |pane, cx| {
            if let Ok(pairing) = asked.await {
                let _ = pane.update(cx, |pane, cx| {
                    pane.links = pairing.links;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn change_settings(
        &mut self,
        change: impl FnOnce(&mut BoardSettingsParams),
        cx: &mut Context<Self>,
    ) {
        let mut params = BoardSettingsParams {
            project: self.folder.clone(),
            ..BoardSettingsParams::default()
        };
        change(&mut params);
        self.ask::<call::BoardSettings>(params, |_, _| {}, cx);
    }

    /// A change to card `id` that changes nothing yet.
    pub(crate) fn card_change(&self, id: u64) -> BoardUpdateParams {
        BoardUpdateParams {
            project: self.folder.clone(),
            id,
            title: None,
            details: None,
            acceptance: None,
            promise: None,
            asserts: None,
            status: None,
            result: None,
            depends_on: None,
            parent: None,
        }
    }

    /// Adds the new card, then opens it.
    pub(crate) fn add(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = &self.fields else {
            return;
        };
        let title = fields.title.read(cx).value().trim().to_owned();
        if title.is_empty() || self.busy {
            return;
        }
        let text = |field: &Entity<TextareaState>| {
            let typed = field.read(cx).value().trim().to_owned();
            (!typed.is_empty()).then_some(typed)
        };
        let params = BoardAddParams {
            project: self.folder.clone(),
            title,
            details: text(&fields.details),
            acceptance: text(&fields.acceptance),
            promise: None,
            asserts: Vec::new(),
            parent: (fields.parent != 0).then_some(fields.parent),
            depends_on: fields.depends_on.clone(),
            ticket: None,
            owner: None,
        };
        self.ask::<call::BoardAdd>(
            params,
            |pane, cx| {
                let newest = pane.cards().iter().map(|card| card.id).max();
                pane.open = newest.map(Open::Card);
                pane.fields = None;
                cx.notify();
            },
            cx,
        );
    }
}

impl Focusable for BoardPane {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BoardPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.find.is_none() {
            let find = cx.new(|cx| InputState::new(window, cx).placeholder("Find a card"));
            self._typing = Some(cx.subscribe(&find, |pane, find, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    pane.words = find.read(cx).value().to_string();
                    cx.notify();
                }
            }));
            self.find = Some(find);
        }
        if let (Some(Open::Card(id)), None) = (self.open, &self.fields) {
            if let Some(card) = self.card(id).cloned() {
                self.fields = Some(self.fields_for(Some(&card), window, cx));
            }
        }
        let panel = crate::board::panels::panel(self, cx);
        div()
            .id("board")
            .key_context("AgentBoard")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .min_h_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(head(self, cx))
                    .child(body(self, cx)),
            )
            .children(panel.map(|panel| ui::floating_panel("board-panel", cx).child(panel)))
    }
}

fn head(pane: &BoardPane, cx: &mut Context<BoardPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let (later, cards): (Vec<&BoardCard>, Vec<&BoardCard>) = pane
        .cards()
        .iter()
        .partition(|card| card.aside_at.is_some());
    let from_tickets = cards.iter().filter(|card| card.ticket.is_some()).count();
    let needing = cards
        .iter()
        .filter(|card| pane.column_of(card) == BoardColumn::Needs)
        .count();
    let mut shows = vec![(Show::All, "All", cards.len())];
    if from_tickets > 0 {
        shows.push((Show::Tickets, "From Jira", from_tickets));
    }
    shows.push((Show::Needs, "Needs you", needing));
    if !later.is_empty() || pane.show == Show::Later {
        shows.push((Show::Later, "Later", later.len()));
    }
    let chosen = shows
        .iter()
        .position(|(show, ..)| *show == pane.show)
        .unwrap_or(0);
    let picks: Vec<Show> = shows.iter().map(|(show, ..)| *show).collect();
    let words = shows
        .iter()
        .map(|(_, label, count)| (SharedString::from(*label), Some(*count as u64)))
        .collect();
    let settings = pane
        .board
        .as_ref()
        .map(|board| (&board.settings, board.running));
    let triggers = pane.board.as_ref().map_or(0, |board| board.triggers.len());
    let auto_on = settings.is_some_and(|(settings, _)| settings.auto);
    let auto_count = settings
        .filter(|(settings, _)| settings.auto)
        .map(|(settings, running)| format!("{running} of {}", settings.slots));
    let auto_tip = pane
        .board
        .as_ref()
        .and_then(|board| board.auto_note.clone())
        .unwrap_or_else(|| if auto_on { "Auto is on" } else { "Auto is off" }.to_owned());
    let header_button =
        |id: &'static str, label: String, icon: Option<Icon>, pressed: bool, open: Open| {
            let mut button = Button::new(id, label, Variant::Outline).pressed(pressed);
            if let Some(icon) = icon {
                button = button.icon(icon);
            }
            button.on_click(cx.listener(move |pane, _, window, cx| {
                let next = (pane.open != Some(open)).then_some(open);
                pane.select(next, window, cx);
            }))
        };
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(SPACE_2)
        .gap_y(SPACE_1H)
        .px(SPACE_3)
        .pt(SPACE_2H)
        .pb(SPACE_2)
        .bg(crate::see_through::ground(t.surface, cx))
        .border_b_1()
        .border_color(t.border)
        .child(
            div().w(px(200.)).children(
                pane.find
                    .as_ref()
                    .map(|find| Field::new(find).icon(Icon::Search)),
            ),
        )
        .child(
            Segmented::words("board-show", words, chosen).on_select(cx.listener(
                move |pane, chosen: &usize, _, cx| {
                    pane.show = picks.get(*chosen).copied().unwrap_or(Show::All);
                    cx.notify();
                },
            )),
        )
        .child(div().flex_1())
        .child(
            header_button(
                "board-auto",
                "Auto".to_owned(),
                None,
                pane.open == Some(Open::Auto),
                Open::Auto,
            )
            .when_some(auto_count, |button, count| {
                button.trail(
                    div()
                        .opacity(0.7)
                        .font_weight(FontWeight::NORMAL)
                        .font_features(crate::theme::tabular())
                        .child(count),
                )
            })
            .lead(div().size(px(6.)).rounded_full().bg(if auto_on {
                t.live
            } else {
                t.text_tertiary
            }))
            .tooltip(auto_tip),
        )
        .when(triggers > 0, |head| {
            head.child(
                header_button(
                    "board-triggers",
                    "Triggers".into(),
                    None,
                    pane.open == Some(Open::Triggers),
                    Open::Triggers,
                )
                .trail(
                    div()
                        .opacity(0.7)
                        .font_weight(FontWeight::NORMAL)
                        .font_features(crate::theme::tabular())
                        .child(triggers.to_string()),
                ),
            )
        })
        .child(header_button(
            "board-messages",
            "Messages".into(),
            None,
            pane.open == Some(Open::Messages),
            Open::Messages,
        ))
        .child(header_button(
            "board-add",
            "Add card".into(),
            Some(Icon::Plus),
            pane.open == Some(Open::New),
            Open::New,
        ))
}

fn body(pane: &BoardPane, cx: &mut Context<BoardPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let note = |title: Option<&'static str>, message: &'static str| {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(SPACE_1H)
            .text_color(t.text_tertiary)
            .when_some(title, |empty, title| {
                empty.child(
                    ui::text(title, TEXT_SM, FontWeight::MEDIUM).text_color(t.text_secondary),
                )
            })
            .child(ui::meta(message, cx))
            .into_any_element()
    };
    if pane.board.is_none() {
        return note(None, "reading the board…");
    }
    if pane.cards().is_empty() {
        return note(
            Some("No cards yet"),
            "Agents plan their work here with board_add, and you can add cards of your own.",
        );
    }
    let lanes = pane.lanes();
    let undrawn = pane.undrawn_done();
    if lanes.is_empty() {
        return note(None, "No card matches.");
    }
    div()
        .id("board-lanes")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(SPACE_3)
        .pt(SPACE_1)
        .pb(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .children(
            lanes
                .into_iter()
                .map(|lane| lane_view(pane, lane, &undrawn, cx)),
        )
        .into_any_element()
}

fn lane_view(
    pane: &BoardPane,
    lane: Lane,
    undrawn: &HashSet<u64>,
    cx: &mut Context<BoardPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let folded = pane.folded.contains(&lane.key);
    let key = lane.key.clone();
    let count = |list: usize| {
        if list == 1 {
            "1 card".to_owned()
        } else {
            format!("{list} cards")
        }
    };
    let lead = lane.parent.and_then(|id| pane.card(id));
    let lead_owner = lead
        .filter(|parent| parent.owner.is_some())
        .map(|parent| owner(parent, cx));
    let (title, meta, select, under) = match lead {
        Some(parent) => {
            let under = pane.under_card(parent.id);
            let above = parent
                .parent
                .and_then(|above| pane.card(above))
                .map(|above| format!("under {}", short_title(&above.title)));
            let meta = [
                Some(format!("{} of {} done", under.done, under.total)),
                parent.ticket.as_ref().map(|ticket| ticket.key.clone()),
                above,
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            (parent.title.clone(), meta, Some(parent.id), Some(under))
        }
        None => {
            let auto = pane.board.as_ref().is_some_and(|board| board.settings.auto);
            let meta = if auto {
                format!("{} · Auto picks up the open ones", count(lane.cards.len()))
            } else {
                count(lane.cards.len())
            };
            ("On their own".to_owned(), meta, None, None)
        }
    };
    let progress = under
        .filter(|under| under.total > 0)
        .map(|under| 100. * under.done as f64 / under.total as f64);
    div()
        .id(SharedString::from(format!("lane-{}", lane.key)))
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(34.))
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .border_b_1()
                .border_color(t.border)
                .child(
                    IconButton::new(
                        "lane-fold",
                        Icon::Chevron,
                        if folded {
                            "Show its cards"
                        } else {
                            "Hide its cards"
                        },
                    )
                    .small()
                    .turned(if folded {
                        0.
                    } else {
                        std::f32::consts::FRAC_PI_2
                    })
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        if !pane.folded.remove(&key) {
                            pane.folded.insert(key.clone());
                        }
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .id("lane-title")
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .when_some(select, |head, id| {
                            head.cursor_pointer().on_click(cx.listener(
                                move |pane, _, window, cx| {
                                    pane.select(Some(Open::Card(id)), window, cx)
                                },
                            ))
                        })
                        .child(
                            ui::text(title, TEXT_SM, FontWeight::SEMIBOLD)
                                .text_color(t.text_primary)
                                .truncate(),
                        )
                        .children(
                            progress.map(|percent| {
                                ui::ring(Some((percent, t.live)), 12., 2., t.border)
                            }),
                        )
                        .child(ui::meta(meta, cx).flex_none()),
                )
                .child(div().flex_1())
                .children(lead_owner.map(|owner| div().pr(SPACE_2).child(owner))),
        )
        .when(!folded, |lane_view| {
            lane_view.child(groups(pane, &lane, undrawn, cx))
        })
        .into_any_element()
}

/// A lane's cards in their groups, those that need the person first and the done ones folded
/// away until asked for. While a card is dragged, every group a card can be dropped in shows.
fn groups(
    pane: &BoardPane,
    lane: &Lane,
    undrawn: &HashSet<u64>,
    cx: &mut Context<BoardPane>,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let dragging = cx.has_active_drag();
    let mut first = true;
    let mut drawn = Vec::new();
    for &(column, name) in &GROUPS {
        let cards: Vec<&BoardCard> = (lane.cards.iter())
            .filter_map(|id| pane.card(*id))
            .filter(|card| pane.column_of(card) == column)
            .collect();
        let dropped = status_dropped_in(column);
        if cards.is_empty() && !(dragging && dropped.is_some()) {
            continue;
        }
        let shown = column != BoardColumn::Done || pane.done_shown.contains(&lane.key);
        let mut group = Group::new(name, "Drop a card here")
            .total(Some(cards.len()))
            .counted();
        if first {
            group = group.first();
            first = false;
        }
        if column == BoardColumn::Done {
            let key = lane.key.clone();
            group = group.folds(
                !shown,
                cx.listener(move |pane, _, _, cx| {
                    if !pane.done_shown.remove(&key) {
                        pane.done_shown.insert(key.clone());
                    }
                    cx.notify();
                }),
            );
        }
        let rows: Vec<AnyElement> = if shown {
            (cards.into_iter())
                .filter(|card| !undrawn.contains(&card.id))
                .map(|card| card_row(pane, card, column, cx))
                .collect()
        } else {
            Vec::new()
        };
        drawn.push(
            div()
                .id(SharedString::from(format!("group-{column:?}")))
                .rounded(theme::RADIUS_2)
                .when_some(dropped, |group, status| {
                    group
                        .drag_over::<DraggedCard>(move |style, _, _, _| style.bg(t.surface_hover))
                        .on_drop(cx.listener(move |pane, dragged: &DraggedCard, _, cx| {
                            pane.move_card(dragged.id, status, cx)
                        }))
                })
                .child(group.children(rows)),
        );
    }
    div()
        .flex()
        .flex_col()
        .pl(px(18.))
        .pt(SPACE_1H)
        .pb(SPACE_2)
        .children(drawn)
}

/// One card as a row: how it stands, its title, the line that says why, its gates, its agent
/// and how long since it changed.
fn card_row(
    pane: &BoardPane,
    card: &BoardCard,
    column: BoardColumn,
    cx: &mut Context<BoardPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    let question = (card.question.as_deref()).and_then(|id| {
        let bench = pane.bench.upgrade()?;
        let bench = bench.read(cx);
        let asked = bench.feeds.questions().find(|question| question.id == id)?;
        Some(format!("@{} asks: {}", asked.name, asked.text))
    });
    let under = pane.under_card(id);
    let after = (card.waits_on.first())
        .or_else(|| card.depends_on.first())
        .and_then(|id| pane.card(*id));
    let line = question
        .or_else(|| {
            card.auto
                .as_ref()
                .filter(|_| card.status != CardStatus::Done)
                .and_then(|run| run.note.clone())
        })
        .or_else(|| (card.aside_at).map(|at| format!("Put aside {}", crate::ui::time::ago(at))))
        .or_else(|| after.map(|after| format!("after {}", short_title(&after.title))))
        .or_else(|| {
            (under.total > 0).then(|| format!("{} of {} under it done", under.done, under.total))
        });
    let chosen = pane.open == Some(Open::Card(id));
    let moving = pane.moving.contains_key(&id);
    let label: SharedString = card.title.clone().into();
    let mark = match column {
        BoardColumn::Needs => ui::dot(Tone::Pending, cx).into_any_element(),
        BoardColumn::Progress => ui::dot(Tone::Ok, cx).into_any_element(),
        BoardColumn::Open => ui::dot(Tone::Idle, cx).into_any_element(),
        BoardColumn::Done => ui::icon(Icon::Check, 12.)
            .text_color(t.live)
            .into_any_element(),
    };
    let mut row = Row::new(SharedString::from(format!("card-{id}")), label.clone())
        .mark(14., mark)
        .chosen(chosen)
        .on_click(
            cx.listener(move |pane, _, window, cx| pane.select(Some(Open::Card(id)), window, cx)),
        );
    if column == BoardColumn::Done {
        row = row.quiet();
    }
    let row = row
        .child(
            div()
                .w(px(REASON_WIDTH))
                .flex_none()
                .min_w_0()
                .children(line.map(|line| ui::meta(line, cx).truncate())),
        )
        .children(
            card.ticket
                .as_ref()
                .map(|ticket| ui::mono(ticket.key.clone(), TEXT_2XS, cx).flex_none()),
        )
        .child(gates(card, &t))
        .child(
            div()
                .w(px(OWNER_WIDTH))
                .flex_none()
                .min_w_0()
                .overflow_hidden()
                .children(card.owner.as_ref().map(|_| owner(card, cx))),
        )
        .child(
            ui::meta(crate::ui::time::age(card.updated_at), cx)
                .w(px(28.))
                .flex_none()
                .text_right(),
        );
    div()
        .id(SharedString::from(format!("card-drag-{id}")))
        .when(moving, |row| row.opacity(0.6))
        .on_drag(
            DraggedCard {
                id,
                label: label.clone(),
            },
            |dragged: &DraggedCard, _, _, cx| ghost(dragged.label.clone(), cx),
        )
        .child(row)
        .into_any_element()
}

/// A slice's gates in a line, each a dot that fills once it passed; the one waiting on the
/// person rings in amber. Nothing for a card that is no slice.
fn gates(card: &BoardCard, t: &Tokens) -> gpui_kit::Div {
    div()
        .w(px(GATES_WIDTH))
        .flex_none()
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .children(
            card.slice
                .iter()
                .flat_map(|slice| &slice.gates)
                .map(|gate| {
                    let you = gate.state == GateState::Waiting && gate.passer == Passer::Person;
                    let node = div().size(px(6.)).rounded_full().flex_none();
                    let node = match gate.state {
                        GateState::Passed => node.bg(t.live),
                        GateState::Waiting => {
                            node.border_1()
                                .border_color(if you { t.warn } else { t.live })
                        }
                        GateState::Ahead => node.border_1().border_color(t.text_tertiary),
                    };
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .child(node)
                        .child(
                            ui::text(
                                crate::goals::step_name(gate).to_owned(),
                                TEXT_2XS,
                                FontWeight::NORMAL,
                            )
                            .whitespace_nowrap()
                            .text_color(
                                if gate.state == GateState::Passed {
                                    t.text_secondary
                                } else {
                                    t.text_tertiary
                                },
                            ),
                        )
                }),
        )
}

/// A card's ticket: its key, what it is in its tracker, and a link to it there.
pub(crate) fn ticket_line(ticket: &CardTicket, cx: &gpui_kit::App) -> gpui_kit::Div {
    let url = ticket.url.clone();
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(ui::mono(ticket.key.clone(), TEXT_2XS_H, cx))
        .when_some(ticket.source.clone(), |head, source| {
            head.child(ui::meta(source, cx))
        })
        .when_some(url, |head, url| {
            head.child(
                IconButton::new("ticket-open", Icon::Link, "Open the ticket in the browser")
                    .small()
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
        })
}

/// The agent a card names, by its mark and name.
pub(crate) fn owner(card: &BoardCard, cx: &gpui_kit::App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .when_some(card.owner_cli.as_deref(), |owner, cli| {
            owner.child(ui::mark(cli, 12.))
        })
        .child(
            ui::text(
                (card.owner_name.as_ref())
                    .map_or_else(|| "An agent".to_owned(), |name| format!("@{name}")),
                TEXT_2XS_H,
                FontWeight::MEDIUM,
            )
            .text_color(t.text_secondary)
            .min_w_0()
            .truncate(),
        )
        .when_some(card.paid_by.clone(), |owner, paid| {
            owner.child(ui::meta(paid, cx))
        })
        .into_any_element()
}

impl Workbench {
    /// Makes the board pane when the project's board tab comes to the front.
    pub(crate) fn board_pane(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project() else {
            return;
        };
        let folder = project.path.to_string_lossy().into_owned();
        let pane = match self.board.pane.clone() {
            Some(pane) if pane.read(cx).folder == folder => pane,
            _ => {
                let (asks, bench) = (self.asks.clone(), cx.entity().downgrade());
                let pane =
                    cx.new(|cx| crate::board::BoardPane::new(asks, folder.clone(), bench, cx));
                self.board.pane = Some(pane.clone());
                pane
            }
        };
        if let Some((_, card)) = self.board.card.take_if(|(at, _)| *at == folder) {
            pane.update(cx, |pane, cx| pane.reveal(card, cx));
        }
    }
}
