//! What opens beside a board: one card to read and change or a new one to add,
//! Auto's settings, and the messages between the project's agents.

use crate::board::ChangesBoards;
use std::rc::Rc;

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_core::server::board::BoardCard;
use sikemux_core::server::board::BoardCardParams;
use sikemux_core::server::board::BoardReviewerParams;
use sikemux_core::server::board::BoardUpdateParams;
use sikemux_core::server::board::CardRun;
use sikemux_core::server::board::CardStatus;
use sikemux_core::server::board::Landing;
use sikemux_core::server::board::{CardContract, ContractState, RunStage};
use sikemux_core::server::call;
use sikemux_core::server::messages::log::MessageOutcome;
use sikemux_core::server::messages::log::MessageRow;
use sikemux_core::server::messages::log::MessageTone;
use sikemux_core::server::messages::MessageKind;

use crate::board::{owner, short_title, BoardPane, Open};
use crate::chat::model_picker;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, TEXT_SM,
    TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, select, Button, IconButton, Pick, Toggle, Variant};

/// Keeps a choice made in a picker as the board's settings.
type Save = Rc<dyn Fn(BoardReviewerParams, &mut gpui_kit::App)>;

const SLOT_CHOICES: [u32; 14] = [1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 24, 32, 48, 64];
const NO_CARD: &str = "0";

const STATUSES: [CardStatus; 3] = [CardStatus::Open, CardStatus::Progress, CardStatus::Done];

/// A status by the name the core takes it by, and its label.
fn status_named(status: CardStatus) -> (&'static str, &'static str) {
    match status {
        CardStatus::Open => ("open", "Open"),
        CardStatus::Progress => ("progress", "In progress"),
        CardStatus::Done => ("done", "Done"),
    }
}

fn status_label(status: CardStatus) -> &'static str {
    status_named(status).1
}

fn run_label(run: &CardRun, status: CardStatus) -> &'static str {
    if status == CardStatus::Done {
        return "Done";
    }
    if run.held.is_some() {
        return "Back with you";
    }
    match run.stage {
        RunStage::Starting => "Starting",
        RunStage::Working => "Working",
        RunStage::Contracting => "Agreeing what to build",
        RunStage::Shipping => "Shipping",
        RunStage::Checking => "Checks running",
        RunStage::Reviewing => "Under review",
        RunStage::Fixing => "Fixing",
        RunStage::Merging => "Merging",
        RunStage::Watching => "Waits for you",
    }
}

pub(crate) fn panel(pane: &BoardPane, cx: &mut Context<BoardPane>) -> Option<AnyElement> {
    Some(match pane.open? {
        Open::Card(id) => card_panel(pane, pane.card(id)?.clone(), cx),
        Open::New => new_panel(pane, cx),
        Open::Auto => auto_panel(pane, cx),
        Open::Messages => messages_panel(pane, cx),
    })
}

fn head(title: AnyElement, cx: &mut Context<BoardPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    div()
        .flex_none()
        .min_h(px(45.))
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .pl(SPACE_3)
        .pr(SPACE_2)
        .bg(crate::see_through::ground(t.surface, cx))
        .border_b_1()
        .border_color(t.border)
        .child(title)
        .child(div().flex_1())
        .child(
            IconButton::new("board-panel-close", Icon::Close, "Close")
                .small()
                .on_click(cx.listener(|pane, _, window, cx| pane.select(None, window, cx))),
        )
}

fn section(name: &'static str, cx: &mut Context<BoardPane>) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .px(SPACE_3H)
        .py(SPACE_2H)
        .child(ui::caps(name, cx))
}

fn fact(name: &'static str, value: AnyElement, cx: &mut Context<BoardPane>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(ui::caps(name, cx).w(px(64.)).flex_none())
        .child(div().flex_1().min_w_0().child(value))
        .into_any_element()
}

