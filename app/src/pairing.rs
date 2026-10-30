//! Asks over the whole window whether a phone waiting to connect may, or another host on the
//! account asking to link, the one that has waited longest first, since each gives up after two
//! minutes. One that starts waiting while Sikemux is in the background is announced with a
//! notification and a bounce. Right after signing in, it first asks once about every phone
//! already on the account, as the Mac does.

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, AnyElement, App, Context, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Window,
};
use serde_json::json;
use serde_json::Value;
use sikemux_wire::protocol::{AllowedDevice, DeviceAccess, PendingDevice};

use crate::core::{Ask, RemoteAsk};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4, TEXT_2XS, TEXT_2XS_H,
    TEXT_SM, TEXT_XS,
};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

actions!(pairing, [Allow, Decline, OtherAccess]);

const CONTEXT: &str = "Pairing";

pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Allow, Some(CONTEXT)),
        KeyBinding::new("escape", Decline, Some(CONTEXT)),
        KeyBinding::new("up", OtherAccess, Some(CONTEXT)),
        KeyBinding::new("down", OtherAccess, Some(CONTEXT)),
    ]);
}

/// The question while a phone or host waits: the access a phone would get, the hosts asking to
/// link, and the requests already told of.
pub struct Pairing {
    access: DeviceAccess,
    links: Vec<PendingDevice>,
    /// The phones on the account this host has not paired, asked about once after signing in.
    phones: Vec<AllowedDevice>,
    announced: HashSet<String>,
    ticking: bool,
    focus: FocusHandle,
}

impl Pairing {
    pub fn new(cx: &mut App) -> Self {
        Self {
            access: DeviceAccess::Full,
            links: Vec::new(),
            phones: Vec::new(),
            announced: HashSet::new(),
            ticking: false,
            focus: cx.focus_handle(),
        }
    }
}

/// Who waits: a phone that wants to connect, or another host on the account that wants to link.
#[derive(Clone, Copy, PartialEq)]
enum Asker {
    Phone,
    Host,
}

impl Asker {
    fn name(self, request: &PendingDevice) -> String {
        match (request.name.is_empty(), self) {
            (false, _) => request.name.clone(),
            (true, Asker::Phone) => "A phone".into(),
            (true, Asker::Host) => "Another host".into(),
        }
    }

    /// What the question says, on two lines.
    fn lines(self, name: &str) -> (String, &'static str) {
        match self {
            Asker::Phone => (format!("{name} wants to connect"), "to this host"),
            Asker::Host => (format!("{name} wants to link"), "with this host"),
        }
    }

    /// What the notification says while Sikemux is in the background.
    fn notice(self, name: &str) -> (String, &'static str) {
        match self {
            Asker::Phone => (
                format!("{name} wants to connect to this host"),
                "It is signed in to your Sikemux account. Allow or decline it in Sikemux.",
            ),
            Asker::Host => (
                format!("{name} wants to link with this host"),
                "Its agents could start agents here. Allow or decline it in Sikemux.",
            ),
        }
    }
}

const ACCESS: [(DeviceAccess, Icon, &str, &str); 2] = [
    (
        DeviceAccess::Full,
        Icon::Pointer,
        "Full control",
        "Start chats, answer agents, type in terminals",
    ),
    (
        DeviceAccess::Watch,
        Icon::Eye,
        "Watch only",
        "See chats and terminals, answer what agents ask",
    ),
];

impl Workbench {
    /// Whether a phone or host waits on the person's answer, or the account's phones do.
    pub(crate) fn someone_waits(&self) -> bool {
        !self.pairing.phones.is_empty() || self.waiting().is_some()
    }

    /// Every phone and host waiting, as the person would be asked about them.
    fn waiting_all(&self) -> Vec<(Asker, &PendingDevice)> {
        let phones = self
            .remote
            .iter()
            .flat_map(|remote| &remote.pending)
            .map(|request| (Asker::Phone, request));
        let hosts = self
            .pairing
            .links
            .iter()
            .map(|request| (Asker::Host, request));
        phones.chain(hosts).collect()
    }

    /// The phone or host that has waited longest, which the person is asked about first.
    fn waiting(&self) -> Option<(Asker, &PendingDevice)> {
        self.waiting_all()
            .into_iter()
            .min_by_key(|(_, request)| request.expires_at)
    }

    fn answer_pairing(&mut self, allow: bool, cx: &mut Context<Self>) {
        if !self.pairing.phones.is_empty() {
            return self.answer_account_phones(allow, cx);
        }
        let Some((asker, id)) = self
            .waiting()
            .map(|(asker, request)| (asker, request.id.clone()))
        else {
            return;
        };
        let access = std::mem::replace(&mut self.pairing.access, DeviceAccess::Full);
        let access = match asker {
            Asker::Phone => access,
            Asker::Host => DeviceAccess::Full,
        };
        self.pairing.links.retain(|request| request.id != id);
        let _ = self
            .asks
            .send(Ask::Remote(RemoteAsk::Answer { id, allow, access }));
        self.parts.redraw(&self.parts.over, cx);
    }
}

