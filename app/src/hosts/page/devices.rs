//! The page's devices: each phone or browser paired with this host, and how to pair another.

use gpui_kit::base::Popover;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::Disableable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, Context, Div, FontWeight, IntoElement, ParentElement,
    SharedString, Styled,
};
use sikemux_wire::protocol::{DeviceAccess, DeviceInfo, NotificationState, RemoteStatus};

use super::board::lifted;
use super::{hair, ink};
use crate::core::{Ask, RemoteAsk};
use crate::icons::Icon;
use crate::theme::{
    Tokens, RADIUS_4, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, SPACE_5, TEXT_2XS_H, TEXT_MD,
    TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

fn signed_in(bench: &Workbench) -> bool {
    (bench.this_view()).is_some_and(|view| view.account.is_some())
}

/// "Pair device" in the page's header, showing how to pair a phone.
pub fn pair_button(bench: &Workbench) -> AnyElement {
    let button = Button::new("devices-pair", "Pair device", Variant::Outline).icon(Icon::Plus);
    if !signed_in(bench) {
        return button
            .disabled(true)
            .tooltip("Sign in to pair a device")
            .into_any_element();
    }
    Popover::new("devices-pair-popover")
        .anchor(Anchor::TopRight)
        .offset(SPACE_1H)
        .trigger_with(move |_, _, _| {
            Button::new("devices-pair", "Pair device", Variant::Outline)
                .icon(Icon::Plus)
                .into_any_element()
        })
        .content(|_, _, cx| {
            ui::float(cx)
                .w(px(480.))
                .child(phone_panel(cx))
                .into_any_element()
        })
        .into_any_element()
}

/// The devices paired with this host, under their count and the switch that lets them reach it.
pub fn devices(bench: &Workbench, query: &str, t: &Tokens, cx: &mut Context<Workbench>) -> Div {
    let status = bench.remote.as_ref();
    let all = status.map_or(&[][..], |status| status.devices.as_slice());
    let shown: Vec<&DeviceInfo> = (all.iter())
        .filter(|device| query.is_empty() || matches(device, query))
        .collect();
    let list = match (status, shown.is_empty()) {
        (Some(status), false) => div()
            .rounded(RADIUS_4)
            .border_1()
            .border_color(hair(t))
            .bg(lifted(t, 0.))
            .overflow_hidden()
            .children(
                (shown.iter().enumerate())
                    .map(|(at, device)| device_row(device, status, at > 0, t, cx)),
            )
            .into_any_element(),
        _ => empty(bench, all.is_empty(), t, cx).into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .mt(px(36.))
                .mb(SPACE_3)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ui::caps("Devices", cx))
                .child(ui::mono(all.len().to_string(), TEXT_2XS_H, cx).text_color(t.text_tertiary))
                .child(div().ml_auto().child(remote_access(bench, status, t, cx))),
        )
        .child(list)
}

fn matches(device: &DeviceInfo, query: &str) -> bool {
    device.name.to_lowercase().contains(query)
        || platform_name(&device.platform)
            .to_lowercase()
            .contains(query)
}

fn remote_access(
    bench: &Workbench,
    status: Option<&RemoteStatus>,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> Div {
    let switch = Switch::new("remote-access")
        .checked(status.is_some_and(|status| status.enabled))
        .disabled(status.is_none() || !signed_in(bench))
        .on_click(cx.listener(|bench, on: &bool, _, _| {
            let _ = bench.asks.send(Ask::Remote(RemoteAsk::Enable(*on)));
        }));
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            ui::text("Remote access", TEXT_XS_H, FontWeight::NORMAL).text_color(t.text_secondary),
        )
        .child(switch)
}

fn empty(bench: &Workbench, none_at_all: bool, t: &Tokens, cx: &mut Context<Workbench>) -> Div {
    let words = match (signed_in(bench), none_at_all) {
        (false, _) => "Sign in to pair a phone or browser with this host",
        (true, true) => "No devices yet. Pair one to watch and answer your agents from anywhere",
        (true, false) => "No device matches",
    };
    div()
        .h(px(96.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_2H)
        .border_1()
        .border_dashed()
        .border_color(t.border)
        .rounded(RADIUS_4)
        .text_color(t.text_tertiary)
        .child(ui::text(words, TEXT_XS_H, FontWeight::NORMAL))
        .when(!signed_in(bench) && bench.this_view().is_some(), |empty| {
            empty.child(
                Button::new("devices-sign-in", "Sign in", Variant::Primary)
                    .on_click(cx.listener(|bench, _, _, _| bench.sign_in())),
            )
        })
}

fn device_row(
    device: &DeviceInfo,
    status: &RemoteStatus,
    ruled: bool,
    t: &Tokens,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let connected = status.connected.contains(&device.id);
    let note = notification_note(
        (status.notifications.iter())
            .find(|phone| phone.device_id == device.id)
            .map(|phone| phone.state),
    );
    let seen = match connected {
        true => "connected now".to_owned(),
        false => seen(device.last_seen),
    };
    let line = div()
        .flex()
        .text_size(crate::theme::ui_text(TEXT_2XS_H))
        .text_color(t.text_tertiary)
        .child(format!("{} · {seen}", platform_name(&device.platform)))
        .children(note.map(|(note, warn)| {
            div()
                .when(warn, |note| note.text_color(t.warn))
                .child(format!(" · {note}"))
        }));
    let name = match device.name.is_empty() {
        true => "Unnamed device".to_owned(),
        false => device.name.clone(),
    };
    let glyph = match device.platform.as_str() {
        "web" => Icon::Globe,
        _ => Icon::Phone,
    };
    let access = match device.access {
        DeviceAccess::Full => "Full access",
        DeviceAccess::Watch => "Watch",
    };
    div()
        .min_h(px(54.))
        .px(SPACE_4)
        .flex()
        .items_center()
        .gap(SPACE_3)
        .when(ruled, |row| row.border_t_1().border_color(hair(t)))
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(ink(t, 0.05))
                .border_1()
                .border_color(hair(t))
                .child(ui::icon(glyph, 15.).text_color(if connected {
                    t.text_secondary
                } else {
                    t.text_tertiary
                })),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child(name.clone()),
                )
                .child(line),
        )
        .child(
            div()
                .flex_none()
                .px(SPACE_2)
                .py(px(2.))
                .rounded(px(6.))
                .border_1()
                .border_color(hair(t))
                .bg(ink(t, 0.04))
                .child(
                    ui::text(access, TEXT_2XS_H, FontWeight::MEDIUM).text_color(t.text_secondary),
                ),
        )
        .child(device_menu(device, name, cx))
        .into_any_element()
}

