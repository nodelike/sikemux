//! The files a message carries: picked, dropped on the chat or pasted, shown as tiles over
//! the draft, each picture also named in the draft where it belongs, as `[Image #n]`.

use std::path::{Path, PathBuf};

use gpui_kit::base::input::{InlineToken, InlineTokenContext};
use gpui_kit::component::attachment::{
    Attachment, AttachmentContent, AttachmentMedia, AttachmentTitle,
};
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, App, AppContext, Axis, ClipboardEntry, ClipboardItem, Context, Div, ExternalPaths,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    PathPromptOptions, SharedString, Styled, WeakEntity, Window,
};
use gpui_transcript::FilePress;
use sikemux_core::acp::MAX_ATTACHMENTS;
use sikemux_wire::chat::ChatMessage;
use sikemux_wire::protocol::ChatContext;
use tokio::sync::oneshot;

use super::Composing;
use crate::core::ChatAsk;
use crate::drag::DraggedPath;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

const IMAGE_EXTENSIONS: [&str; 13] = [
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif", "tif", "tiff", "heic", "heif",
];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// macOS deletes the picture a screenshot's thumbnail drags in once the drop is over.
fn is_screenshot_thumbnail(path: &Path) -> bool {
    path.to_string_lossy()
        .contains("/TemporaryItems/NSIRD_screencaptureui_")
}

const IMAGE: &str = "image:";

/// The largest file sent to another host for a chat there.
const MAX_KEPT: u64 = 50_000_000;

/// The glyph a tag in the draft wears: a picture's, a named agent's face or mark, or a file's
/// or folder's.
fn mark(id: &str, tint: Hsla, bench: &WeakEntity<Workbench>, cx: &App) -> gpui_kit::AnyElement {
    if id.starts_with(IMAGE) {
        return ui::icon(Icon::Image, 12.)
            .text_color(tint)
            .into_any_element();
    }
    let named =
        (id.strip_prefix("agent:")).and_then(|agent| bench.upgrade()?.read(cx).agent_by_id(agent));
    if let Some(agent) = named {
        return ui::agent_mark(&agent, 12., cx);
    }
    if id.starts_with("agent:") || id.starts_with("earlier:") {
        return ui::icon(Icon::Agent, 12.)
            .text_color(tint)
            .into_any_element();
    }
    if let Some(kind) = id.strip_prefix("tracked:") {
        let icon = if kind.starts_with("pull") {
            Icon::PullRequest
        } else {
            Icon::Issue
        };
        return ui::icon(icon, 12.).text_color(tint).into_any_element();
    }
    if id.starts_with("folder:") {
        return ui::icon(Icon::Folder, 12.)
            .text_color(tint)
            .into_any_element();
    }
    let path = Path::new(id.strip_prefix("file:").unwrap_or(id));
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    crate::file_icons::icon(&name, cx).map_or_else(
        || {
            ui::icon(Icon::File, 12.)
                .text_color(tint)
                .into_any_element()
        },
        |icon| gpui_kit::img(icon).size(px(12.)).into_any_element(),
    )
}

fn image_tag(number: usize, path: &Path) -> InlineToken {
    InlineToken::new(
        format!("{IMAGE}{}", path.to_string_lossy()),
        format!("[Image #{number}]"),
    )
    .with_label(format!("Image #{number}"))
}