/// The two ways a project's slices land, as the board's and the Goals view's settings offer them.
pub(crate) fn landing_picks() -> Vec<Pick> {
    vec![
        Pick::new("core", "Once on the branch")
            .hint("Sikemux passes Landed, and Auto merges its pull request itself"),
        Pick::new("person", "When you pass it").hint(
            "Nothing counts as landed until you pass Landed; Auto leaves its pull request for you to merge",
        ),
    ]
}

/// The pick and words shown for `landing`.
pub(crate) fn landing_choice(landing: Landing) -> (&'static str, &'static str) {
    match landing {
        Landing::Core => ("core", "Once on the branch"),
        Landing::Person => ("person", "When you pass it"),
    }
}

pub(crate) fn landing_of(key: &str) -> Landing {
    match key {
        "person" => Landing::Person,
        _ => Landing::Core,
    }
}

/// A group of settings running edge to edge under a hairline, its name above its fields.
fn settings_group(name: &'static str, cx: &mut Context<BoardPane>) -> gpui_kit::Div {
    let t = Tokens::get(cx).clone();
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .mt(SPACE_1)
        .px(SPACE_3H)
        .pt(SPACE_3)
        .pb(SPACE_2H)
        .border_t_1()
        .border_color(t.border)
        .child(ui::caps(name, cx))
}

/// The cards under `id`, and theirs, which it cannot be put under.
fn below(cards: &[BoardCard], id: u64) -> Vec<u64> {
    let mut found = vec![id];
    let mut grew = true;
    while grew {
        grew = false;
        for card in cards {
            if card.parent.is_some_and(|parent| found.contains(&parent))
                && !found.contains(&card.id)
            {
                found.push(card.id);
                grew = true;
            }
        }
    }
    found
}

/// The card it goes under, and the cards it waits on, for a card or a new one.
fn placing(
    pane: &BoardPane,
    card: Option<&BoardCard>,
    cx: &mut Context<BoardPane>,
) -> Vec<AnyElement> {
    let cards = pane.cards();
    let (parent, depends_on) = match (card, &pane.fields) {
        (Some(card), _) => (card.parent.unwrap_or(0), card.depends_on.clone()),
        (None, Some(fields)) => (fields.parent, fields.depends_on.clone()),
        (None, None) => (0, Vec::new()),
    };
    let unplaceable = card.map_or_else(Vec::new, |card| below(cards, card.id));
    let mut parents = vec![Pick::new(NO_CARD, "No card")];
    parents.extend(
        cards
            .iter()
            .filter(|other| !unplaceable.contains(&other.id))
            .map(|other| Pick::new(other.id.to_string(), other.title.clone())),
    );
    let shown_parent = cards
        .iter()
        .find(|other| other.id == parent)
        .map_or_else(|| "No card".to_owned(), |other| other.title.clone());
    let card_id = card.map(|card| card.id);
    let pane_entity = cx.entity();
    let under = select(
        "board-under",
        shown_parent,
        parents,
        Some(parent.to_string().into()),
        pane.busy,
        move |key, _, cx| {
            let parent: u64 = key.parse().unwrap_or(0);
            pane_entity.update(cx, |pane, cx| match card_id {
                Some(id) => pane.ask::<call::BoardUpdate>(
                    BoardUpdateParams {
                        parent: Some(parent),
                        ..pane.card_change(id)
                    },
                    |_, _| {},
                    cx,
                ),
                None => {
                    if let Some(fields) = pane.fields.as_mut() {
                        fields.parent = parent;
                    }
                    cx.notify();
                }
            });
        },
        cx,
    );
    let set_waits = {
        let pane_entity = cx.entity();
        move |next: Vec<u64>, cx: &mut gpui_kit::App| {
            pane_entity.update(cx, |pane, cx| match card_id {
                Some(id) => pane.ask::<call::BoardUpdate>(
                    BoardUpdateParams {
                        depends_on: Some(next),
                        ..pane.card_change(id)
                    },
                    |_, _| {},
                    cx,
                ),
                None => {
                    if let Some(fields) = pane.fields.as_mut() {
                        fields.depends_on = next;
                    }
                    cx.notify();
                }
            });
        }
    };
    let set_waits = std::rc::Rc::new(set_waits);
    let name_of = |id: u64| {
        cards
            .iter()
            .find(|other| other.id == id)
            .map_or_else(|| format!("card {id}"), |other| other.title.clone())
    };
    let waits_on: Vec<Pick> = cards
        .iter()
        .filter(|other| Some(other.id) != card_id && !depends_on.contains(&other.id))
        .map(|other| Pick::new(other.id.to_string(), other.title.clone()))
        .collect();
    let chips = depends_on.iter().map(|&id| {
        let (set_waits, rest) = (
            set_waits.clone(),
            depends_on
                .iter()
                .copied()
                .filter(|other| *other != id)
                .collect::<Vec<_>>(),
        );
        div()
            .flex()
            .items_center()
            .gap(SPACE_0)
            .child(ui::meta(short_title(&name_of(id)), cx))
            .child(
                IconButton::new(
                    SharedString::from(format!("unwait-{id}")),
                    Icon::Close,
                    "No longer wait on it",
                )
                .small()
                .on_click(move |_, _, cx| set_waits(rest.clone(), cx)),
            )
            .into_any_element()
    });
    let mut after = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(SPACE_1H)
        .children(chips);
    if !waits_on.is_empty() {
        let adding = depends_on.clone();
        after = after.child(select(
            "board-after",
            if depends_on.is_empty() {
                "Nothing"
            } else {
                "Add…"
            },
            waits_on,
            None,
            pane.busy,
            move |key, _, cx| {
                let mut next = adding.clone();
                next.extend(key.parse::<u64>().ok());
                set_waits(next, cx);
            },
            cx,
        ));
    }
    vec![
        fact("Under", under, cx),
        fact("After", after.into_any_element(), cx),
    ]
}

