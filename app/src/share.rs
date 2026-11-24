//! Sharing a chat read-only, or a page an agent showed: a link its holders read it from, with or
//! without signing in to Sikemux. The core opens and keeps the share; the window lists them and asks.

use crate::ui::Clicks;
use sikemux_calls::harness::{PageParams, PageShare, SharePageParams};
use sikemux_calls::share::StopShareParams;
use std::future::Future;
use std::pin::Pin;
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
const PAGE_LIFETIMES: [(&str, u64); 4] = [
    LIFETIMES[0],
    LIFETIMES[1],
    LIFETIMES[2],
    (n!("30 days"), 30 * 24 * HOUR_MS),
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
            shared: Shared::Chat(agent.to_owned()),
            lifetime: 1,
            anyone,
            working: false,
            problem: None,
        });
        cx.notify();
    }

    /// Opens the dialog that shares page `id`, or shows the link it has.
    pub(crate) fn open_page_share(&mut self, id: &str, title: &str, cx: &mut Context<Self>) {
        let anyone = (self.pages.share_of(id)).is_none_or(|share| share.anyone);
        self.overlays.sharing = Some(Sharing {
            shared: Shared::Page {
                id: id.to_owned(),
                title: title.to_owned(),
            },
            lifetime: 2,
            anyone,
            working: false,
            problem: None,
        });
        cx.notify();
    }

    fn link_of(&self, shared: &Shared) -> Option<Link> {
        match shared {
            Shared::Chat(agent) => self.share_of(agent).map(|share| Link {
                url: share.url.clone(),
                expires_at: share.expires_at,
                watching: Some(watching(share)),
                stop: Shared::Chat(share.id.clone()),
            }),
            Shared::Page { id, title } => self.pages.share_of(id).map(|share| Link {
                url: share.url.clone(),
                expires_at: share.expires_at,
                watching: None,
                stop: Shared::Page {
                    id: id.clone(),
                    title: title.clone(),
                },
            }),
        }
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
        let Some(shared) = (self.overlays.sharing.as_ref())
            .filter(|sharing| !sharing.working)
            .map(|sharing| sharing.shared.clone())
        else {
            return;
        };
        let fresh = self.link_of(&shared).is_none();
        let Some(sharing) = self.overlays.sharing.as_ref() else {
            return;
        };
        let Some(&(_, lifetime_ms)) = sharing.lifetimes().get(sharing.lifetime) else {
            return;
        };
        let anyone = sharing.anyone;
        type Started = Pin<Box<dyn Future<Output = Result<(String, Option<PageShare>), String>>>>;
        let started: Started = match &shared {
            Shared::Chat(agent) => {
                let asked = crate::core::Target::here(&self.asks).request_typed::<call::ShareChat>(
                    ShareChatParams {
                        agent_id: agent.clone(),
                        lifetime_ms,
                        title: Some(self.agent_title(agent)),
                        anyone,
                    },
                );
                Box::pin(async move { asked.await.map(|share| (share.url, None)) })
            }
            Shared::Page { id, .. } => {
                let asked = self
                    .target()
                    .request_typed::<call::SharePage>(SharePageParams {
                        id: id.clone(),
                        lifetime_ms,
                        anyone,
                        theme: self.desks.read(cx).page_files.theme(),
                    });
                Box::pin(async move { asked.await.map(|share| (share.url.clone(), Some(share))) })
            }
        };
        let Some(sharing) = self.overlays.sharing.as_mut() else {
            return;
        };
        (sharing.working, sharing.problem) = (true, None);
        cx.notify();
        cx.spawn_in(window, async move |bench, cx| {
            let answer = started.await;
            bench
                .update_in(cx, |bench, window, cx| {
                    let url = answer.map(|(url, page)| {
                        if let (Shared::Page { id, .. }, Some(page)) = (&shared, page) {
                            bench.pages.shared_now(id, Some(page));
                        }
                        url
                    });
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

    fn stop_share(&mut self, share: Shared, window: &mut Window, cx: &mut Context<Self>) {
        let stopped: Pin<Box<dyn Future<Output = Result<(), String>>>> = match &share {
            Shared::Chat(id) => {
                let asked = crate::core::Target::here(&self.asks)
                    .request_typed::<call::StopShare>(StopShareParams { id: id.clone() });
                Box::pin(async move { asked.await.map(drop) })
            }
            Shared::Page { id, .. } => Box::pin(
                self.target()
                    .request_typed::<call::StopPageShare>(PageParams::new(id)),
            ),
        };
        if let Some(sharing) = self.overlays.sharing.as_mut() {
            sharing.working = true;
        }
        cx.notify();
        cx.spawn_in(window, async move |bench, cx| {
            let answer = stopped.await;
            bench
                .update(cx, |bench, cx| {
                    match answer {
                        Ok(()) => {
                            if let Shared::Page { id, .. } = &share {
                                bench.pages.shared_now(id, None);
                            }
                            bench.overlays.sharing = None;
                        }
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

/// What the share dialog shares: an agent's chat, or a page an agent showed. As a link to stop,
/// the chat's share by its id.
#[derive(Clone)]
pub enum Shared {
    Chat(String),
    Page { id: String, title: String },
}

/// A link the dialog shows, and what stopping it ends.
struct Link {
    url: String,
    expires_at: u64,
    watching: Option<String>,
    stop: Shared,
}

/// The share dialog while it is open: what it shares, the lifetime picked from its lifetimes by
/// place, who may read it, and how the last ask went.
pub struct Sharing {
    shared: Shared,
    lifetime: usize,
    anyone: bool,
    working: bool,
    problem: Option<String>,
}

impl Sharing {
    fn lifetimes(&self) -> &'static [(&'static str, u64)] {
        match self.shared {
            Shared::Chat(_) => &LIFETIMES,
            Shared::Page { .. } => &PAGE_LIFETIMES,
        }
    }
}

/// The share dialog, as the Mac's sheet draws it: what sharing shows, then how long the link
/// works or the link itself, and the buttons on the foot.
pub fn view(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let sharing = bench.overlays.sharing.as_ref()?;
    let t = Tokens::get(cx).clone();
    let share = bench.link_of(&sharing.shared);
    let page = matches!(sharing.shared, Shared::Page { .. });
    let problem = sharing.problem.clone().or_else(|| match &sharing.shared {
        Shared::Chat(agent) => bench.refusal_of(agent),
        Shared::Page { .. } => None,
    });
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
                        match &share.watching {
                            Some(watching) => t!(
                                "{watching} · ends in {left}",
                                watching = watching,
                                left = crate::ui::time::left(share.expires_at)
                            ),
                            None => t!(
                                "Ends in {left}",
                                left = crate::ui::time::left(share.expires_at)
                            ),
                        },
                        TEXT_XS,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_tertiary),
                )
        }
        None => {
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(label(t!("Link works for")))
                .child(
                    div().flex().gap(SPACE_1H).children(
                        sharing
                            .lifetimes()
                            .iter()
                            .enumerate()
                            .map(|(at, (words, _))| {
                                pill(("share-lifetime", at), words, at == sharing.lifetime, &t)
                                    .clicks(cx.listener(move |bench, _, _, cx| {
                                        if let Some(sharing) = bench.overlays.sharing.as_mut() {
                                            sharing.lifetime = at;
                                        }
                                        cx.notify();
                                    }))
                            }),
                    ),
                )
        }
    };
    let readers = div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(label(match page {
            true => t!("Who can view"),
            false => t!("Who can read"),
        }))
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
                            let shared = sharing.shared.clone();
                            if bench.link_of(&shared).is_some() {
                                bench.start_share(window, cx);
                            }
                            cx.notify();
                        }),
                    )
                })),
        );
    let (who, what) = match (page, sharing.anyone) {
        (false, true) => (
            t!("Anyone who has the link can read this chat as it happens, from its start, without signing in. They cannot type in it or answer its requests."),
            t!("It shows everything the agent read and ran, including the contents of files and any secrets in them. The key stays in the link, so our server cannot read it."),
        ),
        (false, false) => (
            t!("Anyone signed in to Sikemux who has the link can read this chat as it happens, from its start. They cannot type in it or answer its requests."),
            t!("It shows everything the agent read and ran, including the contents of files and any secrets in them. The key stays in the link, so our server cannot read it."),
        ),
        (true, true) => (
            t!("Anyone who has the link can view this artifact, without signing in."),
            t!("The link shows the artifact's latest version, and Sikemux keeps the artifact while it is shared. It is sealed on this Mac with the key in the link, so our server cannot read it."),
        ),
        (true, false) => (
            t!("Anyone signed in to Sikemux who has the link can view this artifact."),
            t!("The link shows the artifact's latest version, and Sikemux keeps the artifact while it is shared. It is sealed on this Mac with the key in the link, so our server cannot read it."),
        ),
    };
    let body = dialog::marked_body()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(dialog::prose(who, &t))
                .child(dialog::prose(what, &t)),
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
                let (this, stop) = (this.clone(), share.stop);
                Rc::new(move |window, cx| {
                    let stop = stop.clone();
                    this.update(cx, |bench, cx| bench.stop_share(stop, window, cx))
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
            let title = match page {
                true => t!("Shared"),
                false => t!("Shared read-only"),
            };
            (title, foot, close.clone())
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
            let title = match page {
                true => t!("Share this artifact"),
                false => t!("Share this chat read-only"),
            };
            (title, foot, create)
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
