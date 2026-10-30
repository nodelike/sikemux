//! Slack's pane: a workspace's channels and direct messages, a channel's latest messages, and a
//! thread beside them to read, reply in, open in Slack or hand to an agent.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window,
};
use gpui_transcript::{Markdown, Pen};
use serde_json::json;
use sikemux_plugin_api::SignInStatus;
use sikemux_plugin_slack_views::{
    Channel, ChannelKind, Channels, History, Listed, Message, OpenThread, Signed,
};

use super::{after, listen, Failure, Host, Panel, PluginPane};
use crate::icons::Icon;
use crate::theme::{
    Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_XS_H,
};
use crate::ui;

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.slack",
    surface: "messages",
    pane: super::boxed::<Slack>,
    top_bar: None,
    palettes: &[],
};

/// Where a person makes the Slack app whose token they paste here.
const SLACK_APPS_PAGE: &str = "https://api.slack.com/apps";

/// Messages read as markdown are kept by their text; past this many, all are read again.
const BODIES_AT_MOST: usize = 500;

/// The Slack workspaces signed in to, as the plugin's `status` says.
type Status = SignInStatus<Signed>;

/// The thread open beside the channel, by where it is and its first message.
#[derive(Clone)]
struct Opened {
    workspace: String,
    channel: String,
    ts: String,
}

/// What a field does when its text changes or Enter is pressed in it.
type Typed = fn(&mut Slack, &mut Host, &mut Context<PluginPane>);

#[derive(Default)]
pub struct Slack {
    workspace: Option<String>,
    channel: Option<String>,
    thread: Option<Opened>,
    token: Option<Entity<InputState>>,
    link: Option<Entity<InputState>>,
    filter: Option<Entity<InputState>>,
    reply: Option<Entity<TextareaState>>,
    typing: Vec<Subscription>,
    bodies: HashMap<String, Rc<Markdown>>,
    /// What a call under way is doing, in the words its button shows.
    busy: Option<&'static str>,
}

impl Panel for Slack {
    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let status = match host.status(cx).read::<Status>() {
            Ok(Some(status)) => status,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let Some(workspace) = shown(&status, self.workspace.as_deref()).filter(|_| status.ok)
        else {
            return self.sign_in(&status, window, cx);
        };
        let workspace_id = workspace.id.clone();
        let channels = host
            .view("channelsView", json!({ "workspace": workspace_id }), cx)
            .read::<Channels>();
        let sidebar = self.sidebar(&status, workspace, &channels, window, cx);
        let main = match self.channel.clone() {
            Some(channel) => {
                self.channel_view(host, &workspace_id, &channel, &channels, window, cx)
            }
            None => super::empty(
                "Pick a channel, or paste a link to a message to read its thread.",
                cx,
            ),
        };
        let thread = self
            .thread
            .clone()
            .map(|opened| self.thread_view(host, &opened, window, cx));
        let t = Tokens::get(cx);
        div()
            .size_full()
            .flex()
            .child(sidebar)
            .child(div().flex_1().min_w(px(260.)).h_full().child(main))
            .children(thread.map(|thread| {
                div()
                    .w(px(400.))
                    .flex_none()
                    .h_full()
                    .border_l_1()
                    .border_color(t.border)
                    .child(thread)
            }))
            .into_any_element()
    }
}

/// The workspace the pane shows: the one picked, else the default, else the first.
fn shown<'a>(status: &'a Status, picked: Option<&str>) -> Option<&'a Listed> {
    status
        .details
        .workspaces
        .iter()
        .find(|each| Some(each.id.as_str()) == picked)
        .or_else(|| {
            status
                .details
                .workspaces
                .iter()
                .find(|each| each.is_default)
        })
        .or(status.details.workspaces.first())
}