fn text_fields(pane: &BoardPane, cx: &mut Context<BoardPane>) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let Some(fields) = &pane.fields else {
        return Vec::new();
    };
    let boxed = |element: AnyElement| {
        div()
            .px(SPACE_2)
            .py(SPACE_1H)
            .rounded(theme::RADIUS_2)
            .border_1()
            .border_color(t.border)
            .child(element)
            .into_any_element()
    };
    vec![
        div()
            .px(SPACE_3H)
            .pt(SPACE_2H)
            .child(
                Input::new(&fields.title)
                    .appearance(false)
                    .text_size(crate::theme::ui_text(15.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .into_any_element(),
        div()
            .px(SPACE_3H)
            .child(boxed(
                Textarea::new(&fields.details)
                    .appearance(false)
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .into_any_element(),
            ))
            .into_any_element(),
        section("Acceptance criteria", cx)
            .child(boxed(
                Textarea::new(&fields.acceptance)
                    .appearance(false)
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .into_any_element(),
            ))
            .into_any_element(),
    ]
}

fn contract_words(contract: &CardContract) -> String {
    match (contract.state, &contract.why) {
        (ContractState::Agreed, _) => "Agreed by a reviewer".to_owned(),
        (ContractState::Stands, Some(why)) => format!("Stands without agreement: {why}"),
        (ContractState::Stands, None) => "Stands without agreement".to_owned(),
        (ContractState::Reviewing, _) => "With a reviewer".to_owned(),
        _ => "Being proposed".to_owned(),
    }
}

/// What the card's builder agreed to build and how to verify it, which the
/// person reads here and never edits.
fn contract_section(contract: &CardContract, cx: &mut Context<BoardPane>) -> gpui_kit::Div {
    let terms = contract.terms.iter().enumerate().map(|(index, term)| {
        div()
            .flex()
            .flex_col()
            .child(
                ui::text(
                    format!("{}. {}", index + 1, term.behaviour),
                    TEXT_XS_H,
                    FontWeight::NORMAL,
                )
                .whitespace_normal(),
            )
            .child(ui::meta(format!("Verify: {}", term.verify), cx).whitespace_normal())
            .into_any_element()
    });
    let terms: Vec<AnyElement> = terms.collect();
    section("Contract", cx)
        .child(ui::meta(contract_words(contract), cx).whitespace_normal())
        .children(terms)
}

fn card_panel(pane: &BoardPane, card: BoardCard, cx: &mut Context<BoardPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    let title = match &card.ticket {
        Some(ticket) => crate::board::ticket_line(ticket, cx).into_any_element(),
        None => ui::meta(
            format!(
                "Added by {}",
                if card.created_by.is_some() {
                    card.created_by_name.as_str()
                } else {
                    "you"
                }
            ),
            cx,
        )
        .into_any_element(),
    };
    let statuses = STATUSES
        .into_iter()
        .map(|status| {
            let (key, label) = status_named(status);
            Pick::new(key, label)
        })
        .collect();
    let pane_entity = cx.entity();
    let (status_key, status_shown) = status_named(card.status);
    let status = select(
        "board-status",
        status_shown,
        statuses,
        Some(status_key.into()),
        pane.busy,
        move |status, _, cx| {
            let status = match status.as_ref() {
                "open" => CardStatus::Open,
                "progress" => CardStatus::Progress,
                _ => CardStatus::Done,
            };
            pane_entity.update(cx, |pane, cx| {
                pane.ask::<call::BoardUpdate>(
                    BoardUpdateParams {
                        status: Some(status),
                        ..pane.card_change(id)
                    },
                    |_, _| {},
                    cx,
                )
            });
        },
        cx,
    );
    let owner_view = if card.owner.is_some() {
        owner(&card, cx)
    } else {
        ui::meta("Nobody yet", cx).into_any_element()
    };
    let under: Vec<BoardCard> = pane
        .cards()
        .iter()
        .filter(|other| other.parent == Some(id))
        .cloned()
        .collect();
    let fields = text_fields(pane, cx);
    let mut view = div()
        .flex()
        .flex_col()
        .child(head(title, cx))
        .children(fields.into_iter().take(1))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .px(SPACE_3H)
                .py(SPACE_2H)
                .child(fact("Status", status, cx))
                .child(fact("Owner", owner_view, cx))
                .children(placing(pane, Some(&card), cx)),
        );
    let mut rest = text_fields(pane, cx).into_iter().skip(1);
    view = view.children(rest.next()).children(rest.next());
    if let Some(contract) = card.contract.as_ref().filter(|c| !c.terms.is_empty()) {
        view = view.child(contract_section(contract, cx));
    }
    if let Some(result) = &card.result {
        view = view
            .child(section("Result", cx).child(
                ui::text(result.clone(), TEXT_XS_H, FontWeight::NORMAL).whitespace_normal(),
            ));
    }
    if let Some(run) = card.auto.clone() {
        let mut auto = section("Auto", cx).child(
            ui::text(run_label(&run, card.status), TEXT_XS_H, FontWeight::MEDIUM)
                .text_color(t.text_primary),
        );
        if let Some(note) = run.note.clone() {
            auto = auto.child(ui::meta(note, cx).whitespace_normal());
        }
        if !run.branch.is_empty() {
            auto = auto.child(ui::meta(format!("Branch {}", run.branch), cx));
        }
        if let Some(pull) = run.pull.clone() {
            let url = pull.url.clone();
            auto = auto.child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1)
                    .child(ui::meta(format!("Pull request #{}", pull.number), cx))
                    .child(
                        IconButton::new(
                            "board-pull-open",
                            Icon::Link,
                            "Open the pull request in the browser",
                        )
                        .small()
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            );
        }
        if run.waits_on_person {
            let again = if run.worktree.is_empty() {
                "Start it again"
            } else {
                "Ship it again"
            };
            auto = auto.child(
                Button::new("board-retry", again, Variant::Outline).on_click(cx.listener(
                    move |pane, _, _, cx| {
                        let card = BoardCardParams {
                            project: pane.folder.clone(),
                            id,
                        };
                        pane.ask::<call::BoardRetry>(card, |_, _| {}, cx)
                    },
                )),
            );
        }
        view = view.child(auto);
    }
    if let Some(note) = card
        .ticket
        .as_ref()
        .and_then(|ticket| ticket.sync_note.clone())
    {
        view = view.child(
            div()
                .px(SPACE_3H)
                .child(ui::meta(note, cx).whitespace_normal()),
        );
    }
    if !under.is_empty() {
        view = view.child(
            section("Cards under it", cx).children(under.into_iter().map(|each| {
                let other = each.id;
                div()
                    .id(SharedString::from(format!("under-{other}")))
                    .flex()
                    .justify_between()
                    .gap(SPACE_2)
                    .py(px(3.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.select(Some(Open::Card(other)), window, cx)
                    }))
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::theme::ui_text(TEXT_XS_H))
                            .child(each.title.clone()),
                    )
                    .child(ui::meta(status_label(each.status), cx))
            })),
        );
    }
    let leaving = card.title.clone();
    let stopping = (card.owner.is_some() && card.status == CardStatus::Progress).then(|| {
        format!(
            "{} is told to stop working on it.",
            card.owner_name
                .clone()
                .unwrap_or_else(|| "The agent working it".into())
        )
    });
    view.child(div().px(SPACE_3H).py(SPACE_3).child(
        Button::new("board-remove", "Remove from board", Variant::Outline).on_click(cx.listener(
            move |pane, _, _, cx| {
                let pane_entity = cx.entity();
                let (title, body) = (
                    format!("Take “{leaving}” off the board?"),
                    stopping.clone().unwrap_or_default(),
                );
                let _ = pane.bench.update(cx, |bench, cx| {
                    bench.confirm(
                        title,
                        body,
                        "Remove",
                        true,
                        move |_, yes, _, cx| {
                            if yes {
                                pane_entity.update(cx, |pane, cx| {
                                    pane.ask::<call::BoardRemove>(
                                        BoardCardParams {
                                            project: pane.folder.clone(),
                                            id,
                                        },
                                        |pane, cx| {
                                            pane.open = None;
                                            pane.fields = None;
                                            cx.notify();
                                        },
                                        cx,
                                    )
                                });
                            }
                        },
                        cx,
                    )
                });
            },
        )),
    ))
    .into_any_element()
}

