//! The mode agents start in, and how chats behave while they run and after.

use crate::core::Target;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, App, Context, Entity, InteractiveElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use sikemux_core::server::calls::NoParams;
use sikemux_core::server::names::{NameTheme, NameThemeParams};
use sikemux_view::prefs::PreferenceChange;
use sikemux_view::prefs::{MidTurnSend, RailDensity};
use sikemux_wire::protocol::PermissionMode;

use crate::settings::rows::{self, Choice};
use crate::theme::{self, Tokens, SPACE_1, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H, TEXT_SM};
use crate::ui;
use crate::workbench::Workbench;
use sikemux_core::server::call;
use sikemux_core::server::workspace::DefaultsParams;

/// What a search finds on this page.
pub(in crate::settings) const NOTIFICATIONS: &[crate::settings::index::Entry] = &[
    crate::settings::index::section(
        crate::settings::index::Page::Notifications,
        "Notifications",
        "notification alert badge dock permission finished",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notifications,
        "Notifications",
        "Notify when an agent needs you",
        "notification alert badge dock permission finished",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notifications,
        "Notifications",
        "Notify when a goal moves",
        "notification goal slice landed finished eta slipped progress",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notifications,
        "Notifications",
        "Test notification",
        "notification sound permission send try",
    ),
];

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[crate::settings::index::Entry] = &[
    crate::settings::index::section(
        crate::settings::index::Page::Permissions,
        "Launch boundary",
        "permissions yolo sandbox safety bypass approval",
    ),
    crate::settings::index::section(crate::settings::index::Page::Chats, "Chats", "sessions"),
    crate::settings::index::row(
        crate::settings::index::Page::Chats,
        "Chats",
        "Sending while an agent works",
        "steer queue interrupt enter",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Chats,
        "Chats",
        "Rail density",
        "compact comfortable sidebar",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Chats,
        "Chats",
        "Agent names",
        "name theme anime nature naruto one piece characters words",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Chats,
        "Chats",
        "Idle chats",
        "sleep memory process agents",
    ),
];

pub struct Fields {
    mid_turn: Choice,
    density: Choice,
    /// What new agents are called, once the core has said.
    names: Option<Choice>,
}

/// The themes new agents' names come from: the core's word, and how the menu shows it.
const NAMES: [(NameTheme, &str); 2] = [
    (NameTheme::Nature, "Nature: Otter, Maple, Flint"),
    (NameTheme::Anime, "Anime: Luffy, Gojo, Frieren"),
];

/// Asks the core what new agents are called, and offers the themes once it says.
pub fn read_names(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn_in(window, async move |bench, cx| {
        let asked =
            crate::core::Target::here(&asks).request_typed::<call::AgentNameTheme>(NoParams {});
        let Ok(answer) = asked.await else {
            return;
        };
        let chosen = NAMES
            .iter()
            .position(|(theme, _)| answer.theme == *theme)
            .unwrap_or(0);
        let _ = bench.update_in(cx, |bench, window, cx| {
            let names = Choice::new(
                NAMES.iter().map(|(_, label)| *label).collect(),
                chosen,
                |bench, at, _| {
                    Target::here(&bench.asks).call_typed::<call::SetAgentNameTheme>(
                        NameThemeParams { theme: NAMES[at].0 },
                    );
                },
                window,
                cx,
            );
            super::change_fields(bench, cx, |fields, _| fields.sessions.names = Some(names));
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
            |bench, at, _| {
                crate::settings::set(
                    bench,
                    PreferenceChange::MidTurnSend([MidTurnSend::Steer, MidTurnSend::Queue][at]),
                )
            },
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
                crate::settings::set(
                    bench,
                    PreferenceChange::RailDensity(
                        [RailDensity::Comfortable, RailDensity::Compact][at],
                    ),
                )
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
const MODES: [(PermissionMode, &str, &str); 2] = [
    (
        PermissionMode::WorkspaceWrite,
        "Normal",
        "Use the agent's normal approval and sandbox behavior.",
    ),
    (
        PermissionMode::Bypass,
        "YOLO",
        "Bypass approvals and sandboxing.",
    ),
];

pub fn boundary(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let current = bench
        .this_view()
        .map_or(PermissionMode::WorkspaceWrite, |view| {
            view.workspace.permission_mode
        });
    let cards = div()
        .flex()
        .gap(SPACE_2H)
        .p(SPACE_4)
        .children(MODES.iter().map(|(mode, name, detail)| {
            let chosen = *mode == current;
            let danger = *mode == PermissionMode::Bypass;
            let this = this.clone();
            div()
                .id(SharedString::from(format!("launch-mode-{}", mode.as_str())))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .p(SPACE_3)
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
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(if chosen && danger {
                            t.danger
                        } else {
                            t.text_primary
                        })
                        .child(*name),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
                        .text_color(t.text_tertiary)
                        .child(*detail),
                )
                .on_click(move |_, _, cx| {
                    this.update(cx, |bench, _| {
                        Target::here(&bench.asks).call_typed::<call::SetAgentDefaults>(
                            DefaultsParams {
                                permission_mode: *mode,
                            },
                        );
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
    fields: &Fields,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
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
                    Target::here(&bench.asks).call_typed::<call::SleepIdleChats>(NoParams {});
                })
            },
        )
    };
    let body = div()
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
    rows::section("Chats", None, None, lit("Chats"), body, cx)
}

/// Whether Sikemux says when an agent needs the person, and a test of it.
pub fn notifications(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let preferences = bench.preferences();
    let body = div()
        .child(rows::row(
            "Notify when an agent needs you",
            Some("While Sikemux is in the background, a notification says when an agent asks for input or finishes."),
            rows::switch(
                "agent-notifications",
                PreferenceChange::AgentNotifications,
                preferences.agent_notifications,
                this,
            ),
            cx,
        ))
        .child(rows::row(
            "Notify when a goal moves",
            Some("A short notification when slices land, a goal finishes or its ETA slips, and never for what you're looking at. What needs you always notifies."),
            rows::switch(
                "goal-notifications",
                PreferenceChange::GoalNotifications,
                preferences.goal_notifications,
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
        ));
    rows::section("Notifications", None, None, lit("Notifications"), body, cx)
}
