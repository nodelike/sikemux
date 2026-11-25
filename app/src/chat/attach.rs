//! The files a message carries: picked, dropped on the chat or pasted, shown as tiles over
//! the draft, each also named by a pill in the draft where it belongs.

use crate::ui::Clicks;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::base::input::{InlineToken, InlineTokenContext, InputContent};
use gpui_kit::component::attachment::{
    Attachment, AttachmentContent, AttachmentMedia, AttachmentTitle,
};
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnimationExt, App, AppContext, Axis, ClipboardEntry, ClipboardItem, Context, Div,
    ExternalPaths, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    PathPromptOptions, SharedString, Styled, StyledImage, WeakEntity, Window,
};
use gpui_transcript::FilePress;
use sikemux_view::chat::{
    tracked_at, tracked_title, SentChip, SentChipKind, SentSpan, TrackedKind, AGENT_URI,
    ARTIFACT_URI, EARLIER_URI, REPLY_URI, SIDE_URI, STASH_URI, WOKEN_URI,
};
use sikemux_view::chat::{ChatMessage, SentContext, WhyChip, WhyKind, WhyOpen, WHY_URI};
use sikemux_wire::protocol::ChatContext;
use sikemux_wire::protocol::{FOR_AGENT_URI, MAX_ATTACHMENT_BYTES};
use tokio::sync::oneshot;

use super::clip::Field;
use super::mention::{AGENT, COMMAND, EARLIER, FILE, FOLDER, TRACKED};
use super::pills::{self, IMAGE};
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

/// The largest file a chat's core keeps for it.
pub(super) const MAX_KEPT: u64 = MAX_ATTACHMENT_BYTES as u64;

