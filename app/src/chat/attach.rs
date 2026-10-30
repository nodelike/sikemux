//! The files a message carries: picked, dropped on the chat or pasted, shown as tiles over
//! the draft, each picture also named in the draft where it belongs, as `[Image #n]`.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::base::input::{InlineToken, InlineTokenContext};
use gpui_kit::component::attachment::{
    Attachment, AttachmentContent, AttachmentMedia, AttachmentTitle,
};
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, App, AppContext, Axis, ClipboardEntry, ClipboardItem, Context, Div, ExternalPaths,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    PathPromptOptions, SharedString, Styled, StyledImage, WeakEntity, Window,
};
use gpui_transcript::FilePress;
use sikemux_core::protocol::MAX_ATTACHMENT_BYTES;
use sikemux_core::server::chat::prompt::MAX_ATTACHMENTS;
use sikemux_wire::chat::{ChatMessage, SentContext};
use sikemux_wire::protocol::ChatContext;
use tokio::sync::oneshot;

use super::Composing;
use crate::core::ChatAsk;
use crate::drag::DraggedPath;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2H, TEXT_SM};
use crate::ui;
use crate::workbench::{AgentRow, Workbench};

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
        (id.strip_prefix("agent:")).and_then(|agent| bench.upgrade()?.read(cx).agent_by_id(agent));
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
    if id.starts_with("agent:") {
        return ui::icon(Icon::Agent, 13.)
            .text_color(tint)
            .into_any_element();
    }
    if id.starts_with("command:") {
        return ui::icon(Icon::Bolt, 13.)
            .text_color(tint)
            .into_any_element();
    }
    if id.starts_with("earlier:") {
        return ui::icon(Icon::Message, 13.)
            .text_color(tint)
            .into_any_element();
    }
    if let Some(kind) = id.strip_prefix("tracked:") {
        let icon = if kind.starts_with("pull") {
            Icon::PullRequest
        } else {
            Icon::Issue
        };
        return ui::icon(icon, 13.).text_color(tint).into_any_element();
    }
    if id.starts_with("folder:") {
        return ui::icon(Icon::Folder, 13.)
            .text_color(tint)
            .into_any_element();
    }
    let path = Path::new(id.strip_prefix("file:").unwrap_or(id));
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

