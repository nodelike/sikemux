//! The account this host is signed in to, remote access, the phones that
//! reach it and the other hosts on the account.

use std::collections::HashMap;

use gpui_kit::base::Popover;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::Disableable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, PromptLevel, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window,
};
use serde_json::{json, Value};
use sikemux_wire::accounts::protocol::RevokeReason;
use sikemux_wire::protocol::{
    AccountLink, AccountLinkState, DeviceAccess, DeviceInfo, NotificationState, RemoteStatus,
};

use crate::core::{self, Ask, RemoteAsk};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::Tokens;
use crate::ui::{self, Menu, MenuItem};
use crate::workbench::Workbench;

pub const SECTIONS: &[&str] = &["Your account", "Remote access", "Phones", "Hosts"];

const DELETE_ACCOUNT_URL: &str = "https://app.sikemux.com/delete-account";

/// The other hosts on the account as the core last listed them, and the one being renamed.
#[derive(Default)]
pub struct Fields {
    links: Option<Vec<Value>>,
    renaming: Option<(String, Entity<InputState>, Subscription)>,
    /// Whether each other host asks before a new device reaches it, read on that host itself;
    /// none for a host this app cannot reach now.
    approvals: HashMap<String, Option<bool>>,
}

fn make(bench: &mut Workbench, _: &mut Window, cx: &mut Context<Workbench>) {
    if let Some(settings) = bench.settings.as_mut() {
        settings.pages.devices = Some(Fields::default());
    }
    let _ = bench.asks.send(Ask::Remote(RemoteAsk::Status));
    read_links(bench, cx);
    bench.parts.redraw(&bench.parts.stage, cx);
}

fn fields_of(bench: &mut Workbench) -> Option<&mut Fields> {
    bench.settings.as_mut()?.pages.devices.as_mut()
}

/// Lists the other hosts on the account again.
pub fn read_links(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let Ok(pairing) = core::request(&asks, "pairing", json!({})).await else {
            return;
        };
        let links = pairing["links"].as_array().cloned().unwrap_or_default();
        let hosts: Vec<String> = links
            .iter()
            .filter_map(|link| link["id"].as_str().map(str::to_owned))
            .collect();
        let _ = bench.update(cx, |bench, cx| {
            if let Some(fields) = fields_of(bench) {
                fields.links = Some(links);
            }
            bench.parts.redraw(&bench.parts.stage, cx);
        });
        for host in hosts {
            let read = core::request_on(&asks, &host, "requireApproval", Value::Null).await;
            let on = read.ok().and_then(|read| read["requireApproval"].as_bool());
            let _ = bench.update(cx, |bench, cx| {
                if let Some(fields) = fields_of(bench) {
                    fields.approvals.insert(host, on);
                }
                bench.parts.redraw(&bench.parts.stage, cx);
            });
        }
    })
    .detach();
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
    match at.map(crate::agents::age).as_deref() {
        None => "never connected".into(),
        Some("now") => "seen now".into(),
        Some(age) => format!("seen {age} ago"),
    }
}

fn account_line(link: Option<&AccountLink>) -> &'static str {
    match link.map(|link| link.state) {
        Some(AccountLinkState::Connecting) => "Connecting to your account",
        Some(AccountLinkState::Offline) => "Can't reach your account, retrying",
        _ => "Signed in to Sikemux",
    }
}

/// Why this host is signed out, when the account let it go rather than the person here.
fn removal_note(link: Option<&AccountLink>) -> Option<&'static str> {
    let link = link.filter(|link| link.state == AccountLinkState::Removed)?;
    Some(match link.reason {
        Some(RevokeReason::AccountDeleted) => {
            "Your account was deleted. Phones already allowed stay allowed."
        }
        Some(RevokeReason::SignedOut) => {
            "This host was signed out of your account. Phones already allowed stay allowed."
        }
        _ => "This host was removed from your account at app.sikemux.com. Phones already allowed stay allowed.",
    })
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

/// A row at the top of the page: a mark, what it is, a line about it, and its control.
fn top_row(
    mark: AnyElement,
    title: impl Into<SharedString>,
    about: impl Into<SharedString>,
    control: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    rows::row_with(
        Some(
            ui::mark_box(28., mark)
                .text_color(t.text_secondary)
                .into_any_element(),
        ),
        title,
        Some(about.into()),
        control,
        cx,
    )
    .into_any_element()
}

