//! The agent board of a project: its cards in columns by where they stand, a lane for
//! each card that has cards under it, and beside the board one card to read and change,
//! Auto's settings, or the messages between the project's agents. The core keeps the
//! board and says which column each card is in; the pane only draws and asks.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window,
};
use serde::Serialize;
use serde_json::json;
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::{
    AgentMessagesParams, BoardAddParams, BoardParams, BoardSettingsParams, BoardUpdateParams,
    BoardView, CardStatus, CardView, Column, MessageRow, Stage,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{self, Ask};
use crate::drag::{ghost, DraggedCard};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, TEXT_2XS,
    TEXT_2XS_H, TEXT_3XS, TEXT_SM, TEXT_XS_H,
};
use crate::toast;
use crate::ui::{self, Button, Field, IconButton, Segmented, Variant};
use crate::workbench::Workbench;

pub mod panels;

const COLUMNS: [(Column, &str); 4] = [
    (Column::Open, "Open"),
    (Column::Progress, "In progress"),
    (Column::Needs, "Needs you"),
    (Column::Done, "Done"),
];

/// A run stage that waits for the person.
pub(crate) fn waits_on_person(stage: Stage) -> bool {
    matches!(stage, Stage::Review | Stage::Held)
}

/// The column a card moving to `status` shows in until the core answers.
fn column_moving_to(status: CardStatus) -> Column {
    match status {
        CardStatus::Open => Column::Open,
        CardStatus::Progress => Column::Progress,
        CardStatus::Done => Column::Done,
    }
}