impl Composing {
    /// Adds the files not already going, up to as many as a message carries, and names each
    /// new picture at the caret.
    fn attach(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut tags = Vec::new();
        for path in paths {
            if path.as_os_str().is_empty() || self.attachments.contains(&path) {
                continue;
            }
            if self.attachments.len() == MAX_ATTACHMENTS {
                break;
            }
            self.attachments.push(path);
            let path = &self.attachments[self.attachments.len() - 1];
            if is_image(path) {
                tags.push(image_tag(self.attachments.len(), path));
            }
        }
        if tags.is_empty() {
            return;
        }
        self.draft.update(cx, |draft, cx| {
            let range = draft.selected_range();
            let value = draft.value();
            let spaced_before = value[..range.start]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace);
            let spaced_after = value[range.end..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace);
            if !spaced_before {
                draft.replace(" ", window, cx);
            }
            for (index, tag) in tags.into_iter().enumerate() {
                if index > 0 {
                    draft.replace(" ", window, cx);
                }
                if let Err(error) = draft.replace_with_token(tag, window, cx) {
                    eprintln!("sikemux: a picture's tag did not go into the draft: {error}");
                }
            }
            if !spaced_after {
                draft.replace(" ", window, cx);
            }
        });
    }

    /// Takes attachment `index` off the message with its tags, and counts the later pictures
    /// down so their tags still name them.
    fn detach(&mut self, index: usize, window: &mut Window, cx: &mut App) {
        if index >= self.attachments.len() {
            return;
        }
        let gone = self
            .attachments
            .remove(index)
            .to_string_lossy()
            .into_owned();
        let attachments = &self.attachments;
        self.draft.update(cx, |draft, cx| {
            let spans = draft.tokens().to_vec();
            for span in spans.iter().rev() {
                let Some(id) = span.token().id().strip_prefix(IMAGE) else {
                    continue;
                };
                if id == gone {
                    let range = span.range();
                    let start = if draft.value()[..range.start].ends_with(' ') {
                        range.start - 1
                    } else {
                        range.start
                    };
                    draft.set_selected_range(start..range.end, cx);
                    draft.replace("", window, cx);
                } else if let Some(at) = attachments
                    .iter()
                    .position(|path| path.to_string_lossy() == id)
                    .filter(|at| *at >= index)
                {
                    if let Err(error) = draft.replace_range_with_token(
                        span.range(),
                        image_tag(at + 1, &attachments[at]),
                        window,
                        cx,
                    ) {
                        eprintln!("sikemux: a picture's tag was not renumbered: {error}");
                    }
                }
            }
        });
    }
}

impl Workbench {
    /// Hands the next message of `agent`'s chat files the person chose on this host. A chat
    /// on another host cannot read them where they are, so they go to its core to keep, as a
    /// screenshot's thumbnail does, which is gone once the drop is over.
    pub fn attach(
        &mut self,
        agent: &str,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let elsewhere = *self.chat_target(agent).place() != Place::This;
        let (keep, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths
            .into_iter()
            .partition(|path| elsewhere || is_screenshot_thumbnail(path));
        if !keep.is_empty() {
            self.keep_files(agent, keep, window, cx);
        }
        self.attach_there(agent, paths, window, cx);
    }

    /// Hands the next message of `agent`'s chat files that are already where the agent runs.
    fn attach_there(
        &mut self,
        agent: &str,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let Some(composing) = self
            .chats
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        else {
            return;
        };
        composing.attach(paths, window, cx);
        self.typed_draft(agent, cx);
        cx.notify();
    }

    /// Reads files off the window's thread and has the chat's core keep each, attaching it
    /// once the core says where it went; one too large to send is said so.
    fn keep_files(
        &self,
        agent: &str,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = agent.to_owned();
        let read = cx.background_spawn(async move {
            paths
                .into_iter()
                .map(|path| {
                    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                    let bytes = (size <= MAX_KEPT).then(|| std::fs::read(&path));
                    (path, bytes)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            for (path, bytes) in read.await {
                let name = path.file_name().map_or_else(
                    || "screenshot.png".to_owned(),
                    |name| name.to_string_lossy().into_owned(),
                );
                match bytes {
                    Some(Ok(bytes)) => {
                        let mime = if is_screenshot_thumbnail(&path) || is_image(&path) {
                            "image/png"
                        } else {
                            "application/octet-stream"
                        };
                        this.update_in(cx, |bench, window, cx| {
                            bench.keep(&agent, name, mime, bytes, window, cx)
                        })
                        .ok();
                    }
                    Some(Err(error)) => {
                        eprintln!("sikemux: {} does not read: {error}", path.display())
                    }
                    None => {
                        let note = format!(
                            "{name} is over {} MB, too large to send to the host the chat runs on",
                            MAX_KEPT / 1_000_000
                        );
                        cx.update(|window, cx| window.push_notification(note, cx))
                            .ok();
                    }
                }
            }
        })
        .detach();
    }

    /// Asks the core to keep a file that lives nowhere the agent can read, and attaches it
    /// once the core says where it went.
    fn keep(
        &self,
        agent: &str,
        name: String,
        mime: &str,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (kept, answer) = oneshot::channel();
        let ask = ChatAsk::Keep {
            agent: agent.to_owned(),
            name,
            mime: mime.to_owned(),
            bytes,
            kept,
        };
        self.ask_chat(ask);
        let agent = agent.to_owned();
        cx.spawn_in(window, async move |this, cx| match answer.await {
            Ok(Ok(path)) => {
                this.update_in(cx, |bench, window, cx| {
                    bench.attach_there(&agent, vec![path], window, cx)
                })
                .ok();
            }
            Ok(Err(error)) => eprintln!("sikemux: the core did not keep a pasted file: {error}"),
            Err(_) => {}
        })
        .detach();
    }

    /// Takes context item `index`, an earlier chat or an issue, off the message.
    pub fn forget(&mut self, agent: &str, index: usize, cx: &mut Context<Self>) {
        if let Some(composing) = self
            .chats
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        {
            if index < composing.context.len() {
                composing.context.remove(index);
                self.typed_draft(agent, cx);
                cx.notify();
            }
        }
    }

    pub fn detach(
        &mut self,
        agent: &str,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(composing) = self
            .chats
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        {
            composing.detach(index, window, cx);
            self.typed_draft(agent, cx);
            cx.notify();
        }
    }

    /// The pictures and files pasted into the draft of `agent`'s chat; whether there were any.
    fn pasted(
        &mut self,
        agent: &str,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut taken = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    let stamp = chrono::Local::now().format("pasted-%Y%m%d-%H%M%S%3f");
                    let name = format!("{stamp}.{}", image.format.extension());
                    self.keep(
                        agent,
                        name,
                        image.format.mime_type(),
                        image.bytes.clone(),
                        window,
                        cx,
                    );
                    taken = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    self.attach(agent, paths.paths().to_vec(), window, cx);
                    taken = true;
                }
                ClipboardEntry::String(_) => {}
            }
        }
        taken
    }