fn set_require_approval(bench: &Workbench, on: bool, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let answer = core::request(
            &asks,
            "setRequireApproval",
            json!({ "requireApproval": on }),
        )
        .await;
        let Some(status) = answer
            .ok()
            .and_then(|status| serde_json::from_value::<RemoteStatus>(status).ok())
        else {
            return;
        };
        let _ = bench.update(cx, |bench, cx| {
            bench.remote = Some(status);
            bench.parts.redraw(&bench.parts.stage, cx);
        });
    })
    .detach();
}

fn sign_in_panel(
    bench: &Workbench,
    link: Option<&AccountLink>,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let waiting = bench.signing_in;
    let (title, lede) = match waiting {
        true => (
            "Finish signing in in your browser",
            "Sikemux opened the sign-in page in your default browser.",
        ),
        false => (
            "Sign in to your Sikemux account",
            removal_note(link).unwrap_or(
                "Your phone finds this host through it. Sign in with Google, GitHub or your email, in your browser.",
            ),
        ),
    };
    let this = this.clone();
    let action = match waiting {
        true => ui::Button::new("devices-cancel-sign-in", "Cancel", ui::Variant::Outline)
            .on_click(move |_, _, cx| this.update(cx, |bench, _| bench.cancel_sign_in())),
        false => ui::Button::new("devices-sign-in", "Sign in", ui::Variant::Primary)
            .on_click(move |_, _, cx| this.update(cx, |bench, _| bench.sign_in())),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(16.))
        .child(
            div()
                .text_size(crate::theme::ui_text(14.))
                .text_color(t.text_primary)
                .child(title),
        )
        .child(
            div()
                .text_size(crate::theme::ui_text(12.))
                .text_color(t.text_secondary)
                .child(lede),
        )
        .child(div().pt(px(4.)).flex().child(action))
        .into_any_element()
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
        .gap(px(20.))
        .p(px(16.))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(crate::theme::ui_text(14.))
                        .text_color(t.text_primary)
                        .child("Your agents, on your phone"),
                )
                .child(div().text_size(crate::theme::ui_text(12.)).text_color(t.text_secondary).child(
                    "Watch them work, answer what they ask and start new chats from anywhere.",
                ))
                .children(steps.iter().enumerate().map(|(at, step)| {
                    div()
                        .text_size(crate::theme::ui_text(12.))
                        .text_color(t.text_secondary)
                        .child(format!("{}. {step}", at + 1))
                })),
        )
        .child(
            div()
                .flex_none()
                .p(px(10.))
                .rounded(px(8.))
                .bg(gpui_kit::white())
                .child(ui::icon(Icon::PhoneQr, 150.).text_color(gpui_kit::black())),
        )
        .into_any_element()
}

fn phone_row(
    device: &DeviceInfo,
    status: &RemoteStatus,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let connected = status.connected.contains(&device.id);
    let note = notification_note(
        status
            .notifications
            .iter()
            .find(|phone| phone.device_id == device.id)
            .map(|phone| phone.state),
    );
    let seen = match connected {
        true => "connected now".to_owned(),
        false => seen(device.last_seen),
    };
    let line = div()
        .flex()
        .text_size(crate::theme::ui_text(11.5))
        .text_color(t.text_tertiary)
        .child(format!("{} · {seen}", platform_name(&device.platform)))
        .children(note.map(|(note, warn)| {
            div()
                .when(warn, |note| note.text_color(t.warn))
                .child(format!(" · {note}"))
        }));
    let name = match device.name.is_empty() {
        true => "Unnamed phone".to_owned(),
        false => device.name.clone(),
    };
    let (access_this, remove_this) = (this.clone(), this.clone());
    let (access_id, remove_id) = (device.id.clone(), device.id.clone());
    let access = ui::Segmented::new(
        SharedString::from(format!("device-access-{}", device.id)),
        vec![
            (Icon::Pointer, SharedString::from("Full control")),
            (Icon::Eye, SharedString::from("Watch only")),
        ],
        match device.access {
            DeviceAccess::Full => 0,
            DeviceAccess::Watch => 1,
        },
    )
    .on_select(move |chosen, _, cx| {
        let access = [DeviceAccess::Full, DeviceAccess::Watch][*chosen];
        let device = access_id.clone();
        access_this.update(cx, |bench, _| {
            let _ = bench
                .asks
                .send(Ask::Remote(RemoteAsk::Access { device, access }));
        })
    });
    let remove = ui::Button::new(
        SharedString::from(format!("device-remove-{}", device.id)),
        "Remove",
        ui::Variant::Outline,
    )
    .icon(Icon::Trash)
    .on_click(move |_, _, cx| {
        let device = remove_id.clone();
        remove_this.update(cx, |bench, _| {
            let _ = bench.asks.send(Ask::Remote(RemoteAsk::Revoke(device)));
        })
    });
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(crate::theme::ui_text(13.))
                        .text_color(t.text_primary)
                        .child(name),
                )
                .child(line),
        )
        .child(access)
        .child(remove)
        .into_any_element()
}