/// What a card dropped in `column` is moved to; none for the column the core alone fills.
fn status_dropped_in(column: Column) -> Option<CardStatus> {
    match column {
        Column::Open => Some(CardStatus::Open),
        Column::Progress => Some(CardStatus::Progress),
        Column::Needs => None,
        Column::Done => Some(CardStatus::Done),
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
}

/// What is open beside the board.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Open {
    Card(u64),
    New,
    Auto,
    Messages,
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
struct Lane {
    key: String,
    parent: Option<u64>,
    cards: Vec<u64>,
}

pub struct BoardPane {
    asks: UnboundedSender<Ask>,
    /// The project's folder, which the core's board calls take.
    pub folder: String,
    pub(crate) bench: WeakEntity<Workbench>,
    pub(crate) board: Option<BoardView>,
    find: Option<Entity<InputState>>,
    words: String,
    show: Show,
    pub(crate) open: Option<Open>,
    pub(crate) fields: Option<Fields>,
    /// Cards on their way to another status, shown there until the core answers.
    moving: HashMap<u64, CardStatus>,
    folded: HashSet<String>,
    pub(crate) messages: Option<Vec<MessageRow>>,
    pub(crate) busy: bool,
    focus: FocusHandle,
    _typing: Option<Subscription>,
    _reading: Task<()>,
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
            moving: HashMap::new(),
            folded: HashSet::new(),
            messages: None,
            busy: false,
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

    fn take(&mut self, board: BoardView) {
        if let Some(Open::Card(id)) = self.open {
            if !board.cards.iter().any(|card| card.id == id) {
                self.open = None;
                self.fields = None;
            }
        }
        self.board = Some(board);
    }

    pub(crate) fn said(&self, error: &str, cx: &mut Context<Self>) {
        let note = toast::error(error.to_owned()).autohide(false);
        let _ = self.bench.update(cx, |bench, cx| bench.notify(note, cx));
    }

    /// Asks the core to change the board, then shows the board it answers.
    pub(crate) fn ask<C: Call<Answer = BoardView> + 'static>(
        &mut self,
        params: C::Params,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        C::Params: Serialize,
    {
        let asked = core::Target::here(&self.asks).request_typed::<C>(params);
        self.busy = true;
        cx.notify();
        cx.spawn(async move |pane, cx| {
            let answer = asked.await;
            let _ = pane.update(cx, |pane, cx| {
                pane.busy = false;
                match answer {
                    Ok(board) => {
                        pane.take(board);
                        then(pane, cx);
                    }
                    Err(error) => pane.said(&error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn move_card(&mut self, id: u64, to: CardStatus, cx: &mut Context<Self>) {
        let Some(card) = self.card(id) else {
            return;
        };
        if card.status == to {
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

    pub(crate) fn card(&self, id: u64) -> Option<&CardView> {
        self.board.as_ref()?.cards.iter().find(|card| card.id == id)
    }

    pub(crate) fn cards(&self) -> &[CardView] {
        self.board
            .as_ref()
            .map_or(&[], |board| board.cards.as_slice())
    }

    fn column_of(&self, card: &CardView) -> Column {
        self.moving
            .get(&card.id)
            .map_or(card.column, |status| column_moving_to(*status))
    }

    fn shown(&self, card: &CardView) -> bool {
        let wanted = self.words.trim().to_lowercase();
        let matches = |text: &str| text.to_lowercase().contains(&wanted);
        (self.show != Show::Tickets || card.ticket.is_some())
            && (self.show != Show::Needs || self.column_of(card) == Column::Needs)
            && (wanted.is_empty()
                || matches(&card.title)
                || matches(&card.details)
                || card.owner_name.as_deref().is_some_and(matches)
                || card
                    .ticket
                    .as_ref()
                    .is_some_and(|ticket| matches(&ticket.key)))
    }

    /// One lane per card that has cards under it, holding them, then one for the cards
    /// on their own; the newest work first.
    fn lanes(&self) -> Vec<Lane> {
        let cards = self.cards();
        let parents: HashSet<u64> = cards.iter().filter_map(|card| card.parent).collect();
        let mut shown: Vec<&CardView> = cards.iter().filter(|card| self.shown(card)).collect();
        shown.sort_by_key(|card| std::cmp::Reverse(card.updated_at));
        let mut lanes: Vec<Lane> = Vec::new();
        let mut alone = Lane {
            key: "alone".into(),
            parent: None,
            cards: Vec::new(),
        };
        let lane_of = |parent: u64, lanes: &mut Vec<Lane>| -> usize {
            match lanes.iter().position(|lane| lane.parent == Some(parent)) {
                Some(place) => place,
                None => {
                    lanes.push(Lane {
                        key: format!("card-{parent}"),
                        parent: Some(parent),
                        cards: Vec::new(),
                    });
                    lanes.len() - 1
                }
            }
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
        cx.notify();
    }

    fn fields_for(
        &self,
        card: Option<&CardView>,
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
            parent: (fields.parent != 0).then_some(fields.parent),
            depends_on: fields.depends_on.clone(),
            ticket: None,
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
        let t = Tokens::get(cx).clone();
        let panel = crate::board::panels::panel(self, cx);
        div()
            .id("board")
            .key_context("AgentBoard")
            .track_focus(&self.focus)
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
                    .children(triggers(self, cx))
                    .child(body(self, cx)),
            )
            .when_some(panel, |pane, panel| {
                pane.child(
                    div()
                        .id("board-panel")
                        .w(px(340.))
                        .flex_none()
                        .border_l_1()
                        .border_color(t.border)
                        .overflow_y_scroll()
                        .child(panel),
                )
            })
    }
}

fn head(pane: &BoardPane, cx: &mut Context<BoardPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let cards = pane.cards();
    let from_tickets = cards.iter().filter(|card| card.ticket.is_some()).count();
    let needing = cards
        .iter()
        .filter(|card| pane.column_of(card) == Column::Needs)
        .count();
    let mut shows = vec![(Show::All, "All", cards.len())];
    if from_tickets > 0 {
        shows.push((Show::Tickets, "From Jira", from_tickets));
    }
    shows.push((Show::Needs, "Needs you", needing));
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

/// The triggers that add cards to this board when their time comes, each of which the person
/// can remove.
fn triggers(pane: &BoardPane, cx: &mut Context<BoardPane>) -> Option<AnyElement> {
    let set = &pane.board.as_ref()?.triggers;
    if set.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let rows: Vec<AnyElement> = set
        .iter()
        .map(|trigger| {
            let id = trigger.id;
            crate::chat::live::trigger_row(
                trigger,
                &t,
                cx.listener(move |pane, _, _, cx| {
                    let asked =
                        core::request(&pane.asks, "removeTrigger", json!({ "trigger": id }));
                    cx.spawn(async move |_, _| drop(asked.await)).detach();
                }),
            )
        })
        .collect();
    Some(
        div()
            .flex_none()
            .pt(SPACE_1H)
            .pb(SPACE_1)
            .px(SPACE_1)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .pl(px(11.))
                    .pb(SPACE_0)
                    .flex()
                    .gap(SPACE_1H)
                    .text_size(crate::theme::ui_text(TEXT_3XS))
                    .text_color(t.text_disabled)
                    .child("TRIGGERS")
                    .child(
                        div()
                            .font_family(crate::theme::MONO)
                            .text_color(t.text_tertiary)
                            .child(set.len().to_string()),
                    ),
            )
            .children(rows)
            .into_any_element(),
    )
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
    if lanes.is_empty() {
        return note(None, "No card matches.");
    }
    div()
        .id("board-lanes")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(SPACE_3)
        .pt(SPACE_2H)
        .pb(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_3H)
        .children(lanes.into_iter().map(|lane| lane_view(pane, lane, cx)))
        .into_any_element()
}

fn lane_view(pane: &BoardPane, lane: Lane, cx: &mut Context<BoardPane>) -> AnyElement {
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
    let (title, meta, select): (String, String, Option<u64>) = match lead {
        Some(parent) => {
            let under: Vec<&CardView> = pane
                .cards()
                .iter()
                .filter(|other| other.parent == Some(parent.id))
                .collect();
            let done = under
                .iter()
                .filter(|other| other.status == CardStatus::Done)
                .count();
            let above = parent
                .parent
                .and_then(|above| pane.card(above))
                .map(|above| format!("under {}", short_title(&above.title)));
            let meta = [
                Some(format!("{done} of {} done", under.len())),
                parent.ticket.as_ref().map(|ticket| ticket.key.clone()),
                above,
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            (parent.title.clone(), meta, Some(parent.id))
        }
        None => {
            let auto = pane.board.as_ref().is_some_and(|board| board.settings.auto);
            let meta = if auto {
                format!("{} · Auto picks up the open ones", count(lane.cards.len()))
            } else {
                count(lane.cards.len())
            };
            ("On their own".to_owned(), meta, None)
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(
                    IconButton::new(
                        SharedString::from(format!("fold-{key}")),
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
                        .id(SharedString::from(format!("lane-{}", lane.key)))
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
                                .text_color(t.text_primary),
                        )
                        .child(ui::meta(meta, cx)),
                )
                .children(lead_owner),
        )
        .when(!folded, |lane_view| {
            lane_view.child(columns(pane, &lane.cards, cx))
        })
        .into_any_element()
}

fn columns(pane: &BoardPane, ids: &[u64], cx: &mut Context<BoardPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    div()
        .flex()
        .gap(SPACE_2H)
        .children(COLUMNS.iter().map(|&(column, name)| {
            let cards: Vec<&CardView> = ids
                .iter()
                .filter_map(|id| pane.card(*id))
                .filter(|card| pane.column_of(card) == column)
                .collect();
            div()
                .id(SharedString::from(format!(
                    "column-{column:?}-{}",
                    ids.first().copied().unwrap_or(0)
                )))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .px(SPACE_1H)
                .when(!cards.is_empty(), |column| column.pb(SPACE_1H))
                .rounded(theme::RADIUS_3)
                .bg(t.surface_sunken)
                .when_some(status_dropped_in(column), |column, status| {
                    column
                        .drag_over::<DraggedCard>(move |style, _, _, _| style.bg(t.surface_hover))
                        .on_drop(cx.listener(move |pane, dragged: &DraggedCard, _, cx| {
                            pane.move_card(dragged.id, status, cx)
                        }))
                })
                .child(
                    div()
                        .flex_none()
                        .h(px(34.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(SPACE_1H)
                        .child(ui::caps(name, cx))
                        .child(ui::meta(cards.len().to_string(), cx)),
                )
                .children(cards.into_iter().map(|card| card_view(pane, card, cx)))
        }))
}

fn card_view(pane: &BoardPane, card: &CardView, cx: &mut Context<BoardPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    let cards = pane.cards();
    let question = (pane.column_of(card) == Column::Needs)
        .then(|| {
            let owner = card.owner.as_deref()?;
            let bench = pane.bench.upgrade()?;
            let bench = bench.read(cx);
            bench
                .chats
                .questions
                .iter()
                .find_map(|question| question.open_from(owner))
                .map(|(name, text)| format!("{name} asks: {text}"))
        })
        .flatten();
    let line = question.or_else(|| {
        card.auto
            .as_ref()
            .filter(|_| card.status != CardStatus::Done)
            .and_then(|run| run.note.clone())
    });
    let under: Vec<&CardView> = cards
        .iter()
        .filter(|other| other.parent == Some(id))
        .collect();
    let done = under
        .iter()
        .filter(|other| other.status == CardStatus::Done)
        .count();
    let waits: Vec<&CardView> = card
        .waits_on
        .iter()
        .filter_map(|id| pane.card(*id))
        .collect();
    let after = waits
        .first()
        .copied()
        .or_else(|| card.depends_on.first().and_then(|id| pane.card(*id)));
    let chosen = pane.open == Some(Open::Card(id));
    let moving = pane.moving.contains_key(&id);
    let label: SharedString = card.title.clone().into();
    div()
        .id(SharedString::from(format!("card-{id}")))
        .flex()
        .flex_col()
        .gap(px(5.))
        .p(SPACE_2)
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(if chosen { t.border_strong } else { t.border })
        .bg(if chosen { t.surface_chosen } else { t.surface })
        .when(moving || card.status == CardStatus::Done, |card| {
            card.opacity(0.6)
        })
        .cursor_pointer()
        .hover(move |style| style.border_color(t.border_strong))
        .on_drag(
            DraggedCard {
                id,
                label: label.clone(),
            },
            |dragged: &DraggedCard, _, _, cx| ghost(dragged.label.clone(), cx),
        )
        .on_click(
            cx.listener(move |pane, _, window, cx| pane.select(Some(Open::Card(id)), window, cx)),
        )
        .child(
            ui::text(label, TEXT_XS_H, FontWeight::MEDIUM)
                .whitespace_normal()
                .text_color(t.text_primary),
        )
        .when_some(line, |view, line| {
            view.child(ui::meta(line, cx).whitespace_normal())
        })
        .when(!under.is_empty() || after.is_some(), |view| {
            view.child(
                div()
                    .flex()
                    .gap(SPACE_2)
                    .when(!under.is_empty(), |row| {
                        row.child(ui::meta(format!("{done}/{}", under.len()), cx))
                    })
                    .when_some(after, |row, after| {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .child(ui::icon(Icon::Link, 10.).text_color(t.text_tertiary))
                                .child(ui::meta(
                                    format!("after {}", short_title(&after.title)),
                                    cx,
                                )),
                        )
                    }),
            )
        })
        .when(card.ticket.is_some() || card.owner.is_some(), |view| {
            view.child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .when_some(card.ticket.as_ref(), |foot, ticket| {
                        foot.child(ui::mono(ticket.key.clone(), TEXT_2XS, cx))
                            .when_some(ticket.source.clone(), |foot, source| {
                                foot.child(ui::meta(source, cx))
                            })
                    })
                    .child(div().flex_1())
                    .children(card.owner.as_ref().map(|_| owner(card, cx))),
            )
        })
        .into_any_element()
}

/// The agent a card names, by its mark and name.
pub(crate) fn owner(card: &CardView, cx: &gpui_kit::App) -> AnyElement {
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
                card.owner_name.clone().unwrap_or_else(|| "An agent".into()),
                TEXT_2XS_H,
                FontWeight::MEDIUM,
            )
            .text_color(t.text_secondary),
        )
        .when_some(card.paid_by.clone(), |owner, paid| {
            owner.child(ui::meta(paid, cx))
        })
        .into_any_element()
}