fn new_panel(pane: &BoardPane, cx: &mut Context<BoardPane>) -> AnyElement {
    let fields = text_fields(pane, cx);
    let mut fields = fields.into_iter();
    div()
        .flex()
        .flex_col()
        .child(head(ui::meta("New card", cx).into_any_element(), cx))
        .children(fields.next())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .px(SPACE_3H)
                .py(SPACE_2H)
                .children(placing(pane, None, cx)),
        )
        .children(fields)
        .child(
            div().px(SPACE_3H).py(SPACE_3).child(
                Button::new("board-add-card", "Add card", Variant::Primary)
                    .on_click(cx.listener(|pane, _, _, cx| pane.add(cx))),
            ),
        )
        .into_any_element()
}

/// Which of Auto's agents a picker chooses: the one that works the cards, or its reviewer.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    Agent,
    Reviewer,
}

/// Auto's agent or reviewer as the composer's own picker: the agent, its account, who pays
/// and the model, in one popover.
fn auto_picker(
    pane: &BoardPane,
    role: Role,
    launcher: Option<&str>,
    payer: Option<&str>,
    model: Option<&str>,
    cx: &mut Context<BoardPane>,
) -> AnyElement {
    let Some(bench) = pane.bench.upgrade() else {
        return div().into_any_element();
    };
    let launchers = bench.read(cx).launchers().to_vec();
    let Some(shown) = launcher
        .and_then(|id| launchers.iter().find(|known| known.id == id))
        .or(launchers.first())
        .cloned()
    else {
        return ui::meta("No agents", cx).into_any_element();
    };
    let profile_of = |id: &str| id.split_once(':').map(|(_, profile)| profile.to_owned());
    let mut models = shown.model.clone();
    if let (Some(models), Some(model)) = (models.as_mut(), model) {
        models.current = model.to_owned();
    }
    let pane_entity = cx.entity();
    let save: Save = Rc::new(move |chosen, cx| {
        pane_entity.update(cx, |pane, cx| {
            pane.change_settings(
                |params| match role {
                    Role::Agent => {
                        params.launcher = Some(chosen.launcher);
                        params.provider = Some(chosen.provider);
                        params.model = Some(chosen.model);
                    }
                    Role::Reviewer => params.reviewer = Some(Some(chosen)),
                },
                cx,
            )
        });
    });
    let set: model_picker::Set = {
        let (save, launcher, payer) = (save.clone(), shown.id.clone(), payer.map(str::to_owned));
        Rc::new(move |id, value, _, cx| {
            if id == "model" {
                let chosen = BoardReviewerParams {
                    launcher: Some(launcher.clone()),
                    provider: payer.clone(),
                    model: Some(value.to_owned()),
                };
                save(chosen, cx);
            }
        })
    };
    let moving: model_picker::Move = {
        let save = save.clone();
        Rc::new(move |to: model_picker::Moving, _, cx| {
            let launcher = launchers
                .iter()
                .find(|known| {
                    known.provider == to.provider
                        && to
                            .profile
                            .as_deref()
                            .is_none_or(|profile| profile_of(&known.id).as_deref() == Some(profile))
                })
                .map(|known| known.id.clone());
            let chosen = BoardReviewerParams {
                launcher,
                provider: to.model_provider,
                model: to.model,
            };
            save(chosen, cx);
        })
    };
    let model_label = models.as_ref().and_then(|models| {
        models
            .choices
            .iter()
            .find(|choice| choice.value == models.current)
            .map(|choice| choice.label.clone())
    });
    let label = match (launcher, model_label) {
        (None, _) if role == Role::Agent => "The first that works".to_owned(),
        (None, _) => "Any other agent".to_owned(),
        (Some(_), Some(model)) => format!("{} · {model}", shown.label),
        (Some(_), None) => shown.label.clone(),
    };
    let picked = model_picker::Picked {
        provider: shown.provider.clone(),
        profile: profile_of(&shown.id),
        model: models,
        model_provider: payer.map(str::to_owned),
        locked: false,
        pending: None,
        set,
        moving,
        target: bench.read(cx).target(),
    };
    let key = match role {
        Role::Agent => "board-auto-agent",
        Role::Reviewer => "board-auto-reviewer",
    };
    let trigger = Button::new(key, label, Variant::Outline).lead(ui::mark(&shown.provider, 14.));
    model_picker::around(trigger, key, picked, &bench).into_any_element()
}