fn rename(
    bench: &mut Workbench,
    id: String,
    name: String,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let input = cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_value(name, window, cx);
        input
    });
    let done = cx.subscribe_in(&input, window, |bench, input, event: &InputEvent, _, cx| {
        if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
            return;
        }
        let name = input.read(cx).value().trim().to_owned();
        let Some((id, _, _)) = fields_of(bench).and_then(|fields| fields.renaming.take()) else {
            return;
        };
        let unchanged = fields_of(bench)
            .and_then(|fields| fields.links.as_ref())
            .and_then(|links| links.iter().find(|link| link["id"] == id.as_str()))
            .is_some_and(|link| link["name"] == name.as_str());
        if !name.is_empty() && !unchanged {
            let asks = bench.asks.clone();
            cx.spawn(async move |bench, cx| {
                let _ = core::request(
                    &asks,
                    "renameAccountDevice",
                    json!({ "key": id, "name": name }),
                )
                .await;
                let _ = bench.update(cx, |bench, cx| read_links(bench, cx));
            })
            .detach();
        }
        bench.parts.redraw(&bench.parts.stage, cx);
    });
    input.update(cx, |input, cx| input.focus(window, cx));
    if let Some(fields) = fields_of(bench) {
        fields.renaming = Some((id, input, done));
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

fn remove_host(
    bench: &Workbench,
    id: String,
    name: String,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let title = format!(
        "Remove {} from your account?",
        if name.is_empty() { "this host" } else { &name }
    );
    let answer = window.prompt(
        PromptLevel::Warning,
        &title,
        Some("Every device and host on your account stops reaching it at once. To add it again, sign it in again."),
        &["Remove", "Cancel"],
        cx,
    );
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        if answer.await != Ok(0) {
            return;
        }
        let _ = core::request(&asks, "removeAccountDevice", json!({ "key": id })).await;
        let _ = bench.update(cx, |bench, cx| read_links(bench, cx));
    })
    .detach();
}

/// Has another host ask, or stop asking, before a new device reaches it, through this app's
/// own connection to it.
fn set_host_approval(bench: &Workbench, host: String, on: bool, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let params = json!({ "requireApproval": on });
        let set = core::request_on(&asks, &host, "setRequireApproval", params).await;
        if set.is_err() {
            return;
        }
        let _ = bench.update(cx, |bench, cx| {
            if let Some(fields) = fields_of(bench) {
                fields.approvals.insert(host, Some(on));
            }
            bench.parts.redraw(&bench.parts.stage, cx);
        });
    })
    .detach();
}