/// A device's ⋯: rename it on the account, choose what it may do here, or take it off this host.
fn device_menu(device: &DeviceInfo, name: String, cx: &mut Context<Workbench>) -> impl IntoElement {
    let (bench, id, access) = (cx.weak_entity(), device.id.clone(), device.access);
    ui::menu_button(
        SharedString::from(format!("device-menu-{id}")),
        Anchor::TopRight,
        ui::IconButton::new(
            SharedString::from(format!("device-more-{id}")),
            Icon::More,
            format!("More for {name}"),
        ),
        move |popup, _, cx| {
            let renaming = (bench.clone(), id.clone(), name.clone());
            let set = |to: DeviceAccess| {
                let (bench, device) = (bench.clone(), id.clone());
                move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
                    let device = device.clone();
                    let _ = bench.update(cx, |bench, _| {
                        let _ = (bench.asks)
                            .send(Ask::Remote(RemoteAsk::Access { device, access: to }));
                    });
                }
            };
            let removing = (bench.clone(), id.clone());
            popup
                .item(
                    PopupMenuItem::new("Rename…")
                        .icon(Icon::Pencil)
                        .on_click(move |_, _, cx| {
                            let (bench, key, named) = &renaming;
                            let _ = bench
                                .update(cx, |bench, cx| bench.rename_on_account(key, named, cx));
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Full access")
                        .icon(Icon::Pointer)
                        .checked(access == DeviceAccess::Full)
                        .on_click(set(DeviceAccess::Full)),
                )
                .item(
                    PopupMenuItem::new("Watch only")
                        .icon(Icon::Eye)
                        .checked(access == DeviceAccess::Watch)
                        .on_click(set(DeviceAccess::Watch)),
                )
                .separator()
                .item(
                    ui::MenuRow::new("Remove")
                        .icon(Icon::Trash)
                        .danger(true)
                        .item(cx)
                        .on_click(move |_, _, cx| {
                            let (bench, device) = &removing;
                            let device = device.clone();
                            let _ = bench.update(cx, |bench, _| {
                                let _ = bench.asks.send(Ask::Remote(RemoteAsk::Revoke(device)));
                            });
                        }),
                )
        },
    )
}

fn platform_name(platform: &str) -> &str {
    match platform {
        "ios" => "iOS",
        "android" => "Android",
        "macos" => "macOS",
        "linux" => "Linux",
        "web" => "Web",
        other => other,
    }
}

fn seen(at: Option<u64>) -> String {
    match at.map(crate::ui::time::age).as_deref() {
        None => "never connected".into(),
        Some("now") => "seen now".into(),
        Some(age) => format!("seen {age} ago"),
    }
}

fn notification_note(state: Option<NotificationState>) -> Option<(&'static str, bool)> {
    Some(match state? {
        NotificationState::On => ("notifications on", false),
        NotificationState::Off => ("notifications off", false),
        NotificationState::PhoneOff => ("notifications turned off on the phone", false),
        NotificationState::NotReaching => ("notifications aren't reaching it", true),
        NotificationState::OtherAccount => {
            ("notifications need it signed in to your account", true)
        }
        NotificationState::SignedOut => ("sign in to send it notifications", false),
    })
}

fn phone_panel(cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let steps = [
        "Scan the code with your phone's camera",
        "Sign in with the same account",
        "Click Allow here when it asks",
    ];
    div()
        .flex()
        .items_center()
        .gap(SPACE_5)
        .p(SPACE_4)
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_MD))
                        .text_color(t.text_primary)
                        .child("Your agents, on your phone"),
                )
                .child(div().text_size(crate::theme::ui_text(TEXT_XS)).text_color(t.text_secondary).child(
                    "Watch them work, answer what they ask and start new chats from anywhere.",
                ))
                .children(steps.iter().enumerate().map(|(at, step)| {
                    div()
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(format!("{}. {step}", at + 1))
                })),
        )
        .child(
            div()
                .flex_none()
                .p(SPACE_2H)
                .rounded(px(8.))
                .bg(gpui_kit::white())
                .child(ui::icon(Icon::PhoneQr, 150.).text_color(gpui_kit::black())),
        )
        .into_any_element()
}