fn auto_panel(pane: &BoardPane, cx: &mut Context<BoardPane>) -> AnyElement {
    let Some(board) = &pane.board else {
        return div().into_any_element();
    };
    let settings = board.settings.clone();
    let slots = SLOT_CHOICES
        .into_iter()
        .map(|slots| {
            Pick::new(
                slots.to_string(),
                if slots == 1 {
                    "1 agent".to_owned()
                } else {
                    format!("{slots} agents")
                },
            )
        })
        .collect();
    let landings = landing_picks();
    let (landing_key, landing_shown) = landing_choice(settings.landing);
    let entity = || cx.entity();
    let (on_slots, on_landing) = (entity(), entity());
    let reviewer = settings.reviewer.clone();
    let auto_on = settings.auto;
    let agent_cards = settings.agent_cards;
    let (reviewer_runs, contract) = (settings.reviewer_runs, settings.contract);
    let mut view = div()
        .flex()
        .flex_col()
        .child(head(ui::text("Auto", TEXT_SM, FontWeight::SEMIBOLD).into_any_element(), cx))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .px(SPACE_3H)
                .py(SPACE_2H)
                .child(
                    Toggle::new("board-auto-on", "Start agents on cards by itself", auto_on).on_click(
                        cx.listener(move |pane, _, _, cx| {
                            pane.change_settings(|params| params.auto = Some(!auto_on), cx)
                        }),
                    ),
                )
                .child(
                    Toggle::new("board-agent-cards", "Also take cards agents add", agent_cards).on_click(
                        cx.listener(move |pane, _, _, cx| {
                            pane.change_settings(|params| params.agent_cards = Some(!agent_cards), cx)
                        }),
                    ),
                )
                .child(
                    ui::meta(
                        "Starts agents on your open cards, each on its own branch; another agent reviews the work before it lands.",
                        cx,
                    )
                    .whitespace_normal(),
                ),
        );
    if let Some(note) = board.auto_note.clone() {
        view = view.child(
            div()
                .px(SPACE_3H)
                .child(ui::meta(note, cx).whitespace_normal()),
        );
    }
    let busy = pane.busy;
    view.child(
        settings_group("Agents", cx)
            .child(fact(
                "At once",
                select(
                    "board-slots",
                    if settings.slots == 1 {
                        "1 agent".to_owned()
                    } else {
                        format!("{} agents", settings.slots)
                    },
                    slots,
                    Some(settings.slots.to_string().into()),
                    busy,
                    move |slots, _, cx| {
                        let slots: u32 = slots.parse().unwrap_or(2);
                        on_slots.update(cx, |pane, cx| {
                            pane.change_settings(|params| params.slots = Some(slots), cx)
                        })
                    },
                    cx,
                ),
                cx,
            ))
            .child(fact(
                "Lands",
                select(
                    "board-landing",
                    landing_shown,
                    landings,
                    Some(landing_key.into()),
                    busy,
                    move |landing, _, cx| {
                        let landing = landing_of(landing.as_ref());
                        on_landing.update(cx, |pane, cx| {
                            pane.change_settings(|params| params.landing = Some(landing), cx)
                        })
                    },
                    cx,
                ),
                cx,
            ))
            .child({
                let picker = auto_picker(
                    pane,
                    Role::Agent,
                    settings.launcher.as_deref(),
                    settings.provider.as_deref(),
                    settings.model.as_deref(),
                    cx,
                );
                fact("Agent", picker, cx)
            }),
    )
    .child(
        settings_group("Reviewer", cx)
            .child({
                let picker = auto_picker(
                    pane,
                    Role::Reviewer,
                    reviewer.as_ref().map(|reviewer| reviewer.launcher.as_str()),
                    reviewer
                        .as_ref()
                        .and_then(|reviewer| reviewer.provider.as_deref()),
                    reviewer
                        .as_ref()
                        .and_then(|reviewer| reviewer.model.as_deref()),
                    cx,
                );
                fact("Reviewer", picker, cx)
            })
            .child(
                ui::meta(
                    "The reviewer checks each change against its card before it merges.",
                    cx,
                )
                .whitespace_normal(),
            )
            .child(
                Toggle::new(
                    "board-reviewer-runs",
                    "Reviewer builds and uses the change",
                    reviewer_runs,
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.change_settings(|params| params.reviewer_runs = Some(!reviewer_runs), cx)
                })),
            )
            .child(
                Toggle::new(
                    "board-contract",
                    "Agree what to build before building",
                    contract,
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.change_settings(|params| params.contract = Some(!contract), cx)
                })),
            ),
    )
    .into_any_element()
}