pub(super) fn mime_of(path: &Path) -> &'static str {
    match is_screenshot_thumbnail(path) || is_image(path) {
        true => "image/png",
        false => "application/octet-stream",
    }
}

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
    if id.starts_with(REPLY_URI) {
        return ui::icon(Icon::Reply, 13.)
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
pub(super) fn about_uri(path: &Path) -> String {
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
pub(super) fn picture_label(number: usize, path: &Path) -> String {
    match is_snip(path) {
        true => t!("Selection {number}", number = number),
        false => t!("Image {number}", number = number),
    }
}

impl Composing {
    /// Adds the files not already going, up to as many as a message carries, and names each at
    /// the caret, one already going again under its own name.
    fn attach(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut added = Vec::new();
        for path in paths {
            if path.as_os_str().is_empty() {
                continue;
            }
            let Some(number) = self.attachments.add(&path) else {
                break;
            };
            self.fresh = Some((path.clone(), std::time::Instant::now()));
            added.push((path, number));
        }
        if added.is_empty() {
            return;
        }
        self.draft.update(cx, |draft, cx| {
            let range = draft.selected_range();
            let value = draft.value();
            let spaced_before = (value.get(..range.start).unwrap_or_default())
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace);
            let spaced_after = (value.get(range.end..).unwrap_or_default())
                .chars()
                .next()
                .is_some_and(char::is_whitespace);
            if !spaced_before {
                draft.replace(" ", window, cx);
            }
            for (index, (path, number)) in added.into_iter().enumerate() {
                if index > 0 {
                    draft.replace(" ", window, cx);
                }
                let pill = pills::pill_of(&path, number, draft.tokens());
                if let Err(error) = draft.replace_with_token(pill, window, cx) {
                    eprintln!("sikemux: a file's pill did not go into the draft: {error}");
                }
            }
            if !spaced_after {
                draft.replace(" ", window, cx);
            }
        });
        self.settle(cx);
    }

    /// Follows what the person did to the draft's pills.
    pub fn settle(&mut self, cx: &App) {
        let draft = self.draft.read(cx);
        self.attachments.settle(draft.tokens(), &mut self.context);
    }

    /// The draft's text with its pills, as it goes out of the composer.
    pub fn content(&self, cx: &App) -> InputContent {
        pills::numbered(self.draft.read(cx).content(), &self.attachments)
    }

    /// Takes attachment `index` off the message, and its pills out of the draft.
    fn detach(&mut self, index: usize, window: &mut Window, cx: &mut App) {
        let Some(path) = self.attachments.get(index).cloned() else {
            return;
        };
        self.attachments.let_go(&path, &mut self.context);
        self.draft.update(cx, |draft, cx| {
            let places = pills::places(&path, draft.tokens(), &draft.value());
            for place in places.into_iter().rev() {
                draft.set_selected_range(place, cx);
                draft.replace("", window, cx);
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
        let composing = self.composing(agent);
        let (mut keep, mut there): (Vec<PathBuf>, Vec<PathBuf>) = (Vec::new(), Vec::new());
        for path in paths {
            let going = composing.and_then(|composing| {
                (composing.kept_from.get(&path))
                    .filter(|kept| composing.attachments.contains(*kept))
            });
            match going {
                Some(kept) => there.push(kept.clone()),
                None if elsewhere || is_screenshot_thumbnail(&path) => keep.push(path),
                None => there.push(path),
            }
        }
        if !keep.is_empty() {
            self.keep_files(agent, keep, window, cx);
        }
        self.attach_there(agent, there, Vec::new(), window, cx);
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
                        let mime = mime_of(&path);
                        let (owner, from) = (agent.clone(), path.clone());
                        let kept =
                            move |bench: &mut Self,
                                  path: PathBuf,
                                  window: &mut Window,
                                  cx: &mut Context<Self>| {
                                if let Some(composing) = bench.composing_mut(&owner) {
                                    composing.kept_from.insert(from, path.clone());
                                }
                                bench.attach_there(&owner, vec![path], Vec::new(), window, cx)
                            };
                        this.update_in(cx, |bench, window, cx| {
                            bench.keep(&agent, name, mime, bytes, kept, window, cx)
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
    /// went to `kept`. It is kept for the agent it is written to.
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
        let answer = self.store(agent, name, mime, bytes);
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

    /// Asks the core to keep a file for the agent it is written to, and answers where it went.
    pub(super) fn store(
        &self,
        agent: &str,
        name: String,
        mime: &str,
        bytes: Vec<u8>,
    ) -> oneshot::Receiver<Result<PathBuf, String>> {
        let (stored, answer) = oneshot::channel();
        let ask = ChatAsk::Keep {
            agent: self.written_to(agent).to_owned(),
            name,
            mime: mime.to_owned(),
            bytes,
            kept: stored,
        };
        self.chat_target(agent).ask(crate::core::Ask::Chat(ask));
        answer
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

    /// The pictures, files and part of a draft pasted into that of `agent`'s chat; whether there
    /// were any.
    fn pasted(
        &mut self,
        agent: &str,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.paste_clip(&Field::Composer(agent.to_owned()), item, window, cx) {
            return true;
        }
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
/// agent or a saved chat. A composer outside a chat takes none.
fn takes_chat(chat: &DraggedChat, agent: &str) -> bool {
    crate::calls::reply::About::of(agent).is_none()
        && (chat.agent.is_some() || chat.recent.is_some())
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
        .border(crate::theme::border())
        .border_dashed()
        .border_color(t.border_selected)
        .rounded(theme::radius_4())
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

/// A tag in the draft as a chip, with the glyph `mark` gives it. `number` is where a picture
/// stands among the files going.
pub fn chip(
    bench: WeakEntity<Workbench>,
    number: impl Fn(&Path, &App) -> Option<usize> + 'static,
) -> impl Fn(&InlineTokenContext, &mut Window, &mut App) -> gpui_kit::AnyElement {
    move |token, _, cx| {
        let picture = token.token().id().strip_prefix(IMAGE).map(Path::new);
        let label = picture
            .and_then(|path| Some(picture_label(number(path, cx)?, path).into()))
            .unwrap_or_else(|| token.token().label().clone());
        drawn_chip(token, &label, &bench, cx).into_any_element()
    }
}

/// Where a picture stands among the files going with what `owner`'s composer holds.
pub fn numbers(
    owner: &str,
    bench: WeakEntity<Workbench>,
) -> impl Fn(&Path, &App) -> Option<usize> + 'static {
    let owner = owner.to_owned();
    move |path, cx| {
        let bench = bench.upgrade()?;
        bench.read(cx).composing(&owner)?.attachments.number(path)
    }
}

fn drawn_chip(
    token: &InlineTokenContext,
    label: &SharedString,
    bench: &WeakEntity<Workbench>,
    cx: &mut App,
) -> impl IntoElement {
    let (key, title) = chip_words(token.token().id(), label);
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
/// after it a step quieter, on a slightly rounded opaque lift, so a backdrop never shows through.
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
        .rounded(theme::radius_2())
        .bg(if selected {
            theme::over(
                Hsla {
                    a: 0.2,
                    ..t.text_primary
                },
                t.surface_raised,
            )
        } else {
            t.surface_raised
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

/// An agent's chip as the transcript draws it in prose, set in a line of `size` text, which
/// opens the agent's chat.
pub(crate) fn agent_pill(
    agent: &AgentRow,
    size: f32,
    bench: &WeakEntity<Workbench>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let label = match (agent.chief, agent.name.as_ref()) {
        (false, Some(name)) => format!("@{name}"),
        _ => agent.title.clone(),
    };
    let (key, title) = chip_words(&format!("{AGENT}{}", agent.id), &label.into());
    let face = chip_face(key, title, false, agent.mark(size + 1., cx), cx)
        .flex_none()
        .text_size(theme::ui_text(size));
    opens_agent(face, &agent.id, None, bench, cx)
}

/// The chip of the agent `id` named `name`: [`agent_pill`] while the window knows the agent,
/// else the same chip wearing `mark`, which opens the agent's chat again through `from`.
pub(crate) fn agent_pill_of(
    id: &str,
    name: &str,
    mark: gpui_kit::AnyElement,
    size: f32,
    from: crate::core::Target,
    bench: &WeakEntity<Workbench>,
    cx: &App,
) -> gpui_kit::AnyElement {
    let known = (bench.upgrade()).and_then(|strong| strong.read(cx).agent_by_id(id));
    if let Some(agent) = known {
        return agent_pill(&agent, size, bench, cx).into_any_element();
    }
    unknown_agent_pill(id, name, mark, size, from, bench, cx)
}

/// The chip of an agent this window doesn't know, wearing `mark`, which opens its chat through `from`.
pub(crate) fn unknown_agent_pill(
    id: &str,
    name: &str,
    mark: gpui_kit::AnyElement,
    size: f32,
    from: crate::core::Target,
    bench: &WeakEntity<Workbench>,
    cx: &App,
) -> gpui_kit::AnyElement {
    let face = chip_face(format!("@{name}").into(), None, false, mark, cx)
        .flex_none()
        .text_size(theme::ui_text(size));
    opens_agent(face, id, Some(from), bench, cx).into_any_element()
}

/// A chip that opens `agent`'s chat, through `from` when given, lifting a step on hover.
fn opens_agent(
    face: Div,
    agent: &str,
    from: Option<crate::core::Target>,
    bench: &WeakEntity<Workbench>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let (agent, bench) = (agent.to_owned(), bench.clone());
    let t = Tokens::get(cx);
    let hover = theme::over(t.surface_active, t.surface_raised);
    face.id(SharedString::from(format!("mention:{agent}")))
        .hover(move |chip| chip.bg(hover))
        .clicks(move |_, _, cx| {
            cx.stop_propagation();
            bench
                .update(cx, |bench, cx| match from.clone() {
                    Some(from) => bench.open_agent_from(&agent, from, cx),
                    None => bench.open_agent(&agent, cx),
                })
                .ok();
        })
}

fn presses_file(face: Div, path: &str, file: &super::files::FilePressed) -> Div {
    let (path, menu_path) = (path.to_owned(), path.to_owned());
    let (file, menu) = (file.clone(), file.clone());
    face.cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            let press = match event.click_count > 1 {
                true => FilePress::OpenTwice,
                false => FilePress::Open,
            };
            file(&path, press, window, cx);
        })
        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
            menu(&menu_path, FilePress::Menu(event.position), window, cx);
        })
}

/// A sent message's look with the chips it wrote drawn in its text as the draft drew them; one
/// for a file opens it.
pub fn with_chips(
    look: Rc<gpui_transcript::Style>,
    message: &ChatMessage,
    file: super::files::FilePressed,
    bench: &WeakEntity<Workbench>,
    window: &mut Window,
) -> Rc<gpui_transcript::Style> {
    let mentions = mentions(message);
    if mentions.is_empty() {
        return look;
    }
    let chips = (mentions.into_iter())
        .map(|mention| {
            mention_chip(
                mention.written,
                mention.id,
                mention.label,
                Some(file.clone()),
                &look,
                bench,
                window,
            )
        })
        .collect::<Vec<_>>();
    Rc::new(gpui_transcript::Style {
        chips: chips.into(),
        ..(*look).clone()
    })
}

/// An agent's answer with each `@name` of an agent the app knows — open or closed — drawn as the
/// chip the draft draws for it.
fn with_named_agents(
    look: Rc<gpui_transcript::Style>,
    message: &ChatMessage,
    writer: &str,
    bench: &WeakEntity<Workbench>,
    window: &mut Window,
    cx: &App,
) -> Rc<gpui_transcript::Style> {
    let texts: Vec<&str> = (message.parts.iter())
        .filter_map(|part| match part {
            sikemux_view::chat::ChatPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .filter(|text| text.contains('@'))
        .collect();
    let Some(strong) = bench.upgrade().filter(|_| !texts.is_empty()) else {
        return look;
    };
    let read = strong.read(cx);
    let mut seen: HashSet<String> = HashSet::new();
    let mut chips: Vec<gpui_transcript::Chip> = named_agents(read, writer)
        .into_iter()
        .map(|(name, id)| (format!("@{name}"), id))
        .filter(|(written, _)| {
            texts.iter().any(|text| text.contains(written.as_str())) && seen.insert(written.clone())
        })
        .map(|(written, id)| {
            mention_chip(
                written.clone(),
                format!("{AGENT}{id}"),
                written,
                None,
                &look,
                bench,
                window,
            )
        })
        .collect();
    let from = read.chat_target(writer);
    for agent in &message.named {
        let written = format!("@{}", agent.name);
        if !texts.iter().any(|text| text.contains(written.as_str()))
            || !seen.insert(written.clone())
        {
            continue;
        }
        chips.push(closed_agent_chip(
            written,
            agent.clone(),
            from.clone(),
            &look,
            bench,
            window,
        ));
    }
    if chips.is_empty() {
        return look;
    }
    Rc::new(gpui_transcript::Style {
        chips: chips.into(),
        ..(*look).clone()
    })
}

/// The chip drawn in an answer's text for an agent resolved by the core that is no longer open;
/// clicking it reopens that agent's chat.
fn closed_agent_chip(
    written: String,
    agent: sikemux_view::chat::NamedAgent,
    from: crate::core::Target,
    look: &gpui_transcript::Style,
    bench: &WeakEntity<Workbench>,
    window: &mut Window,
) -> gpui_transcript::Chip {
    let (size, font) = (look.text_size, look.font.clone());
    let height = (size * look.line_height).floor() - px(4.);
    let mut style = window.text_style();
    style.font_family = font.clone();
    let run = style.to_run(written.len());
    let words = window
        .text_system()
        .shape_line(written.clone().into(), size, &[run], None)
        .width;
    let width = (words + px(CHIP_FRAME + 1.)).min(px(CHIP_WIDEST)).ceil();
    let label: SharedString = written.clone().into();
    let bench = bench.clone();
    gpui_transcript::Chip {
        written: written.into(),
        width,
        draw: Rc::new(move |_, cx| {
            let tint = Tokens::get(cx).text_tertiary;
            let glyph = if let Some(open) = bench
                .upgrade()
                .and_then(|strong| strong.read(cx).agent_by_id(&agent.agent_id))
            {
                open.mark(14., cx)
            } else if agent.name == "Chief" {
                crate::ui::chief_mark(14.)
            } else if agent.face.is_some() || agent.cli.is_some() {
                let cli = agent.cli.as_deref().unwrap_or_default();
                crate::ui::still_mark(
                    cli,
                    agent.face.as_ref(),
                    sikemux_wire::protocol::AgentGroup::Idle,
                    14.,
                    cx,
                )
            } else {
                ui::icon(Icon::Agent, 13.)
                    .text_color(tint)
                    .into_any_element()
            };
            let face = chip_face(label.clone(), None, false, glyph, cx)
                .w_full()
                .h(height);
            let face = opens_agent(face, &agent.agent_id, Some(from.clone()), &bench, cx);
            div()
                .size_full()
                .flex()
                .items_center()
                .text_size(size)
                .font_family(font.clone())
                .child(face)
                .into_any_element()
        }),
    }
}

/// The chip drawn in a message's text in place of `written`, as the draft drew it; an agent's
/// chip opens its chat.
fn mention_chip(
    written: String,
    id: String,
    label: String,
    file: Option<super::files::FilePressed>,
    look: &gpui_transcript::Style,
    bench: &WeakEntity<Workbench>,
    window: &mut Window,
) -> gpui_transcript::Chip {
    let (size, font) = (look.text_size, look.font.clone());
    let height = (size * look.line_height).floor() - px(4.);
    let measure = |text: &str| {
        let mut style = window.text_style();
        style.font_family = font.clone();
        let run = style.to_run(text.len());
        window
            .text_system()
            .shape_line(text.to_owned().into(), size, &[run], None)
            .width
    };
    let (key, title) = chip_words(&id, &label.into());
    let words = measure(&key)
        + title
            .as_ref()
            .map_or(px(0.), |title| px(CHIP_TITLE_GAP) + measure(title));
    let width = (words + px(CHIP_FRAME + 1.)).min(px(CHIP_WIDEST)).ceil();
    let bench = bench.clone();
    gpui_transcript::Chip {
        written: written.into(),
        width,
        draw: Rc::new(move |_, cx| {
            let face = chip_face(
                key.clone(),
                title.clone(),
                false,
                mark(&id, Tokens::get(cx).text_tertiary, &bench, cx),
                cx,
            )
            .w_full()
            .h(height);
            let face = match (id.strip_prefix(AGENT), id.strip_prefix(FILE), &file) {
                (Some(agent), ..) => opens_agent(face, agent, None, &bench, cx).into_any_element(),
                (_, Some(path), Some(file)) => {
                    let t = Tokens::get(cx);
                    let hover = theme::over(t.surface_active, t.surface_raised);
                    presses_file(face.hover(move |chip| chip.bg(hover)), path, file)
                        .into_any_element()
                }
                _ => face.into_any_element(),
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .text_size(size)
                .font_family(font.clone())
                .child(face)
                .into_any_element()
        }),
    }
}

/// An answer's look with each `why` link drawn as the chip the core checked it to be, which opens
/// what it names; a link the core did not check draws nothing.
pub fn with_why(
    look: Rc<gpui_transcript::Style>,
    message: &ChatMessage,
    writer: &str,
    bench: &WeakEntity<Workbench>,
    window: &mut Window,
    cx: &App,
) -> Rc<gpui_transcript::Style> {
    let look = with_named_agents(look, message, writer, bench, window, cx);
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
                            .clicks(move |_, _, cx| {
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

/// The agents a text by `writer` may name, `(name, id)`, its own project's first, so a name two
/// projects share is the writer's own.
pub(crate) fn named_agents(bench: &Workbench, writer: &str) -> Vec<(String, String)> {
    (bench.nameable_by(writer).into_iter())
        .filter_map(|(_, row)| Some((row.name?, row.id)))
        .collect()
}

/// What a sent message's text names, as the core read it.
fn mentions(message: &ChatMessage) -> Vec<Mention> {
    mentions_in(&message.spans)
}

/// What the person wrote in a message, with what it names as a draft's pills.
pub(super) fn written_with_pills(message: &ChatMessage) -> InputContent {
    let text = super::edit::written(message);
    let mut at = 0;
    let mut tokens = Vec::new();
    for span in &message.spans {
        let Some(found) = text.get(at..).and_then(|rest| rest.find(&span.text)) else {
            continue;
        };
        let range = at + found..at + found + span.text.len();
        at = range.end;
        let mention = (span.chip.as_ref()).and_then(|chip| mention_of(&span.text, chip));
        if let Some(mention) = mention {
            let pill = InlineToken::new(mention.id, span.text.clone()).with_label(mention.label);
            tokens.push((range, pill));
        }
    }
    pills::with_tokens(text, tokens)
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
        SentChipKind::Issue => format!(
            "{TRACKED}issue:{}:{carried}",
            written.get(1..).unwrap_or_default()
        ),
        SentChipKind::Pull => format!(
            "{TRACKED}pull:{}:{carried}",
            written.get(1..).unwrap_or_default()
        ),
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
    if id.starts_with(AGENT) {
        return (label.clone(), None);
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
            true => flashed(tile, path, &t).into_any_element(),
            false => tile.into_any_element(),
        }
    });
    let shown = context.iter().enumerate();
    let chips = shown
        .filter(|(_, item)| !item.for_agent_only())
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
            .children(tiles)
            .children(chips),
    )
}

/// How long after a file is attached its tile still flashes when it first draws.
pub const FRESH: std::time::Duration = std::time::Duration::from_millis(800);

/// A tile just attached, its edge lit in the accent and fading back to its border.
fn flashed(tile: Attachment, path: &Path, t: &Tokens) -> impl IntoElement {
    let id = SharedString::from(format!("attachment-flash-{}", path.to_string_lossy()));
    let (accent, border) = (t.accent, t.border);
    tile.with_animation(
        id,
        crate::motion::entrance(crate::motion::ATTACHED),
        move |tile, shown| tile.border_color(theme::mix(accent, border, 1. - shown)),
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
        .tooltip(path.to_string_lossy().into_owned())
        .media(media(path, t, cx).h(px(FRAMED_PREVIEW)))
        .content(AttachmentContent::new().child(caption(number, name, cx)))
}

const FRAMED_WIDTH: f32 = 64.;
const FRAMED_PREVIEW: f32 = 40.;

/// The line under a draft's framed card: a picture's `#n`, then the file's name.
fn caption(number: Option<usize>, name: String, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .min_w_0()
        .text_size(px(theme::TEXT_3XS))
        .children(number.map(|number| ui::mono(format!("#{number}"), 9., cx).flex_none()))
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
/// earlier chat or anything else.
pub(crate) fn context_chip(
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
        None if uri.starts_with(ARTIFACT_URI) => (Icon::Presentation, title.to_owned()),
        None if uri.starts_with(STASH_URI) => (Icon::Text, title.to_owned()),
        None if uri.starts_with(SIDE_URI) => (Icon::Message, title.to_owned()),
        None if web => (
            Icon::Globe,
            if title.is_empty() { uri } else { title }.to_owned(),
        ),
        None if title.is_empty() => (Icon::File, uri.to_owned()),
        None => (Icon::File, title.to_owned()),
    };
    let tooltip = match uri.starts_with(ARTIFACT_URI) {
        true => title.clone(),
        false => uri.to_owned(),
    };
    Attachment::new()
        .id(SharedString::from(id))
        .with_size(gpui_kit::component::Size::Small)
        .tooltip(tooltip)
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
    page: impl Fn(&str) -> super::page::Opens,
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
            .any(|mention| mention.carried == carried && !is_image(Path::new(carried)))
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
            let chip = context_chip(
                format!("sent-context-{}-{index}", message.id),
                &item.uri,
                quote.as_deref().unwrap_or(&item.title),
                None,
                &t,
            );
            match item.uri.strip_prefix(ARTIFACT_URI) {
                Some(id) => {
                    let open = page(id);
                    chip.on_click(move |_, window, cx| open(window, cx))
                }
                None => chip,
            }
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
        return presses_file(div(), &path.to_string_lossy(), &file)
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
        .border(crate::theme::border())
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

/// The icon of a file named `name`'s kind.
fn file_icon(name: &str, t: &Tokens, cx: &App) -> gpui_kit::AnyElement {
    crate::file_icons::icon(&SharedString::from(name.to_owned()), cx)
        .map(|icon| gpui_kit::img(icon).size(px(14.)).into_any_element())
        .unwrap_or_else(|| {
            ui::icon(Icon::File, 13.)
                .text_color(t.text_tertiary)
                .into_any_element()
        })
}

/// A file as a small chip like a context item's: its kind's icon and its name.
pub(crate) fn file_chip(id: String, path: &Path, t: &Tokens, cx: &App) -> Attachment {
    let name = crate::ui::path::file_name(path);
    let icon = file_icon(&name, t, cx);
    Attachment::new()
        .id(SharedString::from(id))
        .with_size(gpui_kit::component::Size::Small)
        .tooltip(path.to_string_lossy().into_owned())
        .media(AttachmentMedia::new().child(icon))
        .content(AttachmentContent::new().title(AttachmentTitle::new(name)))
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
    let icon = file_icon(&name, t, cx);
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
        .border(crate::theme::border())
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
