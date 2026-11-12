//! Snoozing and settling agents out of the rail's main list, and the Undo for the ones the core
//! settles by itself.

use std::collections::HashSet;

use chrono::{Local, TimeZone};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::{
    div, AnyElement, App, Context, IntoElement, ParentElement, Styled, WeakEntity, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::settling::{self, SettleParams, SnoozeParams};
use sikemux_process::clock::epoch_ms;
use sikemux_view::device::DeviceView;

use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1, SPACE_2};
use crate::toast;
use crate::workbench::Workbench;

/// How recently the core must have settled an agent for a client that just heard of it to
/// offer Undo, so opening the app hours later does not.
const FRESH_MS: u64 = 2 * 60_000;

/// The agents in `view` the core settled by itself.
pub fn auto_settled(view: Option<&DeviceView>) -> HashSet<String> {
    view.map(|view| {
        (view.inbox.agents.iter().chain(&view.inbox.kept))
            .filter(|agent| agent.auto_settled)
            .map(|agent| agent.agent_id.clone())
            .collect()
    })
    .unwrap_or_default()
}

/// Offers Undo for the agents the core at `place` has just settled.
pub fn tell(
    bench: &Workbench,
    place: &Place,
    before: &HashSet<String>,
    cx: &mut Context<Workbench>,
) {
    let now = epoch_ms();
    let fresh: Vec<String> = bench
        .view_at(place)
        .into_iter()
        .flat_map(|view| view.inbox.agents.iter().chain(&view.inbox.kept))
        .filter(|agent| agent.auto_settled && !before.contains(&agent.agent_id))
        .filter(|agent| {
            agent
                .settled_at
                .is_some_and(|at| now.saturating_sub(at) < FRESH_MS)
        })
        .map(|agent| agent.agent_id.clone())
        .collect();
    if fresh.is_empty() {
        return;
    }
    let said = match fresh.len() {
        1 => t!("Settled a chat you're done with").to_owned(),
        count => t!("Settled {count} chats you're done with", count = count),
    };
    let (bench_handle, place) = (cx.entity().downgrade(), place.clone());
    bench.notify(
        toast::undoable(said, move |_, cx| {
            settle(&bench_handle, &place, fresh.clone(), false, cx);
        }),
        cx,
    );
}

fn settle(
    bench: &WeakEntity<Workbench>,
    place: &Place,
    agent_ids: Vec<String>,
    settled: bool,
    cx: &mut App,
) {
    let _ = bench.update(cx, |bench, cx| {
        bench.settle_agents(place, agent_ids, settled, cx)
    });
}

fn snooze(
    bench: &WeakEntity<Workbench>,
    place: &Place,
    agent_ids: Vec<String>,
    until: Option<u64>,
    cx: &mut App,
) {
    let _ = bench.update(cx, |bench, cx| {
        bench.snooze_agents(place, agent_ids, until, cx)
    });
}

impl Workbench {
    /// Settles the agents on the host at `place`, offering Undo, or brings them back to the
    /// main list.
    pub fn settle_agents(
        &self,
        place: &Place,
        agent_ids: Vec<String>,
        settled: bool,
        cx: &mut Context<Self>,
    ) {
        Target::new(&self.asks, place.clone()).call_typed::<call::SettleAgents>(SettleParams {
            agent_ids: agent_ids.clone(),
            settled,
        });
        if settled {
            let (handle, place) = (cx.entity().downgrade(), place.clone());
            self.notify(
                toast::undoable(t!("Settled"), move |_, cx| {
                    settle(&handle, &place, agent_ids.clone(), false, cx)
                }),
                cx,
            );
        }
    }

    /// Snoozes the agents until `until`, or wakes them with none, offering Undo for a snooze.
    pub fn snooze_agents(
        &self,
        place: &Place,
        agent_ids: Vec<String>,
        until: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        Target::new(&self.asks, place.clone()).call_typed::<call::SnoozeAgents>(SnoozeParams {
            agent_ids: agent_ids.clone(),
            until,
            typed: None,
        });
        if let Some(until) = until {
            let (handle, place) = (cx.entity().downgrade(), place.clone());
            self.notify(
                toast::undoable(
                    t!("Snoozed until {time}", time = when(until)),
                    move |_, cx| snooze(&handle, &place, agent_ids.clone(), None, cx),
                ),
                cx,
            );
        }
    }
}

