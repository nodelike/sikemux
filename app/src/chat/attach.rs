//! The files a message carries: picked, dropped on the chat or pasted, shown as tiles over
//! the draft, each picture also named in the draft where it belongs, as `[Image #n]`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::base::input::{InlineToken, InlineTokenContext};
use gpui_kit::component::attachment::{
    Attachment, AttachmentContent, AttachmentMedia, AttachmentTitle,
};
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnimationExt, App, AppContext, Axis, ClipboardEntry, ClipboardItem, Context, Div,
    ExternalPaths, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    PathPromptOptions, SharedString, StatefulInteractiveElement, Styled, StyledImage, WeakEntity,
    Window,
};
use gpui_transcript::FilePress;
use sikemux_core::server::chat::prompt::MAX_ATTACHMENTS;
use sikemux_view::chat::{
    tracked_at, tracked_title, SentChip, SentChipKind, SentSpan, TrackedKind, AGENT_URI,
    ANSWER_URI, EARLIER_URI, REPLY_URI, SIDE_URI, STASH_URI, WOKEN_URI,
};
use sikemux_view::chat::{ChatMessage, SentContext, WhyChip, WhyKind, WhyOpen, WHY_URI};
use sikemux_wire::protocol::ChatContext;
use sikemux_wire::protocol::{FOR_AGENT_URI, MAX_ATTACHMENT_BYTES};
use tokio::sync::oneshot;

use super::mention::{AGENT, COMMAND, EARLIER, FILE, FOLDER, TRACKED};
use super::Composing;
use crate::agents::rows::AgentRow;
use crate::core::ChatAsk;
use crate::drag::{DraggedChat, DraggedPath};
use crate::hosts::Place;
use crate::icons::Icon;
use crate::send_to::Recipient;
use crate::theme::{self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, TEXT_SM, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            crate::ui::path::IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// macOS deletes the picture a screenshot's thumbnail drags in once the drop is over.
fn is_screenshot_thumbnail(path: &Path) -> bool {
    path.to_string_lossy()
        .contains("/TemporaryItems/NSIRD_screencaptureui_")
}

/// `path`, or for a screenshot's thumbnail a copy of it in the temporary folder that stays
/// after macOS deletes the original.
pub fn lasting(path: &Path) -> PathBuf {
    if !is_screenshot_thumbnail(path) {
        return path.to_owned();
    }
    let kept = || {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis();
        let folder = std::env::temp_dir()
            .join("sikemux-drops")
            .join(stamp.to_string());
        std::fs::create_dir_all(&folder).ok()?;
        let copy = folder.join(path.file_name()?);
        std::fs::copy(path, &copy).ok()?;
        Some(copy)
    };
    kept().unwrap_or_else(|| path.to_owned())
}

const IMAGE: &str = "image:";

/// The largest file a chat's core keeps for it.
const MAX_KEPT: u64 = MAX_ATTACHMENT_BYTES as u64;

/// The glyph a tag in the draft wears: the picture itself, a named agent's face or mark, or a
/// file's or folder's.
fn mark(id: &str, tint: Hsla, bench: &WeakEntity<Workbench>, cx: &App) -> gpui_kit::AnyElement {
    let named =
        (id.strip_prefix(AGENT)).and_then(|agent| bench.upgrade()?.read(cx).agent_by_id(agent));
    mark_of(id, tint, named, cx)
}

/// The glyph a tag wears, with the agent it names, when it names one the app knows.
fn mark_of(id: &str, tint: Hsla, named: Option<AgentRow>, cx: &App) -> gpui_kit::AnyElement {
    if let Some(path) = id.strip_prefix(IMAGE) {
        return gpui_kit::img(
            crate::picture::Picture::File(PathBuf::from(path)).source_at(px(96.)),
        )
        .size(px(15.))
        .flex_none()
        .rounded(px(3.))
        .object_fit(gpui_kit::ObjectFit::Cover)
        .into_any_element();
    }
    if let Some(agent) = named {
        return agent.mark(14., cx);
    }
    if id.starts_with(AGENT) {
        return ui::icon(Icon::Agent, 13.)
            .text_color(tint)
            .into_any_element();
    }
    if id.starts_with(COMMAND) {
        return ui::icon(Icon::Bolt, 13.)
            .text_color(tint)
            .into_any_element();
    }
    if id.starts_with(EARLIER) {
        return ui::icon(Icon::Message, 13.)
            .text_color(tint)
            .into_any_element();
    }
    if let Some(kind) = id.strip_prefix(TRACKED) {
        let icon = if kind.starts_with("pull") {
            Icon::PullRequest
        } else {
            Icon::Issue
        };
        return ui::icon(icon, 13.).text_color(tint).into_any_element();
    }
    if id.starts_with(FOLDER) {
        return ui::icon(Icon::Folder, 13.)
            .text_color(tint)
            .into_any_element();
    }
    let path = Path::new(id.strip_prefix(FILE).unwrap_or(id));
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    crate::file_icons::icon(&name, cx).map_or_else(
        || {
            ui::icon(Icon::File, 13.)
                .text_color(tint)
                .into_any_element()
        },
        |icon| gpui_kit::img(icon).size(px(13.)).into_any_element(),
    )
}

/// A picture made in the app, such as a snip of a page or a SnapShot of a window, on its way to
/// a chat. `about` writes, from where the picture was kept, a title and text the agent alone is
/// told with it.
pub struct Made {
    pub name: String,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    pub about: About,
}

/// Writes, from where a picture was kept, its title and the text the agent alone is told.
pub type About = Box<dyn FnOnce(&Path) -> (String, String)>;

/// Where the note the agent alone gets about picture `path` is filed, so it goes with the picture.
fn about_uri(path: &Path) -> String {
    format!("{FOR_AGENT_URI}picture{}", path.to_string_lossy())
}

/// How the name of a picture snipped from a page starts, which the draft calls a selection.
pub const SNIP_NAME: &str = "snip-";

fn is_snip(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(SNIP_NAME))
}