fn host_row(link: &Value, fields: &Fields, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = link["id"].as_str().unwrap_or_default().to_owned();
    let name = link["name"].as_str().unwrap_or_default().to_owned();
    let seen = match link["connected"].as_bool() == Some(true) {
        true => "connected now".to_owned(),
        false => seen(link["lastSeen"].as_u64()),
    };
    let line = format!(
        "{} · {seen}",
        platform_name(link["platform"].as_str().unwrap_or_default())
    );
    let title: AnyElement = match &fields.renaming {
        Some((renaming, input, _)) if *renaming == id => div()
            .w(px(240.))
            .child(ui::Field::new(input))
            .into_any_element(),
        _ => {
            let (this, id, named) = (this.clone(), id.clone(), name.clone());
            div()
                .id(SharedString::from(format!("host-name-{id}")))
                .text_size(crate::theme::ui_text(13.))
                .text_color(t.text_primary)
                .child(if name.is_empty() {
                    "Unnamed host".to_owned()
                } else {
                    name.clone()
                })
                .on_click(move |event, window, cx| {
                    if event.click_count() == 2 {
                        let (id, named) = (id.clone(), named.clone());
                        this.update(cx, |bench, cx| rename(bench, id, named, window, cx));
                    }
                })
                .into_any_element()
        }
    };
    let (rename_this, remove_this, approve_this) = (this.clone(), this.clone(), this.clone());
    let approval = fields.approvals.get(&id).copied().flatten();
    let menu = Popover::new(SharedString::from(format!("host-menu-{id}")))
        .anchor(Anchor::TopRight)
        .offset(px(4.))
        .trigger(ui::IconButton::new(
            SharedString::from(format!("host-more-{id}")),
            Icon::More,
            format!("More for {name}"),
        ))
        .content(move |_, _, cx| {
            let popover = cx.entity();
            let (rename_this, remove_this) = (rename_this.clone(), remove_this.clone());
            let (rename_id, remove_id) = (id.clone(), id.clone());
            let (rename_name, remove_name) = (name.clone(), name.clone());
            let shut = popover.clone();
            let (approve_id, approve_shut) = (id.clone(), popover.clone());
            Menu::new()
                .child(
                    MenuItem::new("host-rename", "Rename")
                        .icon(Icon::Pencil)
                        .on_click(move |_, window, cx| {
                            let (id, name) = (rename_id.clone(), rename_name.clone());
                            rename_this.update(cx, |bench, cx| rename(bench, id, name, window, cx));
                            popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        }),
                )
                .child({
                    let (approve_this, approve_id, shut) = (
                        approve_this.clone(),
                        approve_id.clone(),
                        approve_shut.clone(),
                    );
                    MenuItem::new("host-approval", "Require approval for new devices")
                        .icon(Icon::Shield)
                        .checked(approval == Some(true))
                        .disabled(approval.is_none())
                        .on_click(move |_, window, cx| {
                            let host = approve_id.clone();
                            approve_this.update(cx, |bench, cx| {
                                set_host_approval(bench, host, approval != Some(true), cx)
                            });
                            shut.update(cx, |popover, cx| popover.dismiss(window, cx));
                        })
                })
                .child(
                    MenuItem::new("host-remove", "Remove from account")
                        .icon(Icon::Trash)
                        .on_click(move |_, window, cx| {
                            let (id, name) = (remove_id.clone(), remove_name.clone());
                            remove_this
                                .update(cx, |bench, cx| remove_host(bench, id, name, window, cx));
                            shut.update(cx, |popover, cx| popover.dismiss(window, cx));
                        }),
                )
        });
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(title)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(11.5))
                        .text_color(t.text_tertiary)
                        .child(line),
                ),
        )
        .child(menu)
        .into_any_element()
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.devices else {
        cx.defer_in(window, make);
        return Vec::new();
    };
    let t = Tokens::get(cx).clone();
    let known = bench.view.is_some();
    let account = bench.view.as_ref().and_then(|view| view.account.as_deref());
    let signed_in = account.is_some();
    let status = bench.remote.as_ref();
    let link = status.and_then(|status| status.account.as_ref());
    let devices = status
        .map(|status| status.devices.as_slice())
        .unwrap_or_default();

    let mut top: Vec<AnyElement> = Vec::new();
    let mut access: Vec<AnyElement> = Vec::new();
    match account {
        None if !known => top.push(top_row(
            div()
                .size(px(28.))
                .rounded_full()
                .bg(t.surface_raised)
                .into_any_element(),
            "Your account",
            "Checking…",
            div().into_any_element(),
            cx,
        )),
        None => {}
        Some(account) => {
            let (delete, leave) = (this.clone(), this.clone());
            top.push(top_row(
                crate::rail::avatar(bench, account, 28., &t).into_any_element(),
                account
                    .email
                    .clone()
                    .or_else(|| account.name.clone())
                    .unwrap_or_else(|| "Signed in".into()),
                account_line(link),
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        ui::Button::new("account-delete", "Delete account…", ui::Variant::Outline)
                            .on_click(move |_, _, cx| {
                                delete.update(cx, |_, cx| cx.open_url(DELETE_ACCOUNT_URL))
                            }),
                    )
                    .child(
                        ui::Button::new("account-sign-out", "Sign out", ui::Variant::Outline)
                            .on_click(move |_, _, cx| {
                                leave.update(cx, |bench, _| bench.sign_out())
                            }),
                    )
                    .into_any_element(),
                cx,
            ));
        }
    }
    let remote = {
        let this = this.clone();
        Switch::new("remote-access")
            .checked(status.is_some_and(|status| status.enabled))
            .disabled(status.is_none() || !signed_in)
            .on_click(move |on: &bool, _, cx| {
                let on = *on;
                this.update(cx, |bench, _| {
                    let _ = bench.asks.send(Ask::Remote(RemoteAsk::Enable(on)));
                })
            })
    };
    access.push(top_row(
        ui::icon(Icon::Globe, 18.).into_any_element(),
        "Remote access",
        match signed_in {
            true => "Your devices and hosts on your account connect here",
            false => "Sign in to use Sikemux on your phone",
        },
        remote.into_any_element(),
        cx,
    ));
    if signed_in {
        let this = this.clone();
        let approval = Switch::new("require-approval")
            .checked(status.is_some_and(|status| status.require_approval))
            .disabled(status.is_none())
            .on_click(move |on: &bool, _, cx| {
                let on = *on;
                this.update(cx, |bench, cx| set_require_approval(bench, on, cx))
            });
        access.push(top_row(
            ui::icon(Icon::Shield, 18.).into_any_element(),
            "Require approval for new devices",
            "Ask before a device or host on your account connects here for the first time",
            approval.into_any_element(),
            cx,
        ));
    }

    let account_section: AnyElement = match (known, signed_in) {
        (true, false) => rows::section(
            "Your account",
            None,
            None,
            lit("Your account"),
            sign_in_panel(bench, link, this, cx),
            cx,
        ),
        _ => rows::section(
            "Your account",
            None,
            None,
            lit("Your account"),
            div().children(top),
            cx,
        ),
    };
    let phone_waiting = signed_in && status.is_some() && devices.is_empty();
    let access_section = div()
        .flex()
        .flex_col()
        .gap(px(28.))
        .child(rows::section(
            "Remote access",
            None,
            None,
            lit("Remote access"),
            div().children(access),
            cx,
        ))
        .when(phone_waiting, |access| {
            access.child(rows::section(
                "Your phone",
                None,
                None,
                false,
                phone_panel(cx),
                cx,
            ))
        })
        .into_any_element();
    let phones_section: AnyElement = match !known || signed_in || !devices.is_empty() {
        false => div().into_any_element(),
        true => {
            let phones: AnyElement = match status {
                None => div().into_any_element(),
                Some(_) if devices.is_empty() => {
                    rows::note("None yet. Waiting for your phone.", cx)
                }
                Some(status) => div()
                    .children(
                        devices
                            .iter()
                            .map(|device| phone_row(device, status, this, cx)),
                    )
                    .into_any_element(),
            };
            rows::section("Phones", None, None, lit("Phones"), phones, cx)
        }
    };
    let hosts_section: AnyElement = match (signed_in, &fields.links) {
        (true, Some(links)) => {
            let hosts: AnyElement = match links.is_empty() {
                true => rows::note(
                    "No other hosts on your account. Sign in to Sikemux on one to add it.",
                    cx,
                ),
                false => div()
                    .children(links.iter().map(|link| host_row(link, fields, this, cx)))
                    .into_any_element(),
            };
            rows::section(
                "Hosts",
                None,
                Some("Every host signed in to your account links with this one, so agents here can start agents there, on that host's own sign-ins.".into()),
                lit("Hosts"),
                hosts,
                cx,
            )
        }
        _ => div().into_any_element(),
    };
    vec![
        account_section,
        access_section,
        phones_section,
        hosts_section,
    ]
}