impl Workbench {
    /// Lets every phone on the account connect, or declines only those already waiting, as the
    /// Mac does: the others can still ask later.
    fn answer_account_phones(&mut self, allow: bool, cx: &mut Context<Self>) {
        let access = std::mem::replace(&mut self.pairing.access, DeviceAccess::Full);
        let mut phones = std::mem::take(&mut self.pairing.phones);
        if allow {
            for phone in &mut phones {
                phone.access = access;
            }
            let _ = self.asks.send(Ask::Remote(RemoteAsk::Allow(phones)));
        } else {
            let waiting: Vec<String> = self
                .remote
                .iter()
                .flat_map(|remote| &remote.pending)
                .filter(|request| phones.iter().any(|phone| phone.id == request.device_id))
                .map(|request| request.id.clone())
                .collect();
            for id in waiting {
                let answer = RemoteAsk::Answer {
                    id,
                    allow: false,
                    access,
                };
                let _ = self.asks.send(Ask::Remote(answer));
            }
        }
        self.parts.redraw(&self.parts.over, cx);
        cx.notify();
    }
}

/// Right after signing in, reads the phones already on the account that this host has not
/// paired, to ask about them all at once.
pub fn offer_account_phones(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let asked = crate::core::request(&bench.asks, "accountPhones", json!({}));
    cx.spawn(async move |bench, cx| {
        let Ok(found) = asked.await else {
            return;
        };
        let text = |phone: &Value, key: &str| phone[key].as_str().unwrap_or_default().to_owned();
        let phones: Vec<AllowedDevice> = found
            .as_array()
            .into_iter()
            .flatten()
            .map(|phone| AllowedDevice {
                id: text(phone, "key"),
                name: text(phone, "name"),
                platform: text(phone, "platform"),
                access: DeviceAccess::Full,
            })
            .filter(|phone| !phone.id.is_empty())
            .collect();
        let _ = bench.update(cx, |bench, cx| {
            bench.pairing.phones = phones;
            bench.parts.redraw(&bench.parts.over, cx);
            cx.notify();
        });
    })
    .detach();
}

/// Hears what the core says of phones waiting, and reads again which hosts ask to link.
pub fn remote_moved(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let asked = crate::core::request(&bench.asks, "pairing", json!({}));
    cx.spawn(async move |bench, cx| {
        let Ok(pairing) = asked.await else {
            return;
        };
        let links = serde_json::from_value(pairing["pendingLinks"].clone()).unwrap_or_default();
        let _ = bench.update(cx, |bench, cx| {
            bench.pairing.links = links;
            waiting_moved(bench, cx);
        });
    })
    .detach();
    waiting_moved(bench, cx);
}

/// Tells the person of phones and hosts that started waiting while Sikemux is in the background,
/// and keeps the countdown moving while one waits.
fn waiting_moved(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let waiting: Vec<(Asker, PendingDevice)> = bench
        .waiting_all()
        .into_iter()
        .map(|(asker, request)| (asker, request.clone()))
        .collect();
    let fresh: Vec<&(Asker, PendingDevice)> = waiting
        .iter()
        .filter(|(_, request)| !bench.pairing.announced.contains(&request.id))
        .collect();
    if !fresh.is_empty() && cx.active_window().is_none() {
        if let Some(window) = cx.windows().first().copied() {
            let _ = window.update(cx, |_, window, _| window.request_attention());
        }
        for (asker, request) in &fresh {
            let (title, body) = asker.notice(&asker.name(request));
            crate::notifications::post(title, body.into(), None, cx);
        }
    }
    bench.pairing.announced = waiting
        .iter()
        .map(|(_, request)| request.id.clone())
        .collect();
    if !waiting.is_empty() && !bench.pairing.ticking {
        bench.pairing.ticking = true;
        cx.spawn(async move |bench, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let going = bench.update(cx, |bench, cx| {
                let going = bench.waiting().is_some();
                bench.pairing.ticking = going;
                match going {
                    true => bench.parts.redraw(&bench.parts.over, cx),
                    false => cx.notify(),
                }
                going
            });
            if !matches!(going, Ok(true)) {
                return;
            }
        })
        .detach();
    }
    cx.notify();
}

/// What is left of the two minutes, as m:ss.
fn waits(expires_at: u64) -> String {
    let now = crate::ui::time::now_ms();
    let left = expires_at.saturating_sub(now).div_ceil(1000);
    format!("{}:{:02}", left / 60, left % 60)
}

