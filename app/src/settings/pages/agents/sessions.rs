//! The mode agents start in, and how chats behave while they run and after.

use crate::core::Target;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, AnyElement, App, Context, Entity, InteractiveElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use sikemux_calls::names::NameTheme;
use sikemux_calls::names::NameThemeParams;
use sikemux_calls::params::NoParams;
use sikemux_view::prefs::PreferenceChange;
use sikemux_view::prefs::{MidTurnSend, RailDensity, ScreenReader};
use sikemux_wire::protocol::PermissionMode;

use crate::settings::rows::{self, Choice};
use crate::theme::{self, Tokens, SPACE_1, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H, TEXT_SM};
use crate::ui;
use crate::workbench::Workbench;
use sikemux_calls::call;
use sikemux_calls::workspace::DefaultsParams;

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
        "permissions yolo auto sandbox safety bypass approval",
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
        "Screen reader",
        "voiceover accessibility announce read aloud blind a11y",
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
    crate::settings::index::row(
        crate::settings::index::Page::Chats,
        "Chats",
        "Heavy commands at once",
        "cargo build compile test queue line memory jobs parallel limit",
    ),
];

pub struct Fields {
    mid_turn: Choice,
    density: Choice,
    screen_reader: Choice,
    builds: Choice,
    /// What new agents are called, once the core has said.
    names: Option<Choice>,
}

/// How much of the chat in front a screen reader hears, in the order the menu offers it.
const READINGS: [ScreenReader; 3] = [
    ScreenReader::Status,
    ScreenReader::Messages,
    ScreenReader::Everything,
];

/// How many builds agents start may run at once; 0 lets the host size it.
const BUILD_RUNS: [u32; 9] = [0, 1, 2, 3, 4, 6, 8, 12, 16];

/// The themes new agents' names come from: the core's word, and how the menu shows it.
const NAMES: [(NameTheme, &str); 2] = [
    (NameTheme::Nature, n!("Nature: Otter, Maple, Flint")),
    (NameTheme::Anime, n!("Anime: Luffy, Gojo, Frieren")),
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
                NAMES
                    .iter()
                    .map(|(_, label)| crate::i18n::tr(label))
                    .collect(),
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
                t!("Steer: the agent reads it in the running turn"),
                t!("Queue: it waits until the turn ends"),
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
            vec![t!("Comfortable"), t!("Compact")],
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
        let screen_reader = Choice::new(
            vec![
                t!("Status only"),
                t!("Status and messages"),
                t!("Everything"),
            ],
            READINGS
                .iter()
                .position(|reading| *reading == preferences.screen_reader)
                .unwrap_or(0),
            |bench, at, _| {
                crate::settings::set(bench, PreferenceChange::ScreenReader(READINGS[at]))
            },
            window,
            cx,
        );
        let builds = Choice::new(
            BUILD_RUNS
                .iter()
                .map(|runs| match runs {
                    0 => t!("Automatic").to_owned(),
                    runs => runs.to_string(),
                })
                .collect(),
            BUILD_RUNS
                .iter()
                .position(|runs| *runs == preferences.build_runs)
                .unwrap_or(0),
            |bench, at, _| crate::settings::set(bench, PreferenceChange::BuildRuns(BUILD_RUNS[at])),
            window,
            cx,
        );
        Self {
            mid_turn,
            density,
            screen_reader,
            builds,
            names: None,
        }
    }
}

/// Each mode agents can start in: its word in the core, its name, and what it does.
const MODES: [(PermissionMode, &str, &str); 3] = [
    (
        PermissionMode::WorkspaceWrite,
        n!("Normal"),
        n!("Use the agent's normal approval and sandbox behavior."),
    ),
    (
        PermissionMode::Auto,
        n!("Auto"),
        n!("The agent's own reviewer approves safe actions and asks you the risky ones. Agents without one run as Normal."),
    ),
    (
        PermissionMode::Bypass,
        "YOLO",
        n!("Bypass approvals and sandboxing."),
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
                        .child(crate::i18n::tr(name)),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
                        .text_color(t.text_tertiary)
                        .child(crate::i18n::tr(detail)),
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
        .map_or(n!("Normal"), |(_, name, _)| *name);
    rows::section(
        n!("Launch boundary"),
        Some(crate::i18n::tr(name).into()),
        Some(t!("Offered before every launch. Providers without a matching CLI control fall back to their own settings.").into()),
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
        ui::Button::new("sleep-idle", t!("Sleep now"), ui::Variant::Outline).on_click(
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
            t!("Sending while an agent works"),
            Some(t!("What ↵ does mid-turn. {keys} does the other.", keys = keys).into()),
            fields.mid_turn.view(),
            cx,
        ))
        .child(rows::row(
            t!("Rail density"),
            Some(t!("Compact fits more sessions while keeping every state symbol visible.")),
            fields.density.view(),
            cx,
        ))
        .child(rows::row(
            t!("Screen reader"),
            Some(t!("What VoiceOver hears of the chat in front while its turn runs. Messages are read once the agent moves on from them; the closing reply is left for you.")),
            fields.screen_reader.view(),
            cx,
        ))
        .children(fields.names.as_ref().map(|names| {
            rows::row(
                t!("Agent names"),
                Some(t!("What new agents are called. Open agents keep their names.")),
                names.view(),
                cx,
            )
        }))
        .child(rows::row(
            t!("Heavy commands at once"),
            Some(t!("How many heavy commands agents start, such as builds and test suites, run side by side. The rest wait in line and say so. Automatic starts the next one when this host's free memory holds it.")),
            fields.builds.view(),
            cx,
        ))
        .child(rows::row(
            t!("Idle chats"),
            Some(t!("Put every idle chat to sleep now, freeing its process.")),
            sleep,
            cx,
        ))
        .child(rows::note(
            t!("Only confirmed native session IDs are written to disk. Startup commands and terminal output never are."),
            cx,
        ));
    rows::section(n!("Chats"), None, None, lit("Chats"), body, cx)
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
            t!("Notify when an agent needs you"),
            Some(t!("While Sikemux is in the background, a notification says when an agent asks for input or finishes.")),
            rows::switch(
                "agent-notifications",
                PreferenceChange::AgentNotifications,
                preferences.agent_notifications,
                this,
            ),
            cx,
        ))
        .child(rows::row(
            t!("Notify when a goal moves"),
            Some(t!("A short notification when slices land, a goal finishes or its ETA slips, and never for what you're looking at. What needs you always notifies.")),
            rows::switch(
                "goal-notifications",
                PreferenceChange::GoalNotifications,
                preferences.goal_notifications,
                this,
            ),
            cx,
        ))
        .child(rows::row(
            t!("Test notification"),
            Some(t!("Sends one now, with its sound. The first one also has macOS ask whether Sikemux may notify you.")),
            {
                let this = this.clone();
                ui::Button::new("notification-test", t!("Send test"), ui::Variant::Outline)
                    .on_click(move |_, _, cx| this.update(cx, |_, cx| crate::notifications::test(cx)))
            },
            cx,
        ));
    rows::section(
        n!("Notifications"),
        None,
        None,
        lit("Notifications"),
        body,
        cx,
    )
}