/// A one-line field, made the first time the pane draws it.
fn field(
    slot: &mut Option<Entity<InputState>>,
    typing: &mut Vec<Subscription>,
    placeholder: &'static str,
    masked: bool,
    (changed, entered): (Option<Typed>, Option<Typed>),
    window: &mut Window,
    cx: &mut Context<PluginPane>,
) -> Entity<InputState> {
    if let Some(input) = slot {
        return input.clone();
    }
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .masked(masked)
    });
    typing.push(
        cx.subscribe(&input, move |pane, _, event: &InputEvent, cx| {
            let typed = match event {
                InputEvent::Change => changed,
                InputEvent::PressEnter { .. } => entered,
                _ => None,
            };
            if let (Some(typed), Some((slack, host))) = (typed, pane.parts::<Slack>()) {
                typed(slack, host, cx);
            }
            cx.notify();
        }),
    );
    *slot = Some(input.clone());
    input
}

fn text_of(input: &Option<Entity<InputState>>, cx: &App) -> String {
    input
        .as_ref()
        .map(|input| input.read(cx).value().trim().to_owned())
        .unwrap_or_default()
}

/// Empties a field once what it held has gone, in the window the pane is in.
fn empty_field(input: Option<Entity<InputState>>, cx: &mut App) {
    if let (Some(input), Some(window)) = (input, cx.active_window()) {
        let _ = window.update(cx, |_, window, cx| {
            input.update(cx, |input, cx| input.set_value("", window, cx))
        });
    }
}

impl Slack {
    /// Signs in with the token pasted; the core lists the plugin as signed in, so the pane
    /// reads the workspaces again.
    fn connect(&mut self, host: &mut Host, cx: &mut Context<PluginPane>) {
        let token = text_of(&self.token, cx);
        if token.is_empty() || self.busy.is_some() {
            return;
        }
        self.busy = Some("Connecting…");
        let signed = host.call("signIn", json!({ "token": token }));
        after(
            signed,
            |slack: &mut Slack, host, signed, cx| {
                slack.busy = None;
                match signed {
                    Ok(_) => empty_field(slack.token.clone(), cx),
                    Err(failure) => host.said(&failure.titled("Connect Slack"), cx),
                }
            },
            cx,
        );
    }

    /// Opens the thread a pasted message link points at, in its own workspace and channel.
    fn open_link(&mut self, host: &mut Host, cx: &mut Context<PluginPane>) {
        let link = text_of(&self.link, cx);
        if link.is_empty() || self.busy.is_some() {
            return;
        }
        self.busy = Some("Opening…");
        let opened = host.call("thread", json!({ "link": link }));
        after(
            opened,
            |slack: &mut Slack, host, opened, cx| {
                slack.busy = None;
                let thread = match opened {
                    Ok(thread) => thread,
                    Err(failure) => return host.said(&failure.titled("Open the link"), cx),
                };
                let field = |name: &str| thread[name].as_str().map(str::to_owned);
                if let (Some(workspace), Some(channel), Some(ts)) =
                    (field("workspace"), field("channel"), field("ts"))
                {
                    slack.workspace = Some(workspace.clone());
                    slack.channel = Some(channel.clone());
                    slack.thread = Some(Opened {
                        workspace,
                        channel,
                        ts,
                    });
                }
                empty_field(slack.link.clone(), cx);
            },
            cx,
        );
    }

    /// Replies in the open thread with what the person wrote; the core reads the thread
    /// and the channel again after the post.
    fn reply(&mut self, host: &mut Host, cx: &mut Context<PluginPane>) {
        let (Some(reply), Some(opened)) = (self.reply.clone(), self.thread.clone()) else {
            return;
        };
        let text = reply.read(cx).value().trim().to_owned();
        if text.is_empty() || self.busy.is_some() {
            return;
        }
        self.busy = Some("Sending…");
        let posted = host.call(
            "post",
            json!({
                "workspace": opened.workspace,
                "channel": opened.channel,
                "threadTs": opened.ts,
                "text": text,
            }),
        );
        after(
            posted,
            move |slack: &mut Slack, host, posted, cx| {
                slack.busy = None;
                match posted {
                    Ok(_) => {
                        if let Some(window) = cx.active_window() {
                            let _ = window.update(cx, |_, window, cx| {
                                reply.update(cx, |reply, cx| reply.set_value("", window, cx))
                            });
                        }
                    }
                    Err(failure) => host.said(&failure.titled("Reply"), cx),
                }
            },
            cx,
        );
    }

