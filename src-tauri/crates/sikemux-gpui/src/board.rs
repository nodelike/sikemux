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
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{self, Ask};
use crate::drag::{ghost, DraggedCard};
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::toast;
use crate::ui::{self, Button, Field, IconButton, Segmented, Variant};
use crate::workbench::Workbench;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Card {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub details: String,
    #[serde(default)]
    pub acceptance: String,
    pub status: String,
    pub owner: Option<String>,
    pub owner_name: Option<String>,
    pub owner_cli: Option<String>,
    pub paid_by: Option<String>,
    pub parent: Option<u64>,
    #[serde(default)]
    pub depends_on: Vec<u64>,
    #[serde(default)]
    pub waits_on: Vec<u64>,
    pub created_by: Option<String>,
    #[serde(default)]
    pub created_by_name: String,
    pub result: Option<String>,
    pub ticket: Option<Ticket>,
    pub column: String,
    pub auto: Option<Run>,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ticket {
    pub key: String,
    pub url: Option<String>,
    pub source: Option<String>,
    pub sync_note: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Run {
    pub stage: String,
    pub note: Option<String>,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub worktree: String,
    pub pull: Option<Pull>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Pull {
    pub number: u64,
    pub url: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    pub auto: bool,
    pub slots: u32,
    pub launcher: Option<String>,
    /// The connected provider Auto's agents run on in place of their own accounts.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub finish: String,
    pub reviewer: Option<Reviewer>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Reviewer {
    pub launcher: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Board {
    pub cards: Vec<Card>,
    pub settings: Settings,
    pub running: usize,
    pub auto_note: Option<String>,
}

/// One message between the project's agents, as the core folds its log.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageRow {
    pub sent_at: u64,
    pub from: Option<String>,
    pub from_name: Option<String>,
    pub to: String,
    pub to_name: Option<String>,
    pub kind: Option<String>,
    pub outcome: String,
    pub waited: bool,
    pub tone: Option<String>,
}

const COLUMNS: [(&str, &str); 4] = [
    ("open", "Open"),
    ("progress", "In progress"),
    ("needs", "Needs you"),
    ("done", "Done"),
];

/// A run stage that waits for the person.
pub(crate) fn waits_on_person(stage: &str) -> bool {
    matches!(stage, "review" | "held")
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
    pub(crate) board: Option<Board>,
    find: Option<Entity<InputState>>,
    words: String,
    show: Show,
    pub(crate) open: Option<Open>,
    pub(crate) fields: Option<Fields>,
    /// Cards on their way to another status, shown there until the core answers.
    moving: HashMap<u64, &'static str>,
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
        let read = core::request(&self.asks, "board", json!({ "project": self.folder }));
        let messages = (self.open == Some(Open::Messages)).then(|| {
            core::request(
                &self.asks,
                "agentMessageRows",
                json!({ "project": self.folder }),
            )
        });
        self._reading = cx.spawn(async move |pane, cx| {
            let board = read.await;
            let rows = match messages {
                Some(messages) => Some(messages.await),
                None => None,
            };
            let _ = pane.update(cx, |pane, cx| {
                match board {
                    Ok(board) => pane.take(board, cx),
                    Err(error) => pane.said(&format!("Couldn't read the board: {error}"), cx),
                }
                if let Some(Ok(rows)) = rows {
                    pane.messages = serde_json::from_value(rows["rows"].clone()).ok();
                }
                cx.notify();
            });
        });
    }

    fn take(&mut self, board: Value, cx: &mut Context<Self>) {
        match serde_json::from_value::<Board>(board) {
            Ok(board) => {
                if let Some(Open::Card(id)) = self.open {
                    if !board.cards.iter().any(|card| card.id == id) {
                        self.open = None;
                        self.fields = None;
                    }
                }
                self.board = Some(board);
            }
            Err(error) => self.said(&format!("Couldn't read the board: {error}"), cx),
        }
    }

    pub(crate) fn said(&self, error: &str, cx: &mut Context<Self>) {
        let note = toast::error(error.to_owned()).autohide(false);
        let _ = self.bench.update(cx, |bench, cx| bench.notify(note, cx));
    }

    /// Asks the core to change the board, then shows the board it answers.
    pub(crate) fn ask(
        &mut self,
        method: &'static str,
        mut params: Value,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        params["project"] = json!(self.folder);
        let asked = core::request(&self.asks, method, params);
        self.busy = true;
        cx.notify();
        cx.spawn(async move |pane, cx| {
            let answer = asked.await;
            let _ = pane.update(cx, |pane, cx| {
                pane.busy = false;
                match answer {
                    Ok(board) => {
                        pane.take(board, cx);
                        then(pane, cx);
                    }
                    Err(error) => pane.said(&error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn move_card(&mut self, id: u64, to: &'static str, cx: &mut Context<Self>) {
        let Some(card) = self.card(id) else {
            return;
        };
        if card.status == to {
            return;
        }
        self.moving.insert(id, to);
        self.ask(
            "boardUpdate",
            json!({ "id": id, "status": to }),
            move |pane, _| {
                pane.moving.remove(&id);
            },
            cx,
        );
    }

    pub(crate) fn card(&self, id: u64) -> Option<&Card> {
        self.board.as_ref()?.cards.iter().find(|card| card.id == id)
    }

    pub(crate) fn cards(&self) -> &[Card] {
        self.board
            .as_ref()
            .map_or(&[], |board| board.cards.as_slice())
    }

    fn column_of<'a>(&'a self, card: &'a Card) -> &'a str {
        self.moving.get(&card.id).copied().unwrap_or(&card.column)
    }

    fn shown(&self, card: &Card) -> bool {
        let wanted = self.words.trim().to_lowercase();
        let matches = |text: &str| text.to_lowercase().contains(&wanted);
        (self.show != Show::Tickets || card.ticket.is_some())
            && (self.show != Show::Needs || self.column_of(card) == "needs")
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
        let mut shown: Vec<&Card> = cards.iter().filter(|card| self.shown(card)).collect();
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
        card: Option<&Card>,
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
        let mut params = json!({ "id": id });
        params[key] = json!(typed);
        self.ask("boardUpdate", params, |_, _| {}, cx);
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
        let mut params = json!({ "title": title });
        if let Some(details) = text(&fields.details) {
            params["details"] = json!(details);
        }
        if let Some(acceptance) = text(&fields.acceptance) {
            params["acceptance"] = json!(acceptance);
        }
        if fields.parent != 0 {
            params["parent"] = json!(fields.parent);
        }
        if !fields.depends_on.is_empty() {
            params["dependsOn"] = json!(fields.depends_on);
        }
        self.ask(
            "boardAdd",
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
        let panel = crate::board_panels::panel(self, cx);
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
        .filter(|card| pane.column_of(card) == "needs")
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
        .gap_x(px(8.))
        .gap_y(px(6.))
        .px(px(12.))
        .pt(px(10.))
        .pb(px(8.))
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

fn body(pane: &BoardPane, cx: &mut Context<BoardPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let note = |title: Option<&'static str>, message: &'static str| {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .text_color(t.text_tertiary)
            .when_some(title, |empty, title| {
                empty.child(ui::text(title, 13., FontWeight::MEDIUM).text_color(t.text_secondary))
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
        .px(px(12.))
        .pt(px(10.))
        .pb(px(12.))
        .flex()
        .flex_col()
        .gap(px(14.))
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
            let under: Vec<&Card> = pane
                .cards()
                .iter()
                .filter(|other| other.parent == Some(parent.id))
                .collect();
            let done = under.iter().filter(|other| other.status == "done").count();
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
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
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
                        .gap(px(8.))
                        .when_some(select, |head, id| {
                            head.cursor_pointer().on_click(cx.listener(
                                move |pane, _, window, cx| {
                                    pane.select(Some(Open::Card(id)), window, cx)
                                },
                            ))
                        })
                        .child(
                            ui::text(title, 13., FontWeight::SEMIBOLD).text_color(t.text_primary),
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
        .gap(px(10.))
        .children(COLUMNS.iter().map(|&(key, name)| {
            let cards: Vec<&Card> = ids
                .iter()
                .filter_map(|id| pane.card(*id))
                .filter(|card| pane.column_of(card) == key)
                .collect();
            div()
                .id(SharedString::from(format!(
                    "column-{key}-{}",
                    ids.first().copied().unwrap_or(0)
                )))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(6.))
                .when(!cards.is_empty(), |column| column.pb(px(6.)))
                .rounded(theme::RADIUS_3)
                .bg(t.surface_sunken)
                .when(key != "needs", |column| {
                    column
                        .drag_over::<DraggedCard>(move |style, _, _, _| style.bg(t.surface_hover))
                        .on_drop(cx.listener(move |pane, dragged: &DraggedCard, _, cx| {
                            pane.move_card(dragged.id, key, cx)
                        }))
                })
                .child(
                    div()
                        .flex_none()
                        .h(px(34.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(6.))
                        .child(ui::caps(name, cx))
                        .child(ui::meta(cards.len().to_string(), cx)),
                )
                .children(cards.into_iter().map(|card| card_view(pane, card, cx)))
        }))
}

fn card_view(pane: &BoardPane, card: &Card, cx: &mut Context<BoardPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    let cards = pane.cards();
    let question = (pane.column_of(card) == "needs")
        .then(|| {
            let owner = card.owner.as_deref()?;
            let bench = pane.bench.upgrade()?;
            let bench = bench.read(cx);
            bench
                .questions
                .iter()
                .find_map(|question| question.open_from(owner))
                .map(|(name, text)| format!("{name} asks: {text}"))
        })
        .flatten();
    let line = question.or_else(|| {
        card.auto
            .as_ref()
            .filter(|_| card.status != "done")
            .and_then(|run| run.note.clone())
    });
    let under: Vec<&Card> = cards
        .iter()
        .filter(|other| other.parent == Some(id))
        .collect();
    let done = under.iter().filter(|other| other.status == "done").count();
    let waits: Vec<&Card> = card
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
        .p(px(8.))
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(if chosen { t.border_strong } else { t.border })
        .bg(if chosen { t.surface_chosen } else { t.surface })
        .when(moving || card.status == "done", |card| card.opacity(0.6))
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
            ui::text(label, 12.5, FontWeight::MEDIUM)
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
                    .gap(px(8.))
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
                    .gap(px(6.))
                    .when_some(card.ticket.as_ref(), |foot, ticket| {
                        foot.child(ui::mono(ticket.key.clone(), 11., cx))
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
pub(crate) fn owner(card: &Card, cx: &gpui_kit::App) -> AnyElement {
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
                11.5,
                FontWeight::MEDIUM,
            )
            .text_color(t.text_secondary),
        )
        .when_some(card.paid_by.clone(), |owner, paid| {
            owner.child(ui::meta(paid, cx))
        })
        .into_any_element()
}