/// What the draft calls picture `number`. The message itself still says `[Image #n]`, which is
/// where the core puts the picture.
fn picture_label(number: usize, path: &Path) -> String {
    match is_snip(path) {
        true => t!("Selection {number}", number = number),
        false => t!("Image {number}", number = number),
    }
}

fn image_tag(number: usize, path: &Path) -> InlineToken {
    InlineToken::new(
        format!("{IMAGE}{}", path.to_string_lossy()),
        format!("[Image #{number}]"),
    )
    .with_label(picture_label(number, path))
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
            self.fresh = Some((path.clone(), std::time::Instant::now()));
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

    /// Takes off the pictures whose tag the person deleted from the draft.
    pub fn drop_untagged(&mut self, window: &mut Window, cx: &mut App) {
        loop {
            let draft = self.draft.read(cx);
            let untagged = self.attachments.iter().position(|path| {
                let id = format!("{IMAGE}{}", path.to_string_lossy());
                is_image(path) && !draft.tokens().iter().any(|span| *span.token().id() == id)
            });
            let Some(index) = untagged else {
                return;
            };
            self.detach(index, window, cx);
        }
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
        let note = about_uri(Path::new(&gone));
        self.context.retain(|item| item.uri != note);
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
        self.attach_there(agent, paths, Vec::new(), window, cx);
    }

    /// Hands the next message of `agent`'s chat files that are already where the agent runs,
    /// with notes about them for the agent alone. They wait for a composer the chat has not
    /// made yet.
    pub(crate) fn attach_there(
        &mut self,
        agent: &str,
        paths: Vec<PathBuf>,
        notes: Vec<ChatContext>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let Some(composing) = self.composing_mut(agent) else {
            let held = self.chats.held_files.entry(agent.to_owned()).or_default();
            held.0.extend(paths);
            held.1.extend(notes);
            return;
        };
        composing.attach(paths, window, cx);
        let attached: Vec<String> = composing
            .attachments
            .iter()
            .map(|path| about_uri(path))
            .collect();
        composing.context.extend(
            notes
                .into_iter()
                .filter(|note| attached.contains(&note.uri)),
        );
        self.typed_draft(agent, cx);
        cx.notify();
    }

    /// Hands `agent`'s next message a picture made in the app, named at its draft's caret, with
    /// what `about` says of it for the agent alone; an agent in a terminal is typed its path and
    /// that text.
    pub(crate) fn attach_picture(
        &mut self,
        agent: &str,
        made: Made,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Made {
            name,
            mime,
            bytes,
            about,
        } = made;
        let owner = agent.to_owned();
        let kept =
            move |bench: &mut Self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>| {
                let (title, text) = about(&path);
                if bench.in_terminal(&owner) {
                    let typed = format!("{}\n{text}", path.to_string_lossy());
                    return bench.deliver(&owner, typed, window, cx);
                }
                let note = ChatContext {
                    uri: about_uri(&path),
                    title,
                    text,
                };
                bench.attach_there(&owner, vec![path], vec![note], window, cx);
            };
        self.keep(agent, name, mime, bytes, kept, window, cx);
    }

    /// Hands a picture made in the app to the project's front chat, starting one when none is
    /// open. `kind` names such pictures in what the person is told when no agent can take it.
    /// Says which chat it went to, if it went to one.
    pub(crate) fn hand_to_front_chat(
        &mut self,
        made: Made,
        kind: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SharedString> {
        if let Some(waiting) = &mut self.chats.pictures_for_new_chat {
            waiting.push(made);
            return Some(t!("the new chat").into());
        }
        let Some(choice) = crate::send_to::choices(self).into_iter().next() else {
            let note = t!("Start an agent first — {kind} go to its chat", kind = kind);
            self.notify(crate::toast::info(note), cx);
            return None;
        };
        let (launcher, project) = match choice.to {
            Recipient::Agent(agent) => {
                self.attach_picture(&agent, made, window, cx);
                return Some(choice.label);
            }
            Recipient::NewChat { launcher, project } => (launcher, project),
        };
        self.chats.pictures_for_new_chat = Some(vec![made]);
        let (started, agent) = oneshot::channel();
        self.target()
            .start_chat(launcher, project, None, Some(started));
        cx.spawn_in(window, async move |bench, cx| {
            let agent = agent.await.ok();
            let _ = bench.update_in(cx, |bench, window, cx| {
                let waiting = bench.chats.pictures_for_new_chat.take();
                let Some(agent) = agent else {
                    return;
                };
                for made in waiting.into_iter().flatten() {
                    bench.attach_picture(&agent, made, window, cx);
                }
            });
        })
        .detach();
        Some(choice.label)
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
                            bench.keep(&agent, name, mime, bytes, attach_kept(&agent), window, cx)
                        })
                        .ok();
                    }
                    Some(Err(error)) => {
                        eprintln!("sikemux: {} does not read: {error}", path.display())
                    }
                    None => {
                        let note = t!(
                            "{name} is over {megabytes} MB, too large to send to the chat",
                            name = name,
                            megabytes = MAX_KEPT / (1024 * 1024)
                        );
                        cx.update(|window, cx| window.push_notification(note, cx))
                            .ok();
                    }
                }
            }
        })
        .detach();
    }

    /// Asks the core to keep a file that lives nowhere the agent can read, and hands where it
    /// went to `kept`.
    #[allow(clippy::too_many_arguments)]
    fn keep(
        &self,
        agent: &str,
        name: String,
        mime: &str,
        bytes: Vec<u8>,
        kept: impl FnOnce(&mut Self, PathBuf, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (stored, answer) = oneshot::channel();
        let ask = ChatAsk::Keep {
            agent: agent.to_owned(),
            name,
            mime: mime.to_owned(),
            bytes,
            kept: stored,
        };
        self.ask_chat(ask);
        cx.spawn_in(window, async move |this, cx| match answer.await {
            Ok(Ok(path)) => {
                this.update_in(cx, |bench, window, cx| kept(bench, path, window, cx))
                    .ok();
            }
            Ok(Err(error)) => {
                let note = t!(
                    "The picture or file could not be attached: {error}",
                    error = error
                );
                cx.update(|window, cx| window.push_notification(note, cx))
                    .ok();
            }
            Err(_) => {}
        })
        .detach();
    }

    /// Takes context item `index`, an earlier chat or an issue, off the message.
    pub fn forget(&mut self, agent: &str, index: usize, cx: &mut Context<Self>) {
        if let Some(composing) = self.composing_mut(agent) {
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
        if let Some(composing) = self.composing_mut(agent) {
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
                    let (stamp, agent, image) =
                        (stamp.to_string(), agent.to_owned(), image.clone());
                    let readable =
                        cx.background_spawn(async move { crate::pasted::readable(image) });
                    cx.spawn_in(window, async move |this, cx| {
                        let image = readable.await;
                        let name = format!("{stamp}.{}", image.format.extension());
                        this.update_in(cx, |bench, window, cx| {
                            let mime = image.format.mime_type();
                            let kept = attach_kept(&agent);
                            bench.keep(&agent, name, mime, image.bytes, kept, window, cx)
                        })
                        .ok();
                    })
                    .detach();
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

/// Attaches a file the core kept to `agent`'s next message.
fn attach_kept(
    agent: &str,
) -> impl FnOnce(&mut Workbench, PathBuf, &mut Window, &mut Context<Workbench>) + 'static {
    let agent = agent.to_owned();
    move |bench, path, window, cx| bench.attach_there(&agent, vec![path], Vec::new(), window, cx)
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

/// Files dropped on a composer's pane, from Finder or from the file tree, go with its next
/// message, and a chat dragged from the agent rail goes in it as context.
pub fn takes_drops(pane: Div, agent: &str, this: WeakEntity<Workbench>) -> Div {
    let agent = agent.to_owned();
    let (from_tree, from_tree_agent) = (this.clone(), agent.clone());
    let (from_rail, from_rail_agent) = (this.clone(), agent.clone());
    let own = agent.clone();
    pane.can_drop(move |value, _, _| {
        value
            .downcast_ref::<DraggedChat>()
            .is_none_or(|chat| takes_chat(chat, &own))
    })
    .on_drop(move |dropped: &DraggedChat, window, cx| {
        from_rail
            .update(cx, |bench, cx| {
                bench.mention_chat(&from_rail_agent, dropped, window, cx)
            })
            .ok();
    })
    .on_drop(move |dropped: &ExternalPaths, window, cx| {
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

/// Whether `agent`'s composer takes `chat` as context: any chat but its own that names an
/// agent or a saved chat.
fn takes_chat(chat: &DraggedChat, agent: &str) -> bool {
    (chat.agent.is_some() || chat.recent.is_some())
        && chat.agent.as_ref().is_none_or(|(id, _)| id != agent)
}

/// What shows over a composer's pane while files or a chat are dragged over it, saying where
/// they go: `words` for files, and whose chat goes in as context for a chat.
pub fn drop_hint(words: &'static str, agent: &str, bench: &Workbench, cx: &App) -> Div {
    let t = Tokens::get(cx).clone();
    let dragged = crate::drag::dragged_chat(bench);
    let takes = dragged.is_some_and(|chat| takes_chat(chat, agent));
    let chat = dragged.filter(|_| takes).map(|chat| {
        let named = chat.agent.as_ref().map(|(_, name)| format!("@{name}"));
        let title = chat.label.clone();
        match named {
            Some(named) => (t!("Add {named} as context", named = named), Some(title)),
            None => (t!("Add \"{title}\" as context", title = title), None),
        }
    });
    let (words, under) = match chat {
        Some((words, under)) => (SharedString::from(words), under),
        None => (SharedString::from(words), None),
    };
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
        .flex_col()
        .gap(SPACE_1)
        .drag_over::<ExternalPaths>(|style, _, _, _| style.visible())
        .drag_over::<DraggedPath>(|style, _, _, _| style.visible())
        .when(takes, |hint| {
            hint.drag_over::<DraggedChat>(|style, _, _, _| style.visible())
        })
        .child(ui::text(words, TEXT_SM, FontWeight::MEDIUM).text_color(t.text_primary))
        .children(under.map(|under| {
            ui::text(under, TEXT_XS, FontWeight::NORMAL)
                .max_w(px(320.))
                .truncate()
                .text_color(t.text_tertiary)
        }))
}

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
    let (key, title) = chip_words(token.token().id(), token.token().label());
    div().h(token.line_height()).flex().items_center().child(
        chip_face(
            key,
            title,
            token.is_selected(),
            mark(token.token().id(), Tokens::get(cx).text_tertiary, bench, cx),
            cx,
        )
        .h(token.line_height() - px(4.))
        .max_w(token.available_width().min(px(CHIP_WIDEST))),
    )
}

/// The widest a chip grows before its title is cut short.
const CHIP_WIDEST: f32 = 300.;
/// What a chip holds besides its words: the space before its mark, the mark, the space after
/// it, and the space after its words.
const CHIP_MARK: f32 = 15.;
const CHIP_FRAME: f32 = 4. + CHIP_MARK + 5. + 6.;
const CHIP_TITLE_GAP: f32 = 5.;

/// A chip as the draft and a sent message both draw it: its mark, its handle, and the title
/// after it a step quieter, on a slightly rounded grey lift.
fn chip_face(
    key: SharedString,
    title: Option<SharedString>,
    selected: bool,
    mark: gpui_kit::AnyElement,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx).clone();
    div()
        .pl(SPACE_1)
        .pr(SPACE_1H)
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded(theme::RADIUS_2)
        .bg(if selected {
            Hsla {
                a: 0.2,
                ..t.text_primary
            }
        } else {
            t.surface_chosen
        })
        .text_color(t.text_primary)
        .whitespace_nowrap()
        .child(
            div()
                .w(px(CHIP_MARK))
                .flex_none()
                .flex()
                .justify_center()
                .child(mark),
        )
        .child(div().flex_none().child(key))
        .when_some(title, |chip, title| {
            chip.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(t.text_secondary)
                    .child(title),
            )
        })
}

/// A mention as the draft draws its chip, on a line of the size around it, wearing `mark` in
/// place of its own when given one.
pub(crate) fn pill(
    id: &str,
    label: &str,
    mark: Option<gpui_kit::AnyElement>,
    bench: &Workbench,
    cx: &App,
) -> Div {
    let (key, title) = chip_words(id, &label.to_owned().into());
    let mark = mark.unwrap_or_else(|| {
        let named = (id.strip_prefix(AGENT)).and_then(|agent| bench.agent_by_id(agent));
        mark_of(id, Tokens::get(cx).text_tertiary, named, cx)
    });
    chip_face(key, title, false, mark, cx)
}

/// A sent message's look with the chips it wrote drawn in its text as the draft drew them.
pub fn with_chips(
    look: Rc<gpui_transcript::Style>,
    message: &ChatMessage,
    bench: &WeakEntity<Workbench>,
    window: &mut Window,
) -> Rc<gpui_transcript::Style> {
    let mentions = mentions(message);
    if mentions.is_empty() {
        return look;
    }
    let (size, font) = (look.text_size, look.font.clone());
    let height = (size * look.line_height).floor() - px(4.);
    let measure = |text: &str| {
        let mut style = window.text_style();
        style.font_family = look.font.clone();
        let run = style.to_run(text.len());
        window
            .text_system()
            .shape_line(text.to_owned().into(), size, &[run], None)
            .width
    };
    let chips: Vec<gpui_transcript::Chip> = mentions
        .into_iter()
        .map(|mention| {
            let (key, title) = chip_words(&mention.id, &mention.label.into());
            let words = measure(&key)
                + title
                    .as_ref()
                    .map_or(px(0.), |title| px(CHIP_TITLE_GAP) + measure(title));
            let width = (words + px(CHIP_FRAME + 1.)).min(px(CHIP_WIDEST)).ceil();
            let (bench, font) = (bench.clone(), font.clone());
            gpui_transcript::Chip {
                written: mention.written.into(),
                width,
                draw: Rc::new(move |_, cx| {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .text_size(size)
                        .font_family(font.clone())
                        .child(
                            chip_face(
                                key.clone(),
                                title.clone(),
                                false,
                                mark(&mention.id, Tokens::get(cx).text_tertiary, &bench, cx),
                                cx,
                            )
                            .w_full()
                            .h(height),
                        )
                        .into_any_element()
                }),
            }
        })
        .collect();
    Rc::new(gpui_transcript::Style {
        chips: chips.into(),
        ..(*look).clone()
    })
}

/// An answer's look with each `why` link drawn as the chip the core checked it to be, which opens
/// what it names; a link the core did not check draws nothing.
pub fn with_why(
    look: Rc<gpui_transcript::Style>,
    message: &ChatMessage,
    bench: &WeakEntity<Workbench>,
    window: &mut Window,
) -> Rc<gpui_transcript::Style> {
    let links = (message.parts.iter()).any(|part| match part {
        sikemux_view::chat::ChatPart::Text { text, .. } => text.contains(WHY_URI),
        _ => false,
    });
    if !links && message.chips.is_empty() {
        return look;
    }
    let (size, font) = (look.text_size, look.font.clone());
    let height = (size * look.line_height).floor() - px(4.);
    let measure = |text: &str| {
        let mut style = window.text_style();
        style.font_family = look.font.clone();
        let run = style.to_run(text.len());
        window
            .text_system()
            .shape_line(text.to_owned().into(), size, &[run], None)
            .width
    };
    let chips: Vec<(String, gpui_transcript::Chip)> = (message.chips.iter())
        .map(|chip| {
            let (icon, key, title) = why_words(chip);
            let words = measure(&key) + px(CHIP_TITLE_GAP) + measure(&title);
            let width = (words + px(CHIP_FRAME + 1.)).min(px(CHIP_WIDEST)).ceil();
            let (bench, font, open) = (bench.clone(), font.clone(), chip.open.clone());
            let id = SharedString::from(format!("why:{}", chip.href));
            let drawn = gpui_transcript::Chip {
                written: SharedString::default(),
                width,
                draw: Rc::new(move |_, cx| {
                    let mark = ui::icon(icon, CHIP_MARK - 3.)
                        .text_color(Tokens::get(cx).text_tertiary)
                        .into_any_element();
                    let (bench, open) = (bench.clone(), open.clone());
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .text_size(size)
                        .font_family(font.clone())
                        .child(
                            chip_face(
                                key.clone().into(),
                                Some(title.clone().into()),
                                false,
                                mark,
                                cx,
                            )
                            .id(id.clone())
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                let open = open.clone();
                                bench.update(cx, |bench, cx| open_why(bench, open, cx)).ok();
                            })
                            .w_full()
                            .h(height),
                        )
                        .into_any_element()
                }),
            };
            (chip.href.clone(), drawn)
        })
        .collect();
    let hidden = gpui_transcript::Chip {
        written: SharedString::default(),
        width: px(0.),
        draw: Rc::new(|_, _| div().into_any_element()),
    };
    Rc::new(gpui_transcript::Style {
        link_chip: Some(Rc::new(move |href: &str| {
            if !href.starts_with(WHY_URI) {
                return None;
            }
            let known = chips.iter().find(|(known, _)| known == href);
            Some(known.map_or_else(|| hidden.clone(), |(_, chip)| chip.clone()))
        })),
        ..(*look).clone()
    })
}

/// A `why` chip's icon and words: what it is, then when.
fn why_words(chip: &WhyChip) -> (Icon, String, String) {
    let day = |time: bool| chip.at.map(|at| said_at(at, time)).unwrap_or_default();
    match chip.kind {
        WhyKind::Said => (Icon::Message, t!("your message").into(), day(true)),
        WhyKind::Change if chip.landed => (
            Icon::Commit,
            t!("the change").into(),
            t!("landed {day}", day = day(false)),
        ),
        WhyKind::Change => (Icon::Commit, t!("the change").into(), day(false)),
        WhyKind::Call => (Icon::Check, t!("your answer").into(), day(false)),
        WhyKind::Rule => (
            Icon::Book,
            t!("your rule").into(),
            t!("said {day}", day = day(false)),
        ),
        WhyKind::Decision => (Icon::Merge, t!("decided").into(), day(false)),
        WhyKind::Unknown => (Icon::Message, t!("source").into(), day(false)),
    }
}

/// When something happened, as a person says it looking back: the time today, the weekday this
/// week, else the date.
fn said_at(at: u64, time: bool) -> String {
    let Some(at) = chrono::DateTime::from_timestamp_millis(at as i64) else {
        return String::new();
    };
    let at = at.with_timezone(&chrono::Local);
    let days = (chrono::Local::now().date_naive() - at.date_naive()).num_days();
    use crate::ui::time::{month_day, time_of_day, weekday};
    match (days, time) {
        (..=0, true) => time_of_day(at),
        (..=0, false) => t!("today").into(),
        (1..=6, true) => format!("{} {}", weekday(at), time_of_day(at)),
        (1..=6, false) => weekday(at),
        _ => month_day(at),
    }
}

/// Opens what a `why` chip names: the chat at the moment, the commit in Git, the Playbook's
/// rules, or the chat of the agent that asked the call.
fn open_why(bench: &mut Workbench, open: WhyOpen, cx: &mut Context<Workbench>) {
    match open {
        WhyOpen::Chat { agent, feed, index } => {
            let moment = crate::playbook::said::Moment::new(agent, feed, index);
            crate::playbook::said::open_moment(bench, moment, cx);
        }
        WhyOpen::Commit { sha } => bench.open_commit(sha, cx),
        WhyOpen::Rule { .. } => bench.open_rules(),
        WhyOpen::Call { id } => {
            let asker = bench
                .call_at(&id)
                .map(|(_, call)| call.asker.agent_id.clone());
            if let Some(asker) = asker {
                bench.bring_agent(&asker, cx).detach();
            }
        }
        WhyOpen::Unknown => {}
    }
}

/// Something a sent message names in its text: the words it wrote, the chip they stood for
/// in the draft, and the file or address it carried for it.
pub(crate) struct Mention {
    pub written: String,
    pub id: String,
    pub label: String,
    pub carried: String,
}

/// Every named agent the app knows, by name and id, for the `@` mentions sent messages wrote.
pub(crate) fn named_agents(bench: &Workbench) -> Vec<(String, String)> {
    bench
        .projects()
        .iter()
        .flat_map(|project| bench.agents_of(&project.id))
        .filter_map(|row| Some((row.name?, row.id)))
        .collect()
}

/// What a sent message's text names, as the core read it.
fn mentions(message: &ChatMessage) -> Vec<Mention> {
    mentions_in(&message.spans)
}

/// What `written` names of the files and context it went with, and of `agents`. A stash item is
/// read here as the core reads a sent message.
pub(crate) fn named(
    written: &str,
    attachments: &[String],
    context: &[SentContext],
    agents: &[(String, String)],
) -> Vec<Mention> {
    mentions_in(&sikemux_fold::sent_spans(
        written,
        sikemux_fold::SentNames {
            attachments,
            context,
            agents,
            ..Default::default()
        },
    ))
}

fn mentions_in(spans: &[SentSpan]) -> Vec<Mention> {
    let mut seen = HashSet::new();
    (spans.iter())
        .filter_map(|span| Some((span.text.as_str(), span.chip.as_ref()?)))
        .filter(|(written, _)| seen.insert(*written))
        .filter_map(|(written, chip)| mention_of(written, chip))
        .collect()
}

fn mention_of(written: &str, chip: &SentChip) -> Option<Mention> {
    let carried = chip.carried.clone().unwrap_or_default();
    let agent = || {
        (carried.strip_prefix(AGENT_URI))
            .or_else(|| carried.strip_prefix(WOKEN_URI))
            .unwrap_or(&carried)
    };
    let id = match chip.kind {
        SentChipKind::Agent => format!("{AGENT}{}", agent()),
        SentChipKind::Chat => format!(
            "{EARLIER}{}",
            carried.strip_prefix(EARLIER_URI).unwrap_or(&carried)
        ),
        SentChipKind::Command => format!("{COMMAND}{}", written.trim_start_matches('/')),
        SentChipKind::File => format!("{FILE}{carried}"),
        SentChipKind::Folder => format!("{FOLDER}{carried}"),
        SentChipKind::Image => format!("{IMAGE}{carried}"),
        SentChipKind::Issue => format!("{TRACKED}issue:{}:{carried}", &written[1..]),
        SentChipKind::Pull => format!("{TRACKED}pull:{}:{carried}", &written[1..]),
        SentChipKind::Other => return None,
    };
    let label = match chip.kind {
        SentChipKind::Image => chip.label.replace('#', ""),
        _ => chip.label.clone(),
    };
    Some(Mention {
        written: written.to_owned(),
        id,
        label,
        carried,
    })
}

/// What a chip says: its handle without the sign typed to make it, and for an issue or pull
/// request its title after the number, without a `feat(scope):` kind of prefix.
fn chip_words(id: &str, label: &SharedString) -> (SharedString, Option<SharedString>) {
    if id.starts_with(TRACKED) {
        if let Some((number, title)) = label.split_once(' ') {
            let title = title
                .split_once(": ")
                .filter(|(kind, _)| kind.len() <= 24 && !kind.contains(' '))
                .map_or(title, |(_, rest)| rest);
            let title = (!title.is_empty()).then(|| title.to_owned().into());
            return (number.to_owned().into(), title);
        }
    }
    let bare = label
        .strip_prefix(['@', '+', '/'])
        .map_or_else(|| label.clone(), |bare| bare.to_owned().into());
    (bare, None)
}

/// The files going with the message, as tiles: a picture's preview or a file's icon, its
/// number, and a way to take it off; then the chats and issues it carries, as chips.
/// `attachments` are the files going, with the one just attached, whose tile flashes.
pub fn tiles(
    (attachments, fresh): (&[PathBuf], Option<&Path>),
    context: &[ChatContext],
    remove: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    forget: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    open: impl Fn(PathBuf, &mut Window, &mut App) + Clone + 'static,
    asks: Option<&tokio::sync::mpsc::UnboundedSender<crate::core::Ask>>,
    cx: &App,
) -> Option<impl IntoElement> {
    if attachments.is_empty() && context.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let tiles = attachments.iter().enumerate().map(|(index, path)| {
        let remove = remove.clone();
        let opens = is_image(path).then(|| {
            let (open, path) = (open.clone(), path.clone());
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                open(path.clone(), window, cx)
            }
        });
        let number = is_image(path).then_some(index + 1);
        let name = match is_snip(path) {
            true => picture_label(index + 1, path),
            false => crate::ui::path::file_name(path),
        };
        let tile = framed(format!("attachment-{index}"), path, number, name, &t, cx)
            .when_some(opens, |tile, opens| tile.on_click(opens))
            .on_remove(move |_, window, cx| remove(index, window, cx));
        match fresh == Some(path.as_path()) {
            true => flashed(tile, path, t.accent).into_any_element(),
            false => tile.into_any_element(),
        }
    });
    let quotes = context.iter().enumerate();
    let quotes: Vec<_> = quotes
        .filter(|(_, item)| item.uri.starts_with(REPLY_URI))
        .map(|(index, item)| {
            let forget = forget.clone();
            let remove: Remove = Box::new(move |window, cx| forget(index, window, cx));
            quote_card(format!("quote-{index}"), &item.text, remove, cx)
        })
        .collect();
    let shown = context.iter().enumerate();
    let chips = shown
        .filter(|(_, item)| !item.for_agent_only() && !item.uri.starts_with(REPLY_URI))
        .map(|(index, item)| {
            let forget = forget.clone();
            context_chip(format!("context-{index}"), &item.uri, &item.title, asks, &t)
                .on_remove(move |_, window, cx| forget(index, window, cx))
        });
    Some(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(7.))
            .px(SPACE_2H)
            .pt(SPACE_1)
            .children(quotes)
            .children(tiles)
            .children(chips),
    )
}