    fn sign_out(&mut self, host: &mut Host, workspace: String, cx: &mut Context<PluginPane>) {
        let out = host.call("signOut", json!({ "workspace": workspace }));
        after(
            out,
            |slack: &mut Slack, host, out, cx| match out {
                Ok(_) => {
                    slack.workspace = None;
                    slack.channel = None;
                    slack.thread = None;
                }
                Err(failure) => host.said(&failure.titled("Sign out of Slack"), cx),
            },
            cx,
        );
    }

    fn markdown(&mut self, text: &str) -> Rc<Markdown> {
        if self.bodies.len() > BODIES_AT_MOST {
            self.bodies.clear();
        }
        self.bodies
            .entry(text.to_owned())
            .or_insert_with(|| Rc::new(crate::editor::preview::render(text)))
            .clone()
    }

    fn sign_in(
        &mut self,
        status: &Status,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let connect: Typed = |slack, host, cx| slack.connect(host, cx);
        let token = field(
            &mut self.token,
            &mut self.typing,
            "xoxp-…",
            true,
            (None, Some(connect)),
            window,
            cx,
        );
        let failed = status
            .message
            .clone()
            .filter(|_| !status.details.workspaces.is_empty());
        let ink = Tokens::get(cx).text_primary;
        ui::sign_in::Screen::new("slack-sign-in", ui::picture(Icon::Slack, 26., ink), "Connect Slack")
            .lede("Read a thread beside your code, hand it to an agent, and let the agent reply in it when the work is done.")
            .foot(ui::sign_in::KEPT)
            .child(
                ui::sign_in::Field::new("User OAuth Token", &token)
                    .mono()
                    .hint("Make a Slack app, add the user scopes channels:history, groups:history, im:history, mpim:history, channels:read, groups:read, im:read, mpim:read, chat:write, users:read, users:read.email and search:read, install it to your workspace, and copy the User OAuth Token from OAuth & Permissions."),
            )
            .child(
                ui::Button::new(
                    "slack-connect",
                    self.busy.unwrap_or("Connect"),
                    ui::Variant::Primary,
                )
                .wide()
                .disabled(self.busy.is_some())
                .on_click(listen::<Slack, ClickEvent>(cx, |slack, host, _, _, cx| {
                    slack.connect(host, cx)
                })),
            )
            .children(failed.map(|message| ui::sign_in::error(message, cx)))
            .child(
                ui::sign_in::links().child(
                    ui::sign_in::link("slack-make-app", "Make a Slack app", cx)
                        .on_click(|_, _, cx| cx.open_url(SLACK_APPS_PAGE)),
                ),
            )
            .into_any_element()
    }

