//! Sharing a chat read-only: a link its holders read it from as it happens, with or
//! without signing in to Sikemux. The core opens and keeps the share; the window lists them and asks.

use crate::ui::Clicks;
use sikemux_calls::share::StopShareParams;
use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, ClipboardItem, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Styled, Window,
};
use sikemux_calls::share::ShareState;
use sikemux_calls::share::ShareView;

use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, SPACE_3, TEXT_SM, TEXT_XS};
use crate::ui::{self, dialog, Button, Variant};
use crate::workbench::Workbench;
use sikemux_calls::call;
use sikemux_calls::share::ShareChatParams;

const HOUR_MS: u64 = 60 * 60 * 1000;
const LIFETIMES: [(&str, u64); 3] = [
    (n!("1 hour"), HOUR_MS),
    (n!("24 hours"), 24 * HOUR_MS),
    (n!("7 days"), 7 * 24 * HOUR_MS),
];
const READERS: [(&str, bool); 2] = [
    (n!("Anyone with the link"), true),
    (n!("Sikemux accounts"), false),
];

/// Who reads `share` now, in the words beside the chat's name and in the dialog.
pub fn watching(share: &ShareView) -> String {
    match (share.state, share.viewers) {
        (ShareState::Connecting, _) => t!("Connecting…").to_owned(),
        (_, 0) => t!("No one watching yet").to_owned(),
        (_, 1) => t!("1 person watching").to_owned(),
        (_, viewers) => t!("{count} people watching", count = viewers),
    }
}

impl Workbench {
    /// The share of `agent`'s chat that is open or opening.
    pub fn share_of(&self, agent: &str) -> Option<&ShareView> {
        (self.reads.shares.iter())
            .find(|share| share.agent_id == agent && share.state != ShareState::Refused)
    }

    /// Why the server turned down the last share of `agent`'s chat, while it stands.
    fn refusal_of(&self, agent: &str) -> Option<String> {
        (self.reads.shares.iter())
            .find(|share| share.agent_id == agent && share.state == ShareState::Refused)
            .and_then(|share| share.problem.clone())
    }

    /// Opens the dialog that shares `agent`'s chat, or shows the share it has.
    pub fn open_share(&mut self, agent: &str, _: &mut Window, cx: &mut Context<Self>) {
        let anyone = self.share_of(agent).is_none_or(|share| share.anyone);
        self.overlays.sharing = Some(Sharing {
            agent: agent.to_owned(),
            lifetime: 1,
            anyone,
            working: false,
            problem: None,
        });
        cx.notify();
    }

    fn agent_title(&self, agent: &str) -> String {
        self.this_view()
            .and_then(|view| view.workspace.agents.iter().find(|saved| saved.id == agent))
            .map_or_else(|| "this chat".to_owned(), |saved| saved.title.clone())
    }

    fn close_share(&mut self, cx: &mut Context<Self>) {
        self.overlays.sharing = None;
        cx.notify();
    }

