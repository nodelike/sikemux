//! Sharing a chat read-only: a link anyone signed in to Sikemux can read it from, as it
//! happens. The core opens and keeps the share; the window lists them and asks.

use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, ClipboardItem, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, Window,
};
use serde::Deserialize;
use serde_json::json;

use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_3, TEXT_SM, TEXT_XS};
use crate::ui::{self, dialog, Button, Variant};
use crate::workbench::Workbench;

const HOUR_MS: u64 = 60 * 60 * 1000;
const LIFETIMES: [(&str, u64); 3] = [
    ("1 hour", HOUR_MS),
    ("24 hours", 24 * HOUR_MS),
    ("7 days", 7 * 24 * HOUR_MS),
];

/// A chat this host shares, as the core's `shares` lists it.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    id: String,
    agent_id: String,
    url: String,
    expires_at: u64,
    viewers: u32,
    state: String,
    problem: Option<String>,
}

impl Share {
    /// Who reads it now, in the words beside the chat's name and in the dialog.
    pub fn watching(&self) -> String {
        match (self.state.as_str(), self.viewers) {
            ("connecting", _) => "Connecting…".to_owned(),
            (_, 0) => "No one watching yet".to_owned(),
            (_, 1) => "1 person watching".to_owned(),
            (_, viewers) => format!("{viewers} people watching"),
        }
    }

    pub fn viewers(&self) -> u32 {
        self.viewers
    }
}

impl Workbench {
    fn shares(&self) -> Vec<Share> {
        self.reads
            .shares
            .as_ref()
            .and_then(|shares| serde_json::from_value(shares.clone()).ok())
            .unwrap_or_default()
    }

    /// The share of `agent`'s chat that is open or opening.
    pub fn share_of(&self, agent: &str) -> Option<Share> {
        self.shares()
            .into_iter()
            .find(|share| share.agent_id == agent && share.state != "refused")
    }

    /// Why the server turned down the last share of `agent`'s chat, while it stands.
    fn refusal_of(&self, agent: &str) -> Option<String> {
        let refused = self
            .shares()
            .into_iter()
            .find(|share| share.agent_id == agent && share.state == "refused");
        refused.and_then(|share| share.problem)
    }

    /// Opens the dialog that shares `agent`'s chat, or shows the share it has.
    pub fn open_share(&mut self, agent: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.overlays.sharing = Some(Sharing {
            agent: agent.to_owned(),
            lifetime: 1,
            working: false,
            problem: None,
        });
        cx.notify();
    }

    fn agent_title(&self, agent: &str) -> String {
        self.view
            .as_ref()
            .and_then(|view| view.workspace.agents.iter().find(|saved| saved.id == agent))
            .map_or_else(|| "this chat".to_owned(), |saved| saved.title.clone())
    }

    fn close_share(&mut self, cx: &mut Context<Self>) {
        self.overlays.sharing = None;
        cx.notify();
    }