/// How long after a file is attached its tile still flashes when it first draws.
pub const FRESH: std::time::Duration = std::time::Duration::from_millis(800);

/// A tile just attached, its edge lit in `accent` and fading out.
fn flashed(tile: Attachment, path: &Path, accent: Hsla) -> impl IntoElement {
    let id = SharedString::from(format!("attachment-flash-{}", path.to_string_lossy()));
    tile.with_animation(
        id,
        crate::motion::entrance(crate::motion::ATTACHED),
        move |tile, shown| tile.border_color(accent.opacity(1. - shown)),
    )
}

/// A file as a draft and a sent message both show it: a small framed card with its preview, or its
/// kind's icon, over a picture's `#n` and the file's name.
fn framed(
    id: impl Into<SharedString>,
    path: &Path,
    number: Option<usize>,
    name: String,
    t: &Tokens,
    cx: &App,
) -> Attachment {
    Attachment::new()
        .id(id.into())
        .axis(Axis::Vertical)
        .with_size(gpui_kit::component::Size::XSmall)
        .w(px(FRAMED_WIDTH))
        .p(px(3.))
        .gap(px(3.))
        .bg(t.surface_chosen)
        .border_color(gpui_kit::transparent_black())
        .tooltip(path.to_string_lossy().into_owned())
        .media(media(path, t, cx).h(px(FRAMED_PREVIEW)))
        .content(AttachmentContent::new().child(caption(number, name, t, cx)))
}