    /// Asks the core to share the chat as the dialog has it: a new link, which is copied, or
    /// who may read the one it has.
    fn start_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = (self.overlays.sharing.as_ref())
            .filter(|sharing| !sharing.working)
            .map(|sharing| sharing.agent.clone())
        else {
            return;
        };
        let title = self.agent_title(&agent);
        let fresh = self.share_of(&agent).is_none();
        let Some(sharing) = self.overlays.sharing.as_mut() else {
            return;
        };
        let Some(&(_, lifetime_ms)) = LIFETIMES.get(sharing.lifetime) else {
            return;
        };
        let anyone = sharing.anyone;
        let started = crate::core::Target::here(&self.asks).request_typed::<call::ShareChat>(
            ShareChatParams {
                agent_id: sharing.agent.clone(),
                lifetime_ms,
                title: Some(title),
                anyone: sharing.anyone,
            },
        );
        (sharing.working, sharing.problem) = (true, None);
        cx.notify();
        cx.spawn_in(window, async move |bench, cx| {
            let answer = started.await;
            bench
                .update_in(cx, |bench, window, cx| {
                    let url = answer.map(|share| share.url);
                    match &url {
                        Ok(url) if fresh => {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                            window.push_notification(t!("Copied the share link"), cx);
                        }
                        Ok(_) if anyone => {
                            window.push_notification(t!("Anyone with the link can read it now"), cx)
                        }
                        Ok(_) => window.push_notification(
                            t!("Only people signed in to Sikemux can read it now"),
                            cx,
                        ),
                        Err(_) => {}
                    }
                    if let Some(sharing) = bench.overlays.sharing.as_mut() {
                        sharing.working = false;
                        sharing.problem = url.err();
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn stop_share(&mut self, share: String, window: &mut Window, cx: &mut Context<Self>) {
        let stopped = crate::core::Target::here(&self.asks)
            .request_typed::<call::StopShare>(StopShareParams { id: share });
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
/// place, who may read it, and how the last ask went.
pub struct Sharing {
    agent: String,
    lifetime: usize,
    anyone: bool,
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
                .child(label(t!("Link")))
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
                                .border(crate::theme::border())
                                .border_color(t.border)
                                .rounded(theme::radius_2())
                                .bg(t.surface_sunken)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(ui::mono(share.url.clone(), TEXT_XS, cx)),
                        )
                        .child(
                            Button::new("share-copy", t!("Copy"), Variant::Outline)
                                .icon(Icon::Copy)
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                    window.push_notification(t!("Copied the share link"), cx);
                                }),
                        ),
                )
                .child(
                    ui::text(
                        t!(
                            "{watching} · ends in {left}",
                            watching = watching(share),
                            left = crate::ui::time::left(share.expires_at)
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
            .child(label(t!("Link works for")))
            .child(
                div()
                    .flex()
                    .gap(SPACE_1H)
                    .children(LIFETIMES.iter().enumerate().map(|(at, (words, _))| {
                        pill(("share-lifetime", at), words, at == sharing.lifetime, &t).clicks(
                            cx.listener(move |bench, _, _, cx| {
                                if let Some(sharing) = bench.overlays.sharing.as_mut() {
                                    sharing.lifetime = at;
                                }
                                cx.notify();
                            }),
                        )
                    })),
            ),
    };
    let readers = div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(label(t!("Who can read")))
        .child(
            div()
                .flex()
                .gap(SPACE_1H)
                .children(READERS.iter().enumerate().map(|(at, &(words, anyone))| {
                    pill(("share-readers", at), words, anyone == sharing.anyone, &t).clicks(
                        cx.listener(move |bench, _, window, cx| {
                            let Some(sharing) = bench.overlays.sharing.as_mut() else {
                                return;
                            };
                            if sharing.anyone == anyone || sharing.working {
                                return;
                            }
                            sharing.anyone = anyone;
                            let agent = sharing.agent.clone();
                            if bench.share_of(&agent).is_some() {
                                bench.start_share(window, cx);
                            }
                            cx.notify();
                        }),
                    )
                })),
        );
    let who = if sharing.anyone {
        t!("Anyone who has the link can read this chat as it happens, from its start, without signing in. They cannot type in it or answer its requests.")
    } else {
        t!("Anyone signed in to Sikemux who has the link can read this chat as it happens, from its start. They cannot type in it or answer its requests.")
    };
    let body = dialog::marked_body()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(dialog::prose(who, &t))
                .child(dialog::prose(t!("It shows everything the agent read and ran, including the contents of files and any secrets in them. The key stays in the link, so our server cannot read it."), &t)),
        )
        .child(div().pt(SPACE_3).child(readers))
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
                let (this, id) = (this.clone(), share.id.clone());
                Rc::new(move |window, cx| {
                    let id = id.clone();
                    this.update(cx, |bench, cx| bench.stop_share(id, window, cx))
                })
            };
            let foot = dialog::hinted_foot(dialog::escape(t!("close"), cx), cx)
                .child(dialog::button(
                    "share-stop",
                    t!("Stop sharing").into(),
                    None,
                    stop,
                    cx,
                ))
                .child(dialog::button(
                    "share-done",
                    t!("Done").into(),
                    Some(t.accent),
                    close.clone(),
                    cx,
                ));
            (t!("Shared read-only"), foot, close.clone())
        }
        None => {
            let create: dialog::Answer = Rc::new(move |window, cx| {
                this.update(cx, |bench, cx| bench.start_share(window, cx))
            });
            let label = if sharing.working {
                t!("Creating link…")
            } else {
                t!("Create link")
            };
            let foot = dialog::hinted_foot(dialog::escape(t!("close"), cx), cx)
                .child(dialog::button(
                    "share-cancel",
                    t!("Cancel").into(),
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
            (t!("Share this chat read-only"), foot, create)
        }
    };
    Some(
        dialog::marked_sheet(Icon::Share, title.into(), body, foot, yes, close, cx)
            .into_any_element(),
    )
}

/// One choice in a row of the dialog's pills, lit while `on`.
fn pill(
    id: (&'static str, usize),
    words: &'static str,
    on: bool,
    t: &Tokens,
) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(SPACE_3)
        .flex()
        .items_center()
        .rounded(theme::radius_2())
        .border(crate::theme::border())
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_SM))
        .cursor_pointer()
        .map(|pill| match on {
            true => pill.bg(t.surface_chosen).text_color(t.text_primary),
            false => {
                let (primary, strong) = (t.text_primary, t.border_strong);
                pill.bg(t.surface_raised)
                    .text_color(t.text_secondary)
                    .hover(move |pill| pill.text_color(primary).border_color(strong))
            }
        })
        .child(crate::i18n::tr(words))
}