    fn start_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = (self.overlays.sharing.as_ref())
            .filter(|sharing| !sharing.working)
            .map(|sharing| sharing.agent.clone())
        else {
            return;
        };
        let title = self.agent_title(&agent);
        let Some(sharing) = self.overlays.sharing.as_mut() else {
            return;
        };
        let asked = json!({ "agentId": sharing.agent, "lifetimeMs": LIFETIMES[sharing.lifetime].1, "title": title });
        let started = crate::core::request(&self.asks, "shareChat", asked);
        (sharing.working, sharing.problem) = (true, None);
        cx.notify();
        cx.spawn_in(window, async move |bench, cx| {
            let answer = started.await;
            bench
                .update_in(cx, |bench, window, cx| {
                    let url = answer.map(|share| share["url"].as_str().map(str::to_owned));
                    if let Ok(Some(url)) = &url {
                        cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                        window.push_notification("Copied the share link", cx);
                    }
                    if let Some(sharing) = bench.overlays.sharing.as_mut() {
                        sharing.working = false;
                        sharing.problem = match url {
                            Ok(Some(_)) => None,
                            Ok(None) => Some("The core answered without a link".to_owned()),
                            Err(error) => Some(error),
                        };
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn stop_share(&mut self, share: String, window: &mut Window, cx: &mut Context<Self>) {
        let stopped = crate::core::request(&self.asks, "stopShare", json!({ "id": share }));
        if let Some(sharing) = self.overlays.sharing.as_mut() {
            sharing.working = true;
        }
        cx.notify();
        cx.spawn_in(window, async move |bench, cx| {
            let answer = stopped.await;
            bench
                .update(cx, |bench, cx| {
                    match answer {
                        Ok(_) => bench.overlays.sharing = None,
                        Err(error) => {
                            if let Some(sharing) = bench.overlays.sharing.as_mut() {
                                (sharing.working, sharing.problem) = (false, Some(error));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }
}

/// The share dialog while it is open: whose chat, the lifetime picked from `LIFETIMES` by
/// place, and how the last ask went.
pub struct Sharing {
    agent: String,
    lifetime: usize,
    working: bool,
    problem: Option<String>,
}

/// The share dialog, as the Mac's sheet draws it: what sharing shows, then how long the link
/// works or the link itself, and the buttons on the foot.
pub fn view(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let sharing = bench.overlays.sharing.as_ref()?;
    let t = Tokens::get(cx).clone();
    let share = bench.share_of(&sharing.agent);
    let problem = sharing
        .problem
        .clone()
        .or_else(|| bench.refusal_of(&sharing.agent));
    let label = |words: &'static str| {
        ui::text(words, TEXT_XS, FontWeight::MEDIUM).text_color(t.text_tertiary)
    };
    let field = match &share {
        Some(share) => {
            let url = share.url.clone();
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(label("Link"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(32.))
                                .px(SPACE_2)
                                .flex()
                                .items_center()
                                .border_1()
                                .border_color(t.border)
                                .rounded(theme::RADIUS_2)
                                .bg(t.surface_sunken)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(ui::mono(share.url.clone(), TEXT_XS, cx)),
                        )
                        .child(
                            Button::new("share-copy", "Copy", Variant::Outline)
                                .icon(Icon::Copy)
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                    window.push_notification("Copied the share link", cx);
                                }),
                        ),
                )
                .child(
                    ui::text(
                        format!(
                            "{} · ends in {}",
                            share.watching(),
                            crate::ui::time::left(share.expires_at)
                        ),
                        TEXT_XS,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_tertiary),
                )
        }
        None => div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .child(label("Link works for"))
            .child(
                div()
                    .flex()
                    .gap(SPACE_1H)
                    .children(LIFETIMES.iter().enumerate().map(|(at, (words, _))| {
                        let on = at == sharing.lifetime;
                        div()
                            .id(("share-lifetime", at))
                            .h(px(28.))
                            .px(SPACE_3)
                            .flex()
                            .items_center()
                            .rounded(theme::RADIUS_2)
                            .border_1()
                            .border_color(t.border)
                            .text_size(crate::theme::ui_text(TEXT_SM))
                            .cursor_pointer()
                            .map(|pill| match on {
                                true => pill.bg(t.surface_chosen).text_color(t.text_primary),
                                false => pill
                                    .bg(t.surface_raised)
                                    .text_color(t.text_secondary)
                                    .hover(|pill| {
                                        pill.text_color(t.text_primary)
                                            .border_color(t.border_strong)
                                    }),
                            })
                            .on_click(cx.listener(move |bench, _, _, cx| {
                                if let Some(sharing) = bench.overlays.sharing.as_mut() {
                                    sharing.lifetime = at;
                                }
                                cx.notify();
                            }))
                            .child(*words)
                    })),
            ),
    };
    let body = dialog::marked_body()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(dialog::prose("Anyone signed in to Sikemux who has the link can read this chat as it happens, from its start. They cannot type in it or answer its requests.", &t))
                .child(dialog::prose("It shows everything the agent read and ran, including the contents of files and any secrets in them.", &t)),
        )
        .child(div().pt(SPACE_3).child(field))
        .children(problem.map(|problem| {
            div()
                .pt(SPACE_3)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .line_height(crate::theme::ui_text(18.))
                .text_color(t.danger)
                .child(problem)
        }))
        .into_any_element();
    let this = cx.entity();
    let close: dialog::Answer = {
        let this = this.clone();
        Rc::new(move |_, cx| this.update(cx, |bench, cx| bench.close_share(cx)))
    };
    let (title, foot, yes): (&str, Div, dialog::Answer) = match share {
        Some(share) => {
            let stop: dialog::Answer = {
                let this = this.clone();
                Rc::new(move |window, cx| {
                    let id = share.id.clone();
                    this.update(cx, |bench, cx| bench.stop_share(id, window, cx))
                })
            };
            let foot = dialog::hinted_foot(dialog::escape("close", cx), cx)
                .child(dialog::button(
                    "share-stop",
                    "Stop sharing".into(),
                    None,
                    stop,
                    cx,
                ))
                .child(dialog::button(
                    "share-done",
                    "Done".into(),
                    Some(t.accent),
                    close.clone(),
                    cx,
                ));
            ("Shared read-only", foot, close.clone())
        }
        None => {
            let create: dialog::Answer = Rc::new(move |window, cx| {
                this.update(cx, |bench, cx| bench.start_share(window, cx))
            });
            let label = if sharing.working {
                "Creating link…"
            } else {
                "Create link"
            };
            let foot = dialog::hinted_foot(dialog::escape("close", cx), cx)
                .child(dialog::button(
                    "share-cancel",
                    "Cancel".into(),
                    None,
                    close.clone(),
                    cx,
                ))
                .child(dialog::button(
                    "share-create",
                    label.into(),
                    Some(t.accent),
                    create.clone(),
                    cx,
                ));
            ("Share this chat read-only", foot, create)
        }
    };
    Some(
        dialog::marked_sheet(Icon::Share, title.into(), body, foot, yes, close, cx)
            .into_any_element(),
    )
}