/// How soon a snoozed row wakes, after its title: minutes or hours while it is today's, else
/// the weekday.
pub fn wake_label(until: u64, cx: &App) -> AnyElement {
    const HOUR_MS: u64 = 3_600_000;
    let left = until.saturating_sub(epoch_ms());
    let words = match left {
        left if left < HOUR_MS => t!("{minutes}m", minutes = left.div_ceil(60_000).max(1)),
        left if left < 24 * HOUR_MS => {
            t!("{hours}h", hours = ((left + HOUR_MS / 2) / HOUR_MS).max(1))
        }
        _ => Local
            .timestamp_millis_opt(until as i64)
            .single()
            .map(|at| at.format("%a").to_string())
            .unwrap_or_default(),
    };
    let ink = Tokens::get(cx).text_tertiary;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(SPACE_1)
        .pl(SPACE_2)
        .child(crate::ui::icon(Icon::Clock, 11.).text_color(ink))
        .child(crate::ui::meta(words, cx))
        .into_any_element()
}

/// `at` as the menu and toasts say it: the time today, else its weekday and time.
fn when(at: u64) -> String {
    let Some(at) = Local.timestamp_millis_opt(at as i64).single() else {
        return String::new();
    };
    match at.date_naive() == Local::now().date_naive() {
        true => at.format("%-I:%M %p").to_string(),
        false => at.format("%a %-I:%M %p").to_string(),
    }
}

/// Snooze and Settle in an agent's menu, or Wake and Restore once it is out of the main list,
/// asked of the core at `place`.
#[allow(clippy::too_many_arguments)]
pub fn menu_items(
    popup: PopupMenu,
    bench: WeakEntity<Workbench>,
    place: Place,
    agent_id: &str,
    snoozed: bool,
    settled: bool,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let id = vec![agent_id.to_owned()];
    let popup = if snoozed {
        let (bench, id, place) = (bench.clone(), id.clone(), place.clone());
        popup.item(
            PopupMenuItem::new(t!("Wake Now"))
                .icon(Icon::Clock)
                .on_click(move |_, _, cx| snooze(&bench, &place, id.clone(), None, cx)),
        )
    } else {
        let (bench, id, place) = (bench.clone(), id.clone(), place.clone());
        popup.submenu_with_icon(
            Some(Icon::Clock.into()),
            t!("Snooze"),
            window,
            cx,
            crate::ui::menu(move |menu, _, _| {
                let menu = settling::presets().into_iter().fold(menu, |menu, preset| {
                    let (bench, id, place) = (bench.clone(), id.clone(), place.clone());
                    let label = format!("{}  ·  {}", preset.label, when(preset.until));
                    menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        snooze(&bench, &place, id.clone(), Some(preset.until), cx)
                    }))
                });
                let (bench, id, place) = (bench.clone(), id.clone(), place.clone());
                menu.separator()
                    .item(
                        PopupMenuItem::new(t!("Pick a Time…")).on_click(move |_, _, cx| {
                            let (id, place) = (id.clone(), place.clone());
                            let _ = bench.update(cx, |bench, cx| {
                                bench.prompt(
                                    t!("Snooze until, such as fri 2pm or in 90m"),
                                    "",
                                    move |bench, typed, _, cx| match settling::parse_time(&typed) {
                                        Some(until) => {
                                            bench.snooze_agents(&place, id.clone(), Some(until), cx)
                                        }
                                        None => bench.notify(
                                            toast::error(t!(
                                                "Couldn't read \"{typed}\" as a time",
                                                typed = typed
                                            )),
                                            cx,
                                        ),
                                    },
                                    cx,
                                )
                            });
                        }),
                    )
            }),
        )
    };
    let (label, icon) = match settled {
        true => (t!("Restore"), Icon::Rotate),
        false => (t!("Settle"), Icon::Check),
    };
    popup.item(
        PopupMenuItem::new(label)
            .icon(icon)
            .on_click(move |_, _, cx| settle(&bench, &place, id.clone(), !settled, cx)),
    )
}