const FRAMED_WIDTH: f32 = 64.;
const FRAMED_PREVIEW: f32 = 40.;

/// The line under a draft's framed card: a picture's `#n`, then the file's name.
fn caption(number: Option<usize>, name: String, t: &Tokens, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .min_w_0()
        .text_size(px(theme::TEXT_3XS))
        .text_color(t.text_secondary)
        .children(number.map(|number| {
            ui::mono(format!("#{number}"), 9., cx)
                .flex_none()
                .text_color(t.text_tertiary)
        }))
        .child(div().min_w_0().truncate().child(name))
}

fn media(path: &Path, t: &Tokens, cx: &App) -> AttachmentMedia {
    let media = if is_image(path) {
        AttachmentMedia::new()
            .src(crate::picture::Picture::File(path.to_path_buf()).source_at(px(96.)))
    } else {
        let icon =
            crate::file_icons::icon(&SharedString::from(crate::ui::path::file_name(path)), cx)
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

/// The mark and number of the issue or pull request at `uri`, when it names one.
fn tracked(uri: &str) -> Option<(Icon, &str)> {
    tracked_at(uri).map(|(kind, number)| {
        let icon = match kind {
            TrackedKind::Issue => Icon::Issue,
            TrackedKind::Pull => Icon::PullRequest,
        };
        (icon, number)
    })
}

/// A context item's chip: `#12 Title` for an issue or pull request, its title for an
/// earlier chat, an answer or anything else.
fn context_chip(
    id: String,
    uri: &str,
    title: &str,
    asks: Option<&tokio::sync::mpsc::UnboundedSender<crate::core::Ask>>,
    t: &Tokens,
) -> Attachment {
    let web = uri.starts_with("https://") || uri.starts_with("http://");
    let (icon, title) = match tracked(uri) {
        Some((icon, number)) => (
            icon,
            format!("#{number} {}", tracked_title(title, number))
                .trim_end()
                .to_owned(),
        ),
        None if uri.starts_with(EARLIER_URI) => (Icon::Message, title.to_owned()),
        None if uri.starts_with(STASH_URI) => (Icon::Text, title.to_owned()),
        None if uri.starts_with(ANSWER_URI) || uri.starts_with(SIDE_URI) => {
            (Icon::Message, title.to_owned())
        }
        None if uri.starts_with(REPLY_URI) => (Icon::Reply, title.to_owned()),
        None if web => (
            Icon::Globe,
            if title.is_empty() { uri } else { title }.to_owned(),
        ),
        None if title.is_empty() => (Icon::File, uri.to_owned()),
        None => (Icon::File, title.to_owned()),
    };
    Attachment::new()
        .id(SharedString::from(id))
        .with_size(gpui_kit::component::Size::Small)
        .tooltip(
            match uri.starts_with(ANSWER_URI) || uri.starts_with(REPLY_URI) {
                true => title.clone(),
                false => uri.to_owned(),
            },
        )
        .media(AttachmentMedia::new().child(site_icon(uri, web, icon, asks, t)))
        .content(AttachmentContent::new().title(AttachmentTitle::new(title)))
}

/// A link's site icon, which the core fetches, else `icon`.
pub(crate) fn site_icon(
    uri: &str,
    web: bool,
    icon: Icon,
    asks: Option<&tokio::sync::mpsc::UnboundedSender<crate::core::Ask>>,
    t: &Tokens,
) -> gpui_kit::AnyElement {
    let glyph = ui::icon(icon, 13.).text_color(t.text_tertiary);
    let favicon = url::Url::parse(uri)
        .ok()
        .filter(|_| web && tracked(uri).is_none())
        .and_then(|url| {
            Some(format!(
                "{}://{}/favicon.ico",
                url.scheme(),
                url.host_str()?
            ))
        });
    match (favicon, asks) {
        (Some(favicon), Some(asks)) => {
            let shown =
                crate::picture::Picture::Remote(crate::picture::Remote::new(favicon, asks.clone()));
            let fallback = std::cell::RefCell::new(Some(glyph.into_any_element()));
            gpui_kit::img(shown.source_at(px(14.)))
                .size(px(14.))
                .object_fit(gpui_kit::ObjectFit::Contain)
                .with_fallback(move || {
                    fallback
                        .borrow_mut()
                        .take()
                        .unwrap_or_else(|| div().into_any_element())
                })
                .into_any_element()
        }
        _ => glyph.into_any_element(),
    }
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
    let inline = mentions(message);
    let in_text = |carried: &str| {
        inline
            .iter()
            .any(|mention| mention.carried == carried && !mention.id.starts_with(IMAGE))
    };
    let files = message
        .attachments
        .iter()
        .enumerate()
        .filter(|(_, path)| !in_text(path));
    let files = files.map(|(index, path)| {
        let number = index + 1;
        carried(
            format!("sent-{}-{index}", message.id),
            PathBuf::from(path),
            written
                .contains(&format!("[Image #{number}]"))
                .then_some(number),
            open.clone(),
            file.clone(),
            &t,
            cx,
        )
    });
    let context = message.context.iter().enumerate();
    let context: Vec<_> = context
        .filter(|(_, item)| !in_text(&item.uri))
        .map(|(index, item)| {
            let quote = (item.quote.as_deref())
                .map(|quote| quote.split_whitespace().collect::<Vec<_>>().join(" "));
            context_chip(
                format!("sent-context-{}-{index}", message.id),
                &item.uri,
                quote.as_deref().unwrap_or(&item.title),
                None,
                &t,
            )
        })
        .collect();
    let files: Vec<_> = files.collect();
    if files.is_empty() && context.is_empty() {
        return None;
    }
    Some(
        div()
            .flex()
            .flex_wrap()
            .justify_end()
            .items_end()
            .gap(SPACE_1H)
            .pb(SPACE_1H)
            .children(files)
            .children(context),
    )
}

/// What takes a quote off a draft.
type Remove = Box<dyn Fn(&mut Window, &mut App)>;

/// The answer a draft replies to: the reply mark beside two lines of its words, on a sunken
/// card, with a way to take it off.
fn quote_card(
    id: impl Into<SharedString>,
    quote: &str,
    remove: Remove,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx);
    let quote: String = quote.split_whitespace().collect::<Vec<_>>().join(" ");
    div()
        .id(id.into())
        .w_full()
        .flex()
        .items_start()
        .gap(SPACE_1H)
        .px(SPACE_2)
        .py(SPACE_1H)
        .rounded(theme::RADIUS_3)
        .bg(Hsla {
            a: 0.6,
            ..t.surface_sunken
        })
        .child(
            ui::icon(Icon::Reply, 12.)
                .flex_none()
                .mt(px(2.))
                .text_color(t.text_tertiary),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .line_clamp(2)
                .child(quote),
        )
        .child(
            ui::IconButton::new("quote-remove", Icon::Close, t!("Remove quote"))
                .glyph(11.)
                .width(18.)
                .on_click(move |_, window, cx| remove(window, cx)),
        )
}

/// One file a message carried: a picture as a tile that opens over the window, with the number its
/// `[Image #n]` tag in the text names when it has one, or any other file by name.
fn carried(
    id: String,
    path: PathBuf,
    tag: Option<usize>,
    open: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
    file: super::files::FilePressed,
    t: &Tokens,
    cx: &App,
) -> gpui_kit::AnyElement {
    let name = crate::ui::path::file_name(&path);
    if !is_image(&path) {
        let (full, menu_full) = (
            path.to_string_lossy().into_owned(),
            path.to_string_lossy().into_owned(),
        );
        let menu_file = file.clone();
        return div()
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let press = if event.click_count > 1 {
                    FilePress::OpenTwice
                } else {
                    FilePress::Open
                };
                file(&full, press, window, cx);
            })
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                menu_file(&menu_full, FilePress::Menu(event.position), window, cx);
            })
            .child(framed(id, &path, None, name, t, cx))
            .into_any_element();
    }
    framed(id, &path, tag, name, t, cx)
        .on_click(move |_, window, cx| open(path.clone(), window, cx))
        .into_any_element()
}