fn sender(id: Option<&str>, name: Option<&String>) -> String {
    match id {
        Some("person") => "You".into(),
        Some("board") => "Agent board".into(),
        Some("auto") => "Auto".into(),
        Some("sikemux") => "Sikemux".into(),
        Some(_) => name.cloned().unwrap_or_else(|| "A closed agent".into()),
        None => "An agent".into(),
    }
}

fn kind_label(kind: Option<MessageKind>) -> &'static str {
    match kind {
        Some(MessageKind::Report) => "Report",
        Some(MessageKind::Question) => "Question",
        Some(MessageKind::Answer) => "Answer",
        Some(MessageKind::Notice) => "Note",
        Some(MessageKind::Message) | None => "Message",
    }
}

fn outcome_label(row: &MessageRow) -> &'static str {
    match row.outcome {
        MessageOutcome::Delivered if row.waited => "Delivered after waiting",
        MessageOutcome::Steered => "Read mid-turn",
        MessageOutcome::NewTurn => "Started a turn",
        MessageOutcome::Typed => "Typed in",
        MessageOutcome::Waiting => "Waiting",
        MessageOutcome::Delivered => "Delivered",
        MessageOutcome::Returned => "Not taken, waiting",
        MessageOutcome::Repeat => "Repeat, not sent",
        MessageOutcome::Refused => "Refused",
        MessageOutcome::Dropped => "Dropped unread",
    }
}

