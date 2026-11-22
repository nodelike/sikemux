//! Snoozing and settling agents out of the rail's main list.

use chrono::{Local, TimeZone};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::{
    div, AnyElement, App, Context, IntoElement, ParentElement, Styled, WeakEntity, Window,
};
use sikemux_calls::call;
use sikemux_calls::settling;
use sikemux_calls::settling::SettleParams;
use sikemux_calls::settling::SnoozeParams;
use sikemux_process::clock::epoch_ms;

use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1, SPACE_2};
use crate::toast;
use crate::workbench::Workbench;

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
                    t!("Snoozed until {time}", time = crate::ui::time::clock(until)),
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
            .map(crate::ui::time::weekday)
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
                    let label = format!(
                        "{}  ·  {}",
                        preset.label,
                        crate::ui::time::clock(preset.until)
                    );
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
        true => (t!("Restore"), Icon::Restore),
        false => (t!("Settle"), Icon::Check),
    };
    popup.item(
        PopupMenuItem::new(label)
            .icon(icon)
            .on_click(move |_, _, cx| settle(&bench, &place, id.clone(), !settled, cx)),
    )
}