/// A picture sent with an answer, as a square thumbnail that opens over the window.
pub fn thumbnail(
    id: String,
    path: PathBuf,
    open: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
    t: &Tokens,
) -> gpui_kit::AnyElement {
    let picture = crate::picture::Picture::File(path.clone()).source_at(px(128.));
    div()
        .id(SharedString::from(id))
        .size(px(64.))
        .flex_none()
        .rounded(px(7.))
        .overflow_hidden()
        .border_1()
        .border_color(t.border)
        .bg(t.surface_sunken)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open(path.clone(), window, cx)
        })
        .child(
            gpui_kit::img(picture)
                .size_full()
                .object_fit(gpui_kit::ObjectFit::Cover),
        )
        .into_any_element()
}

/// A file sent with an answer, as a slim pill with its icon and name that opens it.
pub fn file_pill(
    id: String,
    path: PathBuf,
    file: super::files::FilePressed,
    t: &Tokens,
    cx: &App,
) -> gpui_kit::AnyElement {
    let name = crate::ui::path::file_name(&path);
    let icon = crate::file_icons::icon(&SharedString::from(name.clone()), cx)
        .map(|icon| gpui_kit::img(icon).size(px(14.)).into_any_element())
        .unwrap_or_else(|| {
            ui::icon(Icon::File, 13.)
                .text_color(t.text_tertiary)
                .into_any_element()
        });
    let (full, menu_full, menu_file) = (
        path.to_string_lossy().into_owned(),
        path.to_string_lossy().into_owned(),
        file.clone(),
    );
    div()
        .id(SharedString::from(id))
        .h(px(30.))
        .pl(SPACE_2)
        .pr(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded(px(7.))
        .border_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .text_color(t.text_secondary)
        .cursor_pointer()
        .hover(|pill| pill.bg(t.surface_hover))
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            let press = match event.click_count > 1 {
                true => FilePress::OpenTwice,
                false => FilePress::Open,
            };
            file(&full, press, window, cx);
        })
        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
            menu_file(&menu_full, FilePress::Menu(event.position), window, cx);
        })
        .child(icon)
        .child(name)
        .into_any_element()
}