    /// Opens the system's picker of files, for the next message of `agent`'s chat.
    pub fn add_files(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        let agent = agent.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update_in(cx, |bench, window, cx| {
                    bench.attach(&agent, paths, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }
}

/// What the draft takes in place of its paste, for pictures and files on the clipboard.
pub fn on_paste(
    agent: &str,
    cx: &mut Context<Workbench>,
) -> impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool + use<> {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    move |item: &ClipboardItem, window: &mut Window, cx: &mut App| {
        this.update(cx, |bench, cx| bench.pasted(&agent, item, window, cx))
            .unwrap_or(false)
    }
}

/// Files dropped on a chat, from Finder or from the file tree, go with the next message.
pub fn takes_drops(pane: Div, agent: &str, cx: &mut Context<Workbench>) -> Div {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    let (from_tree, from_tree_agent) = (this.clone(), agent.clone());
    pane.on_drop(move |dropped: &ExternalPaths, window, cx| {
        this.update(cx, |bench, cx| {
            bench.attach(&agent, dropped.paths().to_vec(), window, cx)
        })
        .ok();
    })
    .on_drop(move |dropped: &DraggedPath, window, cx| {
        from_tree
            .update(cx, |bench, cx| {
                bench.attach(&from_tree_agent, vec![dropped.path.clone()], window, cx)
            })
            .ok();
    })
}

/// What shows over a chat while files are dragged over it.
pub fn drop_hint(cx: &App) -> Div {
    let t = Tokens::get(cx).clone();
    div()
        .absolute()
        .inset(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_dashed()
        .border_color(t.border_selected)
        .rounded(theme::RADIUS_4)
        .bg(Hsla {
            a: 0.86,
            ..t.surface_sunken
        })
        .invisible()
        .drag_over::<ExternalPaths>(|style, _, _, _| style.visible())
        .drag_over::<DraggedPath>(|style, _, _, _| style.visible())
        .child(
            ui::text(
                "Drop files or folders into this session",
                13.,
                FontWeight::MEDIUM,
            )
            .text_color(t.text_primary),
        )
}

/// A tag in the draft, set on the line like a word of it.
/// A tag in the draft as a chip, with the glyph `mark` gives it.
pub fn chip(
    bench: WeakEntity<Workbench>,
) -> impl Fn(&InlineTokenContext, &mut Window, &mut App) -> gpui_kit::AnyElement {
    move |token, _, cx| drawn_chip(token, &bench, cx).into_any_element()
}

fn drawn_chip(
    token: &InlineTokenContext,
    bench: &WeakEntity<Workbench>,
    cx: &mut App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let selected = token.is_selected();
    div().h(token.line_height()).flex().items_center().child(
        div()
            .h(px(20.))
            .max_w(token.available_width().min(px(260.)))
            .mx(px(1.))
            .pl(px(4.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .border_1()
            .border_color(if selected {
                Hsla {
                    a: 0.3,
                    ..t.text_primary
                }
            } else {
                t.line
            })
            .rounded(theme::RADIUS_1)
            .bg(if selected {
                t.surface_chosen
            } else {
                Hsla {
                    a: 0.06,
                    ..t.text_primary
                }
            })
            .text_size(crate::theme::ui_text(12.))
            .text_color(t.text_primary)
            .child(mark(token.token().id(), t.text_tertiary, bench, cx))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(token.token().label().clone()),
            ),
    )
}

/// The files going with the message, as tiles: a picture's preview or a file's icon, its
/// number, and a way to take it off; then the chats and issues it carries, as chips.
pub fn tiles(
    attachments: &[PathBuf],
    context: &[ChatContext],
    remove: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    forget: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    open: impl Fn(PathBuf, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Option<impl IntoElement> {
    if attachments.is_empty() && context.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let tiles = attachments.iter().enumerate().map(|(index, path)| {
        let number = badge(index + 1, &t, cx);
        let remove = remove.clone();
        let opens = is_image(path).then(|| {
            let (open, path) = (open.clone(), path.clone());
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                open(path.clone(), window, cx)
            }
        });
        Attachment::new()
            .id(SharedString::from(format!("attachment-{index}")))
            .when_some(opens, |tile, opens| tile.on_click(opens))
            .axis(Axis::Vertical)
            .with_size(gpui_kit::component::Size::Size(px(16.6)))
            .tooltip(path.to_string_lossy().into_owned())
            .media(media(path, &t, cx).overlay(number))
            .on_remove(move |_, window, cx| remove(index, window, cx))
    });
    let chips = context.iter().enumerate().map(|(index, item)| {
        let forget = forget.clone();
        context_chip(format!("context-{index}"), &item.uri, &item.title, &t)
            .on_remove(move |_, window, cx| forget(index, window, cx))
    });
    Some(
        div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap(px(7.))
            .px(px(6.))
            .pt(px(6.))
            .children(tiles)
            .children(chips),
    )
}

fn file_name(path: &Path) -> SharedString {
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy())
        .into_owned()
        .into()
}

/// A picture's preview, or the icon of a file's kind.
fn media(path: &Path, t: &Tokens, cx: &App) -> AttachmentMedia {
    let media = if is_image(path) {
        AttachmentMedia::new()
            .src(crate::picture::Picture::File(path.to_path_buf()).source_at(px(96.)))
    } else {
        let icon = crate::file_icons::icon(&file_name(path), cx)
            .map(|icon| gpui_kit::img(icon).size(px(28.)).into_any_element());
        AttachmentMedia::new().child(icon.unwrap_or_else(|| {
            ui::icon(Icon::File, 26.)
                .text_color(t.text_tertiary)
                .into_any_element()
        }))
    };
    media.bg(Hsla {
        a: 0.48,
        ..t.surface_sunken
    })
}

/// The number a picture goes by in the text, `#n`, over the corner of its tile.
fn badge(number: usize, t: &Tokens, cx: &App) -> Div {
    div().size_full().relative().child(
        div()
            .absolute()
            .left(px(3.))
            .bottom(px(3.))
            .px(px(4.))
            .py(px(2.))
            .rounded(theme::RADIUS_1)
            .bg(Hsla {
                a: 0.72,
                ..t.surface_sunken
            })
            .child(ui::mono(format!("#{number}"), 9.5, cx).text_color(t.text_primary)),
    )
}

/// An issue's or pull request's number in its link, when the link is to one.
fn tracked(uri: &str) -> Option<(Icon, &str)> {
    let kinds = [
        ("/issues/", Icon::Issue),
        ("/pull/", Icon::PullRequest),
        ("/pulls/", Icon::PullRequest),
        ("/pull-requests/", Icon::PullRequest),
        ("/merge_requests/", Icon::PullRequest),
    ];
    kinds.into_iter().find_map(|(kind, icon)| {
        let (_, after) = uri.split_once(kind)?;
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let number = &after[..end];
        (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some((icon, number))
    })
}

/// A context item's chip: `#12 Title` for an issue or pull request, its title for an
/// earlier chat or anything else.
fn context_chip(id: String, uri: &str, title: &str, t: &Tokens) -> Attachment {
    let (icon, title) = match tracked(uri) {
        Some((icon, number)) => {
            let bare = title.trim_start_matches("Issue ");
            let bare = bare.trim_start_matches("Pull request ");
            let bare = bare
                .strip_prefix(&format!("#{number}"))
                .map_or(title, |rest| rest.trim_start_matches(':').trim_start());
            (icon, format!("#{number} {bare}").trim_end().to_owned())
        }
        None if uri.starts_with("sikemux://earlier/") => (Icon::Message, title.to_owned()),
        None if title.is_empty() => (Icon::File, uri.to_owned()),
        None => (Icon::File, title.to_owned()),
    };
    Attachment::new()
        .id(SharedString::from(id))
        .with_size(gpui_kit::component::Size::Small)
        .tooltip(uri.to_owned())
        .media(AttachmentMedia::new().child(ui::icon(icon, 13.).text_color(t.text_tertiary)))
        .content(AttachmentContent::new().title(AttachmentTitle::new(title)))
}

/// What a sent message carried, inside its bubble over the text: its pictures, which open
/// over the window, its files by name, and the issues and pages sent with it.
pub fn sent(
    message: &ChatMessage,
    open: impl Fn(PathBuf, &mut Window, &mut App) + Clone + 'static,
    file: super::files::FilePressed,
    cx: &App,
) -> Option<Div> {
    if message.attachments.is_empty() && message.context.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let written = super::edit::written(message);
    let files = message.attachments.iter().enumerate().map(|(index, path)| {
        let path = PathBuf::from(path);
        let id = SharedString::from(format!("sent-{}-{index}", message.id));
        if !is_image(&path) {
            let (name, menu_name) = (
                path.to_string_lossy().into_owned(),
                path.to_string_lossy().into_owned(),
            );
            let (file, menu_file) = (file.clone(), file.clone());
            return div()
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    let press = if event.click_count > 1 {
                        FilePress::OpenTwice
                    } else {
                        FilePress::Open
                    };
                    file(&name, press, window, cx);
                })
                .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    menu_file(&menu_name, FilePress::Menu(event.position), window, cx);
                })
                .child(
                    Attachment::new()
                        .id(id)
                        .with_size(gpui_kit::component::Size::Small)
                        .tooltip(path.to_string_lossy().into_owned())
                        .media(media(&path, &t, cx))
                        .content(
                            AttachmentContent::new().title(AttachmentTitle::new(file_name(&path))),
                        ),
                )
                .into_any_element();
        }
        let number = index + 1;
        let tagged = written.contains(&format!("[Image #{number}]"));
        let open = open.clone();
        let mut tile = media(&path, &t, cx);
        if tagged {
            tile = tile.overlay(badge(number, &t, cx));
        }
        Attachment::new()
            .id(id)
            .axis(Axis::Vertical)
            .with_size(gpui_kit::component::Size::Size(px(16.6)))
            .tooltip(path.to_string_lossy().into_owned())
            .media(tile)
            .on_click(move |_, window, cx| open(path.clone(), window, cx))
            .into_any_element()
    });
    let context = message.context.iter().enumerate().map(|(index, item)| {
        context_chip(
            format!("sent-context-{}-{index}", message.id),
            &item.uri,
            &item.title,
            &t,
        )
    });
    Some(
        div()
            .flex()
            .flex_wrap()
            .justify_end()
            .items_end()
            .gap(px(6.))
            .pb(px(6.))
            .children(files)
            .children(context),
    )
}