fn image_tag(number: usize, path: &Path) -> InlineToken {
    InlineToken::new(
        format!("{IMAGE}{}", path.to_string_lossy()),
        format!("[Image #{number}]"),
    )
    .with_label(format!("Image {number}"))
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
            .open
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
                            "{name} is over {} MB, too large to send to the chat",
                            MAX_KEPT / (1024 * 1024)
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
            Ok(Err(error)) => {
                let note = format!("The picture or file could not be attached: {error}");
                cx.update(|window, cx| window.push_notification(note, cx))
                    .ok();
            }
            Err(_) => {}
        })
        .detach();
    }

    /// Takes context item `index`, an earlier chat or an issue, off the message.
    pub fn forget(&mut self, agent: &str, index: usize, cx: &mut Context<Self>) {
        if let Some(composing) = self
            .chats
            .open
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
            .open
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
                    let (stamp, agent, image) =
                        (stamp.to_string(), agent.to_owned(), image.clone());
                    let readable =
                        cx.background_spawn(async move { crate::pasted::readable(image) });
                    cx.spawn_in(window, async move |this, cx| {
                        let image = readable.await;
                        let name = format!("{stamp}.{}", image.format.extension());
                        this.update_in(cx, |bench, window, cx| {
                            let mime = image.format.mime_type();
                            bench.keep(&agent, name, mime, image.bytes, window, cx)
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
                TEXT_SM,
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
        let named = (id.strip_prefix("agent:")).and_then(|agent| bench.agent_by_id(agent));
        mark_of(id, Tokens::get(cx).text_tertiary, named, cx)
    });
    chip_face(key, title, false, mark, cx)
}

/// A sent message's look with the chips it wrote drawn in its text as the draft drew them.
pub fn with_chips(
    look: Rc<gpui_transcript::Style>,
    message: &ChatMessage,
    agent: &str,
    bench: &WeakEntity<Workbench>,
    window: &mut Window,
    cx: &App,
) -> Rc<gpui_transcript::Style> {
    let (agents, commands) = bench
        .upgrade()
        .map(|bench| {
            let bench = bench.read(cx);
            let commands: Vec<String> = bench
                .chats
                .open
                .get(agent)
                .and_then(|chat| chat.state.as_ref())
                .map(|head| {
                    head.commands
                        .iter()
                        .map(|command| command.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            (named_agents(bench), commands)
        })
        .unwrap_or_default();
    let mentions = mentions(message, &agents, &commands);
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

/// What a sent message's text names, as the draft's chips wrote it. Agents and commands
/// are named only in the text, so `agents` and `commands` say which words they are.
fn mentions(
    message: &ChatMessage,
    agents: &[(String, String)],
    commands: &[String],
) -> Vec<Mention> {
    let written = super::edit::written(message);
    named(
        &written,
        &message.attachments,
        &message.context,
        agents,
        commands,
    )
}

/// What `written` names, as a draft's chips wrote it, of the files and context it went with.
pub(crate) fn named(
    written: &str,
    attachments: &[String],
    context: &[SentContext],
    agents: &[(String, String)],
    commands: &[String],
) -> Vec<Mention> {
    let mut found: Vec<Mention> = agents
        .iter()
        .map(|(name, id)| Mention {
            written: format!("@{name}"),
            id: format!("agent:{id}"),
            label: format!("@{name}"),
            carried: format!("sikemux://agent/{id}"),
        })
        .chain(commands.iter().map(|name| Mention {
            written: format!("/{name}"),
            id: format!("command:{name}"),
            label: format!("/{name}"),
            carried: String::new(),
        }))
        .collect();
    for (index, path) in attachments.iter().enumerate() {
        let number = index + 1;
        if is_image(Path::new(path)) {
            found.push(Mention {
                written: format!("[Image #{number}]"),
                id: format!("{IMAGE}{path}"),
                label: format!("Image {number}"),
                carried: path.clone(),
            });
            continue;
        }
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let tag = (1..=parts.len())
            .rev()
            .map(|from| format!("+{}", parts[parts.len() - from..].join("/")))
            .find(|tag| written.contains(tag.as_str()));
        if let Some(tag) = tag {
            let kind = if Path::new(path).is_dir() {
                "folder"
            } else {
                "file"
            };
            found.push(Mention {
                written: tag.clone(),
                id: format!("{kind}:{path}"),
                label: tag,
                carried: path.clone(),
            });
        }
    }
    for item in context {
        let named = item.title.starts_with('@');
        let mention = item
            .uri
            .strip_prefix("sikemux://earlier/")
            .filter(|_| named)
            .map(|chat| format!("earlier:{chat}"));
        if let Some(id) = mention {
            found.push(Mention {
                written: item.title.clone(),
                id,
                label: item.title.clone(),
                carried: item.uri.clone(),
            });
        } else if let Some((icon, number)) = tracked(&item.uri) {
            let (sign, kind) = if matches!(icon, Icon::PullRequest) {
                ('!', "pull")
            } else {
                ('#', "issue")
            };
            found.push(Mention {
                written: format!("{sign}{number}"),
                id: format!("tracked:{kind}:{number}:{}", item.uri),
                label: format!("#{number} {}", tracked_title(&item.title, number)),
                carried: item.uri.clone(),
            });
        }
    }
    found.retain(|mention| written.contains(mention.written.as_str()));
    found
}

/// What a chip says: its handle without the sign typed to make it, and for an issue or pull
/// request its title after the number, without a `feat(scope):` kind of prefix.
fn chip_words(id: &str, label: &SharedString) -> (SharedString, Option<SharedString>) {
    if id.starts_with("tracked:") {
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
pub fn tiles(
    attachments: &[PathBuf],
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
        let name = match is_image(path) {
            true => format!("#{} {}", index + 1, file_name(path)),
            false => file_name(path).to_string(),
        };
        Attachment::new()
            .id(SharedString::from(format!("attachment-{index}")))
            .when_some(opens, |tile, opens| tile.on_click(opens))
            .with_size(gpui_kit::component::Size::Small)
            .tooltip(path.to_string_lossy().into_owned())
            .media(chip_media(path, &t, cx))
            .content(AttachmentContent::new().title(AttachmentTitle::new(name)))
            .on_remove(move |_, window, cx| remove(index, window, cx))
    });
    let chips = context.iter().enumerate().map(|(index, item)| {
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
            .pt(SPACE_2H)
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
/// A file's mark in a draft's chip: a picture's own thumbnail, else its kind's icon.
fn chip_media(path: &Path, t: &Tokens, cx: &App) -> AttachmentMedia {
    if is_image(path) {
        return AttachmentMedia::new()
            .src(crate::picture::Picture::File(path.to_path_buf()).source_at(px(48.)));
    }
    let icon = crate::file_icons::icon(&file_name(path), cx)
        .map(|icon| gpui_kit::img(icon).size(px(14.)).into_any_element());
    AttachmentMedia::new().child(icon.unwrap_or_else(|| {
        ui::icon(Icon::File, 13.)
            .text_color(t.text_tertiary)
            .into_any_element()
    }))
}

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
            .px(SPACE_1)
            .py(SPACE_0)
            .rounded(theme::RADIUS_1)
            .bg(Hsla {
                a: 0.72,
                ..t.surface_sunken
            })
            .child(ui::mono(format!("#{number}"), 9.5, cx).text_color(t.text_primary)),
    )
}

/// An issue's or pull request's number in its link, when the link is to one.
/// An issue's or pull request's title without the kind and number its context names it by.
fn tracked_title<'a>(title: &'a str, number: &str) -> &'a str {
    let bare = title.trim_start_matches("Issue ");
    let bare = bare.trim_start_matches("Pull request ");
    bare.strip_prefix(&format!("#{number}"))
        .map_or(title, |rest| rest.trim_start_matches(':').trim_start())
}

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
        None if uri.starts_with("sikemux://earlier/") => (Icon::Message, title.to_owned()),
        None if uri.starts_with("sikemux://stash/") => (Icon::Text, title.to_owned()),
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
        .tooltip(uri.to_owned())
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
    let inline = mentions(message, &[], &[]);
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
    let context = message.context.iter().enumerate();
    let context: Vec<_> = context
        .filter(|(_, item)| !in_text(&item.uri))
        .map(|(index, item)| {
            context_chip(
                format!("sent-context-{}-{index}", message.id),
                &item.uri,
                &item.title,
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