    fn sidebar(
        &mut self,
        status: &Status,
        workspace: &Listed,
        channels: &Result<Option<Rc<Channels>>, Failure>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let open: Typed = |slack, host, cx| slack.open_link(host, cx);
        let link = field(
            &mut self.link,
            &mut self.typing,
            "Paste a Slack message link",
            false,
            (None, Some(open)),
            window,
            cx,
        );
        let filter = field(
            &mut self.filter,
            &mut self.typing,
            "Find a channel or person",
            false,
            (None, None),
            window,
            cx,
        );
        let words = filter.read(cx).value().trim().to_lowercase();
        let list = match channels {
            Ok(None) => super::loading(cx),
            Err(failure) if failure.signed_out() => {
                super::empty("Slack signed this workspace out. Sign in again.", cx)
            }
            Err(failure) => super::failure(failure, cx),
            Ok(Some(channels)) => {
                let matches = |channel: &&Channel| {
                    words.is_empty() || channel.name.to_lowercase().contains(&words)
                };
                let rooms: Vec<AnyElement> = channels
                    .rooms
                    .iter()
                    .filter(matches)
                    .map(|channel| self.channel_row(channel, cx))
                    .collect();
                let people: Vec<AnyElement> = channels
                    .people
                    .iter()
                    .filter(matches)
                    .map(|channel| self.channel_row(channel, cx))
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .when(!rooms.is_empty(), |list| {
                        list.child(ui::Group::new("Channels", "").children(rooms))
                    })
                    .when(!people.is_empty(), |list| {
                        list.child(ui::Group::new("Direct messages", "").children(people))
                    })
                    .into_any_element()
            }
        };
        let others = (status.details.workspaces.len() > 1).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .children(status.details.workspaces.iter().map(|each| {
                    let id = each.id.clone();
                    ui::Row::new(
                        SharedString::from(format!("slack-workspace-{}", each.id)),
                        each.name.clone(),
                    )
                    .quiet()
                    .chosen(each.id == workspace.id)
                    .on_click(listen::<Slack, ClickEvent>(
                        cx,
                        move |slack, _, _, _, cx| {
                            slack.workspace = Some(id.clone());
                            slack.channel = None;
                            slack.thread = None;
                            cx.notify();
                        },
                    ))
                }))
        });
        let leaving = workspace.id.clone();
        div()
            .w(px(240.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(t.border)
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .p(SPACE_2)
            .map(|side| match others {
                Some(others) => side.child(others),
                None => side.child(
                    div().px(SPACE_1H).child(
                        ui::text(workspace.name.clone(), 13., FontWeight::SEMIBOLD)
                            .text_color(t.text_primary),
                    ),
                ),
            })
            .child(ui::Field::new(&link).icon(Icon::Link))
            .child(ui::Field::new(&filter).icon(Icon::Search))
            .child(
                div()
                    .id("slack-channels")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                ui::Button::new(
                    "slack-sign-out",
                    format!("Sign out of {}", workspace.name),
                    ui::Variant::Outline,
                )
                .icon(Icon::LogOut)
                .on_click(listen::<Slack, ClickEvent>(
                    cx,
                    move |slack, host, _, _, cx| slack.sign_out(host, leaving.clone(), cx),
                )),
            )
            .into_any_element()
    }

    fn channel_row(&self, channel: &Channel, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let id = channel.id.clone();
        ui::Row::new(
            SharedString::from(format!("slack-channel-{}", channel.id)),
            channel.name.clone(),
        )
        .mark(
            16.,
            ui::text(mark(channel.kind), 12., FontWeight::MEDIUM).text_color(t.text_tertiary),
        )
        .quiet()
        .chosen(self.channel.as_ref() == Some(&channel.id))
        .on_click(listen::<Slack, ClickEvent>(
            cx,
            move |slack, _, _, _, cx| {
                slack.channel = Some(id.clone());
                slack.thread = None;
                cx.notify();
            },
        ))
        .into_any_element()
    }

    fn channel_view(
        &mut self,
        host: &mut Host,
        workspace: &str,
        channel: &str,
        channels: &Result<Option<Rc<Channels>>, Failure>,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let title = channels
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .and_then(|channels| {
                channels
                    .rooms
                    .iter()
                    .chain(&channels.people)
                    .find(|each| each.id == channel)
            })
            .map_or_else(
                || channel.to_owned(),
                |found| match found.kind {
                    ChannelKind::Channel => format!("#{}", found.name),
                    _ => found.name.clone(),
                },
            );
        let history = host
            .view(
                "historyView",
                json!({ "workspace": workspace, "channel": channel }),
                cx,
            )
            .read::<History>();
        let open = self.thread.as_ref().map(|opened| opened.ts.clone());
        let body = match history {
            Ok(None) => super::loading(cx),
            Err(failure) => super::failure(&failure, cx),
            Ok(Some(history)) if history.messages.is_empty() => {
                super::empty("No messages here yet.", cx)
            }
            Ok(Some(history)) => {
                let rows: Vec<AnyElement> = history
                    .messages
                    .iter()
                    .map(|message| {
                        let root = message.thread_ts.as_ref().unwrap_or(&message.ts);
                        let opened = open.as_ref() == Some(root);
                        let place = (workspace, channel);
                        self.message(message, Some(place), opened, window, cx)
                    })
                    .collect();
                div()
                    .id("slack-messages")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .p(SPACE_2H)
                    .children(rows)
                    .into_any_element()
            }
        };
        let refresh = ui::IconButton::new("slack-refresh", Icon::Refresh, "Read the channel again")
            .on_click(listen::<Slack, ClickEvent>(cx, |_, host, _, _, _| {
                host.read_again(Some("historyView"))
            }))
            .into_any_element();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head(title, vec![refresh], cx))
            .child(body)
            .into_any_element()
    }

    fn thread_view(
        &mut self,
        host: &mut Host,
        opened: &Opened,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let open = host
            .view(
                "threadView",
                json!({ "workspace": opened.workspace, "channel": opened.channel, "ts": opened.ts }),
                cx,
            )
            .read::<OpenThread>();
        let mut actions = Vec::new();
        if let Ok(Some(open)) = &open {
            actions.push(super::send_to_agent(
                "slack-send",
                host,
                open.for_agent.clone(),
                cx,
            ));
            if let Some(link) = open.thread.permalink.clone() {
                let copied = link.clone();
                actions.push(
                    ui::IconButton::new("slack-copy-link", Icon::Copy, "Copy the link")
                        .on_click(listen::<Slack, ClickEvent>(cx, move |_, host, _, _, cx| {
                            host.copy(copied.clone(), "the link", cx)
                        }))
                        .into_any_element(),
                );
                actions.push(
                    ui::IconButton::new("slack-open", Icon::Globe, "Open in Slack")
                        .on_click(move |_, _, cx| cx.open_url(&link))
                        .into_any_element(),
                );
            }
        }
        actions.push(
            ui::IconButton::new("slack-close-thread", Icon::Close, "Close the thread")
                .on_click(listen::<Slack, ClickEvent>(cx, |slack, _, _, _, cx| {
                    slack.thread = None;
                    cx.notify();
                }))
                .into_any_element(),
        );
        let body = match open {
            Ok(None) => super::loading(cx),
            Err(failure) => super::failure(&failure, cx),
            Ok(Some(open)) => {
                let rows: Vec<AnyElement> = open
                    .thread
                    .messages
                    .iter()
                    .map(|message| self.message(message, None, false, window, cx))
                    .collect();
                div()
                    .id("slack-thread")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .p(SPACE_2H)
                    .children(rows)
                    .into_any_element()
            }
        };
        let composer = self.composer(window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head("Thread".to_owned(), actions, cx))
            .child(body)
            .child(composer)
            .into_any_element()
    }

    fn composer(&mut self, window: &mut Window, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let reply = match &self.reply {
            Some(reply) => reply.clone(),
            None => {
                let reply = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(2, 8)
                        .placeholder("Reply in the thread (⌘↵ to send)")
                });
                self.typing.push(
                    cx.subscribe(&reply, move |pane, _, event: &InputEvent, cx| {
                        let send = matches!(
                            event,
                            InputEvent::PressEnter {
                                secondary: true,
                                ..
                            }
                        );
                        if let (true, Some((slack, host))) = (send, pane.parts::<Slack>()) {
                            slack.reply(host, cx);
                        }
                        cx.notify();
                    }),
                );
                self.reply = Some(reply.clone());
                reply
            }
        };
        let written = !reply.read(cx).value().trim().is_empty();
        div()
            .m(SPACE_2H)
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .child(
                div()
                    .px(SPACE_2H)
                    .py(SPACE_2)
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .child(
                        Textarea::new(&reply)
                            .appearance(false)
                            .text_size(crate::theme::ui_text(TEXT_XS_H)),
                    ),
            )
            .child(
                div()
                    .px(SPACE_2)
                    .py(SPACE_1H)
                    .flex()
                    .justify_end()
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        ui::Button::new(
                            "slack-reply",
                            self.busy.unwrap_or("Reply"),
                            ui::Variant::Primary,
                        )
                        .disabled(self.busy.is_some() || !written)
                        .on_click(listen::<Slack, ClickEvent>(cx, |slack, host, _, _, cx| {
                            slack.reply(host, cx)
                        })),
                    ),
            )
            .into_any_element()
    }

    fn message(
        &mut self,
        message: &Message,
        place: Option<(&str, &str)>,
        opened: bool,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let body = self.markdown(&message.text);
        let pen = Pen::new(Rc::new(crate::editor::preview::reading_style(13., cx)));
        let initial: String = message
            .name
            .chars()
            .take(1)
            .flat_map(char::to_uppercase)
            .collect();
        let replies = place
            .filter(|_| message.reply_count > 0)
            .map(|(workspace, channel)| {
                let thread = Opened {
                    workspace: workspace.to_owned(),
                    channel: channel.to_owned(),
                    ts: message
                        .thread_ts
                        .clone()
                        .unwrap_or_else(|| message.ts.clone()),
                };
                let words = match message.reply_count {
                    1 => "1 reply".to_owned(),
                    count => format!("{count} replies"),
                };
                ui::Button::new(
                    SharedString::from(format!("slack-replies-{}", message.ts)),
                    words,
                    ui::Variant::Outline,
                )
                .on_click(listen::<Slack, ClickEvent>(
                    cx,
                    move |slack, _, _, _, cx| {
                        slack.thread = Some(thread.clone());
                        cx.notify();
                    },
                ))
            });
        div()
            .flex()
            .gap(SPACE_2H)
            .px(SPACE_2)
            .py(SPACE_1H)
            .rounded(RADIUS_2)
            .when(opened, |row| row.bg(t.surface_chosen))
            .child(
                div()
                    .size(px(24.))
                    .flex_none()
                    .rounded_full()
                    .bg(t.surface_raised)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        ui::text(initial, 11., FontWeight::SEMIBOLD).text_color(t.text_secondary),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .child(
                                ui::text(message.name.clone(), 12.5, FontWeight::SEMIBOLD)
                                    .text_color(t.text_primary),
                            )
                            .when(message.bot, |head| head.child(ui::meta("app", cx)))
                            .child(ui::meta(stamp(&message.at), cx)),
                    )
                    .child(pen.markdown(
                        SharedString::from(format!("slack-text-{}", message.ts)),
                        &body,
                        window,
                        cx,
                    ))
                    .when(!message.files.is_empty(), |text| {
                        text.child(ui::meta(
                            format!("Attached: {}", message.files.join(", ")),
                            cx,
                        ))
                    })
                    .children(replies),
            )
            .into_any_element()
    }
}

fn mark(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Channel => "#",
        ChannelKind::Private => "🔒",
        ChannelKind::Dm => "●",
        ChannelKind::Group => "◍",
    }
}

/// When a message was written, as the person's day and minute.
fn stamp(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%b %-d, %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| at.to_owned())
}

fn head(title: String, actions: Vec<AnyElement>, cx: &Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .h(px(40.))
        .flex_none()
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_1)
        .border_b_1()
        .border_color(t.border)
        .child(ui::text(title, 13., FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(div().flex_1())
        .children(actions)
        .into_any_element()
}