pub fn overlay(
    bench: &Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let phones = &bench.pairing.phones;
    let waiting = bench.waiting();
    if phones.is_empty() && waiting.is_none() {
        return None;
    }
    let host = phones.is_empty() && waiting.is_some_and(|(asker, _)| asker == Asker::Host);
    let t = Tokens::get(cx).clone();
    if !bench.pairing.focus.is_focused(window) {
        cx.defer_in(window, |bench, window, cx| {
            bench.pairing.focus.focus(window, cx)
        });
    }
    let (first, second) = match (phones.as_slice(), waiting) {
        ([], Some((asker, request))) => asker.lines(&asker.name(request)),
        (phones, _) => {
            let who = match phones {
                [one] if !one.name.is_empty() => one.name.clone(),
                [_] => "your phone".to_owned(),
                many => format!("your {} phones", many.len()),
            };
            (format!("Let {who} connect"), "to this host?")
        }
    };
    let names = (phones.len() > 1).then(|| {
        phones
            .iter()
            .map(|phone| phone.name.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    });
    let waits_for = waiting
        .filter(|_| phones.is_empty())
        .map(|(_, request)| waits(request.expires_at));
    let decline = if phones.is_empty() {
        "Decline"
    } else {
        "Not now"
    };
    let account = bench
        .view
        .as_ref()
        .and_then(|view| view.account.as_deref())
        .and_then(|account| account.email.clone().or_else(|| account.name.clone()));
    let tile = |icon: Icon| {
        div()
            .size(px(72.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS_4)
            .border_1()
            .border_color(t.border)
            .bg(t.surface_raised)
            .child(ui::icon(icon, 36.).text_color(t.text_primary))
    };
    let choices = ACCESS.iter().map(|&(access, icon, label, detail)| {
        let on = bench.pairing.access == access;
        div()
            .id(SharedString::from(format!("pairing-{label}")))
            .w(px(360.))
            .flex()
            .items_center()
            .gap(SPACE_3)
            .px(SPACE_3H)
            .py(SPACE_2H)
            .rounded(theme::RADIUS_3)
            .border_1()
            .border_color(if on { t.text_secondary } else { t.border })
            .when(on, |choice| choice.bg(t.surface_chosen))
            .child(ui::icon(icon, 16.).text_color(t.text_secondary))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .child(ui::text(label, TEXT_SM, FontWeight::MEDIUM).text_color(t.text_primary))
                    .child(
                        ui::text(detail, TEXT_2XS_H, FontWeight::NORMAL)
                            .text_color(t.text_tertiary),
                    ),
            )
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.pairing.access = access;
                bench.parts.redraw(&bench.parts.over, cx);
            }))
    });
    let hint = |keys: &'static str, what: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(ui::kbd(keys, cx))
            .child(ui::text(what, TEXT_2XS, FontWeight::NORMAL).text_color(t.text_tertiary))
    };
    let column = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(18.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_3H)
                .child(tile(if host { Icon::Laptop } else { Icon::Phone }))
                .child(div().w(px(48.)).h(px(1.)).bg(t.border))
                .child(tile(Icon::Laptop)),
        )
        .child(
            div()
                .text_center()
                .text_size(crate::theme::ui_text(22.))
                .text_color(t.text_primary)
                .child(first)
                .child(div().child(second)),
        )
        .children(names.map(|names| {
            div()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(names)
        }))
        .when(host, |column| {
            column.child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_secondary)
                    .child("Its agents could start agents here, on this host's sign-ins."),
            )
        })
        .children(account.map(|account| {
            div()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(format!("Signed in as {account}"))
        }))
        .when(!host, |column| {
            column.child(div().flex().flex_col().gap(SPACE_2).children(choices))
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2H)
                .children(waits_for.map(|left| ui::mono(format!("waits {left}"), TEXT_2XS, cx)))
                .child(
                    Button::new("pairing-decline", decline, Variant::Outline)
                        .on_click(cx.listener(|bench, _, _, cx| bench.answer_pairing(false, cx))),
                )
                .child(
                    Button::new("pairing-allow", "Allow", Variant::Primary)
                        .on_click(cx.listener(|bench, _, _, cx| bench.answer_pairing(true, cx))),
                ),
        )
        .child(
            div()
                .flex()
                .gap(SPACE_4)
                .child(hint("esc", decline))
                .when(!host, |hints| hints.child(hint("↑↓", "Choose access")))
                .child(hint("↵", "Allow")),
        );
    Some(
        div()
            .id("pairing")
            .key_context(CONTEXT)
            .track_focus(&bench.pairing.focus)
            .on_action(cx.listener(|bench, _: &Allow, _, cx| bench.answer_pairing(true, cx)))
            .on_action(cx.listener(|bench, _: &Decline, _, cx| bench.answer_pairing(false, cx)))
            .on_action(cx.listener(move |bench, _: &OtherAccess, _, cx| {
                if host {
                    return;
                }
                bench.pairing.access = match bench.pairing.access {
                    DeviceAccess::Full => DeviceAccess::Watch,
                    DeviceAccess::Watch => DeviceAccess::Full,
                };
                bench.parts.redraw(&bench.parts.over, cx);
            }))
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(t.surface)
            .child(column)
            .child(
                div()
                    .absolute()
                    .bottom(px(24.))
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                    .text_color(t.text_tertiary)
                    .child("Only allow phones you own. Remove one any time in Settings › Devices."),
            )
            .into_any_element(),
    )
}
