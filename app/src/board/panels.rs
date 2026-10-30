//! What opens beside a board: one card to read and change or a new one to add,
//! Auto's settings, and the messages between the project's agents.

use std::rc::Rc;

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use serde_json::{json, Value};

use crate::board::{owner, short_title, waits_on_person, BoardPane, Card, MessageRow, Open};
use crate::chat::model_picker;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, TEXT_2XS_H,
    TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, select, Button, IconButton, Pick, Toggle, Variant};

/// Keeps a choice made in a picker as the board's settings.
type Save = Rc<dyn Fn(Value, &mut gpui_kit::App)>;

const MAX_SLOTS: u32 = 8;
const NO_CARD: &str = "0";

fn status_label(status: &str) -> &'static str {
    match status {
        "progress" => "In progress",
        "done" => "Done",
        _ => "Open",
    }
}

fn stage_label(stage: &str) -> &'static str {
    match stage {
        "starting" => "Starting",
        "working" => "Working",
        "shipping" => "Shipping",
        "checking" => "Checks running",
        "reviewing" => "Under review",
        "fixing" => "Fixing",
        "merging" => "Merging",
        "review" => "Waits for you",
        "held" => "Back with you",
        "merged" => "Merged",
        _ => "Closed",
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
fn below(cards: &[Card], id: u64) -> Vec<u64> {
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
fn placing(pane: &BoardPane, card: Option<&Card>, cx: &mut Context<BoardPane>) -> Vec<AnyElement> {
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
                Some(id) => pane.ask(
                    "boardUpdate",
                    json!({ "id": id, "parent": parent }),
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
                Some(id) => pane.ask(
                    "boardUpdate",
                    json!({ "id": id, "dependsOn": next }),
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

fn card_panel(pane: &BoardPane, card: Card, cx: &mut Context<BoardPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = card.id;
    let title = match &card.ticket {
        Some(ticket) => {
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
                        IconButton::new(
                            "board-ticket-open",
                            Icon::Link,
                            "Open the ticket in the browser",
                        )
                        .small()
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                })
                .into_any_element()
        }
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
    let statuses = ["open", "progress", "done"]
        .into_iter()
        .map(|status| Pick::new(status, status_label(status)))
        .collect();
    let pane_entity = cx.entity();
    let status = select(
        "board-status",
        status_label(&card.status),
        statuses,
        Some(card.status.clone().into()),
        pane.busy,
        move |status, _, cx| {
            let status = status.to_string();
            pane_entity.update(cx, |pane, cx| {
                pane.ask(
                    "boardUpdate",
                    json!({ "id": id, "status": status }),
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
    let under: Vec<Card> = pane
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
    if let Some(result) = &card.result {
        view = view
            .child(section("Result", cx).child(
                ui::text(result.clone(), TEXT_XS_H, FontWeight::NORMAL).whitespace_normal(),
            ));
    }
    if let Some(run) = card.auto.clone() {
        let mut auto = section("Auto", cx).child(
            ui::text(stage_label(&run.stage), TEXT_XS_H, FontWeight::MEDIUM)
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
        if waits_on_person(&run.stage) {
            let again = if run.worktree.is_empty() {
                "Start it again"
            } else {
                "Ship it again"
            };
            auto = auto.child(
                Button::new("board-retry", again, Variant::Outline).on_click(cx.listener(
                    move |pane, _, _, cx| {
                        pane.ask("boardRetry", json!({ "id": id }), |_, _| {}, cx)
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
                    .child(ui::meta(status_label(&each.status), cx))
            })),
        );
    }
    let leaving = card.title.clone();
    let stopping = (card.owner.is_some() && card.status == "progress").then(|| {
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
                                    pane.ask(
                                        "boardRemove",
                                        json!({ "id": id }),
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
        let changes = match role {
            Role::Agent => chosen,
            Role::Reviewer => json!({ "reviewer": chosen }),
        };
        pane_entity.update(cx, |pane, cx| {
            pane.ask("boardSettings", changes, |_, _| {}, cx)
        });
    });
    let set: model_picker::Set = {
        let (save, launcher, payer) = (save.clone(), shown.id.clone(), payer.map(str::to_owned));
        Rc::new(move |id, value, _, cx| {
            if id == "model" {
                save(
                    json!({ "launcher": launcher, "provider": payer, "model": value }),
                    cx,
                );
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
            save(
                json!({ "launcher": launcher, "provider": to.model_provider, "model": to.model }),
                cx,
            );
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
    let save =
        |changes: Value, cx: &mut gpui_kit::App, pane_entity: &gpui_kit::Entity<BoardPane>| {
            pane_entity.update(cx, |pane, cx| {
                pane.ask("boardSettings", changes, |_, _| {}, cx)
            });
        };
    let slots = (1..=MAX_SLOTS)
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
    let finishes = vec![
        Pick::new("merge", "Merge it").hint("Once CI, or the check command, passes"),
        Pick::new("pr", "Stop at the pull request").hint("You review and merge it"),
    ];
    let finish_shown = if settings.finish == "pr" {
        "Stop at the pull request"
    } else {
        "Merge it"
    };
    let entity = || cx.entity();
    let (on_slots, on_finish) = (entity(), entity());
    let reviewer = settings.reviewer.clone();
    let auto_on = settings.auto;
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
                            pane.ask("boardSettings", json!({ "auto": !auto_on }), |_, _| {}, cx)
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
                        save(json!({ "slots": slots }), cx, &on_slots)
                    },
                    cx,
                ),
                cx,
            ))
            .child(fact(
                "Then",
                select(
                    "board-finish",
                    finish_shown,
                    finishes,
                    Some(settings.finish.clone().into()),
                    busy,
                    move |finish, _, cx| save(json!({ "finish": finish.as_ref() }), cx, &on_finish),
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
                    reviewer
                        .as_ref()
                        .and_then(|reviewer| reviewer.launcher.as_deref()),
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

fn kind_label(kind: Option<&str>) -> &'static str {
    match kind {
        Some("report") => "Report",
        Some("question") => "Question",
        Some("answer") => "Answer",
        Some("notice") => "Note",
        _ => "Message",
    }
}

fn outcome_label(row: &MessageRow) -> &'static str {
    match row.outcome.as_str() {
        "delivered" if row.waited => "Delivered after waiting",
        "steered" => "Read mid-turn",
        "newTurn" => "Started a turn",
        "typed" => "Typed in",
        "waiting" => "Waiting",
        "delivered" => "Delivered",
        "returned" => "Not taken, waiting",
        "repeat" => "Repeat, not sent",
        "refused" => "Refused",
        _ => "Dropped unread",
    }
}

fn messages_panel(pane: &BoardPane, cx: &mut Context<BoardPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let rows = pane.messages.clone();
    let count = |tone: &str| {
        rows.as_ref().map_or(0, |rows| {
            rows.iter()
                .filter(|row| row.tone.as_deref() == Some(tone))
                .count()
        })
    };
    let (held, failed) = (count("held"), count("failed"));
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
                let tint = match row.tone.as_deref() {
                    Some("held") => t.warn,
                    Some("failed") => t.danger,
                    _ => t.text_tertiary,
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
                            .child(ui::meta(
                                format!("{} ·", kind_label(row.kind.as_deref())),
                                cx,
                            ))
                            .child(ui::meta(outcome_label(&row), cx).text_color(tint)),
                    )
            }));
        }
    }
    view.into_any_element()
}
