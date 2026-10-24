//! The mode agents start in, and how chats behave while they run and after.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, Entity, InteractiveElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use serde_json::json;
use sikemux_wire::protocol::{MidTurnSend, RailDensity};

use crate::core::Ask;
use crate::settings::rows::{self, Choice};
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

pub struct Fields {
    mid_turn: Choice,
    density: Choice,
    /// What new agents are called, once the core has said.
    names: Option<Choice>,
}

/// The themes new agents' names come from: the core's word, and how the menu shows it.
const NAMES: [(&str, &str); 2] = [
    ("nature", "Nature: Otter, Maple, Flint"),
    ("anime", "Anime: Luffy, Gojo, Frieren"),
];

/// Asks the core what new agents are called, and offers the themes once it says.
pub fn read_names(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn_in(window, async move |bench, cx| {
        let Ok(answer) = crate::core::request(&asks, "agentNameTheme", json!({})).await else {
            return;
        };
        let chosen = NAMES
            .iter()
            .position(|(theme, _)| answer["theme"] == *theme)
            .unwrap_or(0);
        let _ = bench.update_in(cx, |bench, window, cx| {
            let names = Choice::new(
                NAMES.iter().map(|(_, label)| *label).collect(),
                chosen,
                |bench, at, _| {
                    let _ = bench.asks.send(Ask::Call {
                        method: "setAgentNameTheme",
                        params: json!({ "theme": NAMES[at].0 }),
                    });
                },
                window,
                cx,
            );
            if let Some(fields) = super::fields_of(bench) {
                fields.sessions.names = Some(names);
            }
            bench.parts.redraw(&bench.parts.stage, cx);
        });
    })
    .detach();
}

impl Fields {
    pub fn new(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let preferences = bench.preferences();
        let mid_turn = Choice::new(
            vec![
                "Steer: the agent reads it in the running turn",
                "Queue: it waits until the turn ends",
            ],
            match preferences.mid_turn_send {
                MidTurnSend::Steer => 0,
                MidTurnSend::Queue => 1,
            },
            |bench, at, _| crate::settings::set(bench, "midTurnSend", ["steer", "queue"][at]),
            window,
            cx,
        );
        let density = Choice::new(
            vec!["Comfortable", "Compact"],
            match preferences.rail_density {
                RailDensity::Comfortable => 0,
                RailDensity::Compact => 1,
            },
            |bench, at, _| {
                crate::settings::set(bench, "railDensity", ["comfortable", "compact"][at])
            },
            window,
            cx,
        );
        Self {
            mid_turn,
            density,
            names: None,
        }
    }
}

/// Each mode agents can start in: its word in the core, its name, and what it does.
const MODES: [(&str, &str, &str); 2] = [
    (
        "workspace-write",
        "Normal",
        "Use the provider's normal approval and sandbox behavior.",
    ),
    ("bypass", "YOLO", "Bypass approvals and sandboxing."),
];

pub fn boundary(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let current = bench
        .view
        .as_ref()
        .map(|view| view.workspace.permission_mode.as_str())
        .unwrap_or("workspace-write");
    let current = if current == "bypass" {
        "bypass"
    } else {
        "workspace-write"
    };
    let cards = div()
        .flex()
        .gap(px(10.))
        .p(px(14.))
        .children(MODES.iter().map(|(mode, name, detail)| {
            let chosen = *mode == current;
            let danger = *mode == "bypass";
            let this = this.clone();
            div()
                .id(SharedString::from(format!("launch-mode-{mode}")))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(12.))
                .rounded(theme::RADIUS_3)
                .border_1()
                .cursor_pointer()
                .border_color(match (chosen, danger) {
                    (true, true) => t.danger,
                    (true, false) => t.border_selected,
                    _ => t.border,
                })
                .when(chosen, |card| card.bg(t.surface_chosen))
                .when(!chosen, |card| {
                    card.hover(|style| style.bg(t.surface_hover))
                })
                .child(
                    div()
                        .text_size(crate::theme::ui_text(13.))
                        .text_color(if chosen && danger {
                            t.danger
                        } else {
                            t.text_primary
                        })
                        .child(*name),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(11.5))
                        .text_color(t.text_tertiary)
                        .child(*detail),
                )
                .on_click(move |_, _, cx| {
                    this.update(cx, |bench, _| {
                        let _ = bench.asks.send(Ask::Call {
                            method: "setAgentDefaults",
                            params: json!({ "permissionMode": mode }),
                        });
                    })
                })
        }));
    let name = MODES
        .iter()
        .find(|(mode, _, _)| *mode == current)
        .map_or("Normal", |(_, name, _)| *name);
    rows::section(
        "Launch boundary",
        Some(name.into()),
        Some("Offered before every launch. Providers without a matching CLI control fall back to their own settings.".into()),
        lit("Launch boundary"),
        cards,
        cx,
    )
}

pub fn render(
    bench: &Workbench,
    fields: &Fields,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let preferences = bench.preferences();
    let keys = if cfg!(target_os = "macos") {
        "⌘↵"
    } else {
        "Ctrl+↵"
    };
    let sleep = {
        let this = this.clone();
        ui::Button::new("sleep-idle", "Sleep now", ui::Variant::Outline).on_click(
            move |_, _, cx| {
                this.update(cx, |bench, _| {
                    let _ = bench.asks.send(Ask::Call {
                        method: "sleepIdleChats",
                        params: json!({}),
                    });
                })
            },
        )
    };
    let body = div()
        .child(rows::row(
            "Restore agent tabs",
            Some("Resumable tabs come back asleep and start only when selected."),
            rows::switch("restore-agent-tabs", "restoreAgentTabs", preferences.restore_agent_tabs, this),
            cx,
        ))
        .child(rows::row(
            "Notify when an agent needs you",
            Some("While Sikemux is in the background, a notification says when an agent asks for input or finishes."),
            rows::switch(
                "agent-notifications",
                "agentNotifications",
                preferences.agent_notifications,
                this,
            ),
            cx,
        ))
        .child(rows::row(
            "Test notification",
            Some("Sends one now, with its sound. The first one also has macOS ask whether Sikemux may notify you."),
            {
                let this = this.clone();
                ui::Button::new("notification-test", "Send test", ui::Variant::Outline)
                    .on_click(move |_, _, cx| this.update(cx, |_, cx| crate::notifications::test(cx)))
            },
            cx,
        ))
        .child(rows::row_with(
            None,
            "Sending while an agent works",
            Some(format!("What ↵ does mid-turn. {keys} does the other.").into()),
            fields.mid_turn.view(),
            cx,
        ))
        .child(rows::row(
            "Rail density",
            Some("Compact fits more sessions while keeping every state symbol visible."),
            fields.density.view(),
            cx,
        ))
        .children(fields.names.as_ref().map(|names| {
            rows::row(
                "Agent names",
                Some("What new agents are called. Open agents keep their names."),
                names.view(),
                cx,
            )
        }))
        .child(rows::row(
            "Idle chats",
            Some("Put every idle chat to sleep now, freeing its process."),
            sleep,
            cx,
        ))
        .child(rows::note(
            "Only confirmed native session IDs are written to disk. Startup commands and terminal output never are.",
            cx,
        ));
    rows::section("Sessions", None, None, lit("Sessions"), body, cx)
}