fn messages_panel(pane: &BoardPane, cx: &mut Context<BoardPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let rows = pane.messages.clone();
    let count = |tone: MessageTone| {
        rows.as_ref().map_or(0, |rows| {
            rows.iter().filter(|row| row.tone == Some(tone)).count()
        })
    };
    let (held, failed) = (count(MessageTone::Held), count(MessageTone::Failed));
    let summary = [
        (held > 0).then(|| format!("{held} waiting")),
        (failed > 0).then(|| format!("{failed} not delivered")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let title = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::text("Messages", TEXT_SM, FontWeight::SEMIBOLD))
        .child(ui::meta(summary, cx))
        .into_any_element();
    let mut view = div().flex().flex_col().child(head(title, cx)).child(
        div().px(SPACE_3H).py(SPACE_2).child(
            ui::meta(
                "Newest first. Sikemux keeps who sent what kind and how it went, never the text.",
                cx,
            )
            .whitespace_normal(),
        ),
    );
    match rows {
        None => view = view.child(div().px(SPACE_3H).child(ui::meta("reading…", cx))),
        Some(rows) if rows.is_empty() => {
            view = view.child(
                div()
                    .px(SPACE_3H)
                    .child(ui::meta("No agent has messaged another here yet.", cx)),
            )
        }
        Some(rows) => {
            view = view.children(rows.into_iter().map(|row| {
                let tint = match row.tone {
                    Some(MessageTone::Held) => t.warn,
                    Some(MessageTone::Failed) => t.danger,
                    None => t.text_tertiary,
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .px(SPACE_3H)
                    .py(SPACE_1H)
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(crate::theme::ui_text(TEXT_XS))
                            .child(ui::text(
                                sender(row.from.as_deref(), row.from_name.as_ref()),
                                TEXT_XS,
                                FontWeight::SEMIBOLD,
                            ))
                            .child(ui::meta("to", cx))
                            .child(ui::text(
                                sender(Some(&row.to), row.to_name.as_ref()),
                                TEXT_XS,
                                FontWeight::SEMIBOLD,
                            ))
                            .child(div().flex_1())
                            .child(ui::meta(crate::ui::time::age(row.sent_at), cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(SPACE_1)
                            .child(ui::meta(format!("{} ·", kind_label(row.kind)), cx))
                            .child(ui::meta(outcome_label(&row), cx).text_color(tint)),
                    )
            }));
        }
    }
    view.into_any_element()
}
