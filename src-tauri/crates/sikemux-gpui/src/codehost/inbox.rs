//! The signed-in account's notifications on the project's code host, as a page of the Git
//! pane's side column. A pull request or issue of the project's own repository opens in
//! its area; anything else opens on the host.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use serde_json::json;
use sikemux_wire::codehost::Notification;
use sikemux_wire::protocol::ProjectCodeHost;

use super::{ago, answer, call_of, host_label, notice, Area, Read};
use crate::core::Target;
use crate::theme::Tokens;
use crate::toast;
use crate::ui::{self, Button, Toggle, Variant};
use crate::workbench::Workbench;

fn reason_label(reason: &str) -> String {
    match reason {
        "review_requested" => "Review requested".into(),
        "mention" => "Mentioned".into(),
        "team_mention" => "Team mentioned".into(),
        "assign" => "Assigned".into(),
        "author" => "You opened it".into(),
        "comment" => "New comment".into(),
        "ci_activity" => "CI finished".into(),
        "state_change" => "State changed".into(),
        "subscribed" | "manual" => "Subscribed".into(),
        other => other.replace('_', " "),
    }
}

pub struct Inbox {
    core: Target,
    bench: WeakEntity<Workbench>,
    host: ProjectCodeHost,
    account: Option<String>,
    /// Read notifications are listed too.
    all: bool,
    list: Read<Vec<Notification>>,
    busy: bool,
}

impl Inbox {
    pub fn new(
        core: Target,
        bench: WeakEntity<Workbench>,
        host: ProjectCodeHost,
        account: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut inbox = Self {
            core,
            bench,
            host,
            account,
            all: false,
            list: None,
            busy: false,
        };
        inbox.read(cx);
        inbox
    }

    /// Whether it lists `host`'s notifications for `account`.
    pub fn is_for(&self, host: &ProjectCodeHost, account: &Option<String>) -> bool {
        self.host.host == host.host && self.host.repo == host.repo && &self.account == account
    }

    fn call(
        &self,
        method: &'static str,
        more: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, SharedString>> + 'static {
        let answered = self.core.request(
            "plugin",
            call_of(
                &self.host.host,
                &self.host.repo,
                &self.account,
                method,
                more,
            ),
        );
        async move {
            match answered.await {
                Ok(value) => answer(&value),
                Err(error) => Err(error.into()),
            }
        }
    }

    pub fn read(&mut self, cx: &mut Context<Self>) {
        let listed = self.call("inbox", json!({ "all": self.all }));
        cx.spawn(async move |inbox, cx| {
            let listed = listed.await;
            inbox
                .update(cx, |inbox, cx| {
                    inbox.list = Some(listed.and_then(|value| {
                        serde_json::from_value(value).map_err(|error| error.to_string().into())
                    }));
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn open(&mut self, item: &Notification, cx: &mut Context<Self>) {
        if item.unread {
            let read = self.call("markRead", json!({ "id": item.id }));
            cx.spawn(async move |inbox, cx| {
                if read.await.is_ok() {
                    inbox.update(cx, |inbox, cx| inbox.read(cx)).ok();
                }
            })
            .detach();
        }
        let area = match item.kind.as_str() {
            "PullRequest" => Some(Area::Pulls),
            "Issue" => Some(Area::Issues),
            _ => None,
        };
        match (area, item.number) {
            (Some(area), Some(number)) => {
                let repo = item.repo.clone();
                self.bench
                    .update(cx, |bench, cx| bench.open_host_item(area, number, repo, cx))
                    .ok();
            }
            _ => {
                if let Some(url) = &item.url {
                    cx.open_url(url);
                }
            }
        }
    }

    fn read_everything(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        let done = self.call("markAllRead", json!({}));
        cx.spawn(async move |inbox, cx| {
            let done = done.await;
            inbox
                .update(cx, |inbox, cx| {
                    inbox.busy = false;
                    let note = match done {
                        Ok(_) => toast::success("Inbox cleared"),
                        Err(error) => toast::error(format!("Could not clear the inbox: {error}")),
                    };
                    inbox
                        .bench
                        .update(cx, |bench, cx| bench.notify(note, cx))
                        .ok();
                    inbox.read(cx);
                })
                .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for Inbox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let host = host_label(&self.host.host).to_owned();
        let list = match &self.list {
            None => {
                return div()
                    .size_full()
                    .child(notice("Loading notifications", None, cx))
                    .into_any_element()
            }
            Some(Err(error)) => {
                return div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(notice(
                        format!("Could not read your {host} notifications"),
                        Some(error.clone()),
                        cx,
                    ))
                    .child(
                        div().flex().justify_center().child(
                            Button::new("inbox-retry", "Try again", Variant::Outline)
                                .on_click(cx.listener(|inbox, _, _, cx| inbox.read(cx))),
                        ),
                    )
                    .into_any_element()
            }
            Some(Ok(list)) => list,
        };
        let unread = list.iter().filter(|item| item.unread).count();
        let head = div()
            .px(px(12.))
            .py(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(t.border)
            .child(
                Toggle::new("inbox-all", "Include read", self.all).on_click(cx.listener(
                    |inbox, _, _, cx| {
                        inbox.all = !inbox.all;
                        inbox.read(cx);
                        cx.notify();
                    },
                )),
            )
            .child(div().flex_1())
            .child(ui::meta(format!("{unread} unread"), cx))
            .when(unread > 0, |head| {
                head.child(
                    Button::new("inbox-clear", "Mark all read", Variant::Outline)
                        .disabled(self.busy)
                        .on_click(cx.listener(|inbox, _, _, cx| inbox.read_everything(cx))),
                )
            });
        let rows: Vec<AnyElement> = list
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut sub = item.repo.clone();
                if let Some(number) = item.number {
                    sub.push_str(&format!(" · #{number}"));
                }
                sub.push_str(&format!(" · {}", reason_label(&item.reason)));
                let opened = item.clone();
                div()
                    .id(("inbox", index))
                    .px(px(12.))
                    .py(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(move |row| row.bg(t.surface_hover))
                    .on_click(cx.listener(move |inbox, _, _, cx| inbox.open(&opened, cx)))
                    .child(
                        div()
                            .size(px(7.))
                            .flex_none()
                            .rounded_full()
                            .when(item.unread, |dot| dot.bg(t.accent)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(crate::theme::ui_text(12.5))
                                    .text_color(if item.unread {
                                        t.text_primary
                                    } else {
                                        t.text_secondary
                                    })
                                    .child(item.title.clone()),
                            )
                            .child(ui::meta(sub, cx).truncate()),
                    )
                    .child(ui::meta(ago(&item.updated_at), cx))
                    .into_any_element()
            })
            .collect();
        let body = if rows.is_empty() {
            notice("Nothing waiting", Some("No notifications.".into()), cx)
        } else {
            div().flex().flex_col().children(rows).into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("inbox-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }
}
