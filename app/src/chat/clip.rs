//! What was cut or copied from a draft with pills in it, so pasting into any draft brings the
//! files and the rest the pills stand for. The clipboard itself holds the plain text and the
//! clip's id alone: another program can at most have the person's own last clip pasted again,
//! and never names a file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use base64::Engine as _;
use gpui_kit::base::input::InlineTokenSpan;
use gpui_kit::component::input::TextareaState;
use gpui_kit::{
    App, AppContext, ClipboardEntry, ClipboardItem, Context, Entity, Global, WeakEntity, Window,
};
use serde::{Deserialize, Serialize};
use sikemux_calls::call;
use sikemux_git::files::FileBytesParams;

use super::attach::{about_uri, mime_of, MAX_KEPT};
use super::draft::{restored, Kept};
use super::pills::{self, path_in, Files};
use super::Sent;
use crate::core::Target;
use crate::handover;
use crate::hosts::Place;
use crate::workbench::Workbench;

/// How much of a file on another host is read at once.
const PIECE: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Clip {
    id: String,
    /// Where its files are.
    place: Place,
    draft: Kept,
}

/// The last clip. It is also written down, so it outlives the app as the clipboard does.
#[derive(Default)]
struct LastClip(Option<Rc<Clip>>);

impl Global for LastClip {}

fn saved_path() -> Option<PathBuf> {
    Some(sikemux_process::paths::config_dir()?.join("clip.json"))
}

impl Clip {
    fn keep(self, cx: &mut App) {
        if let Ok(bytes) = serde_json::to_vec(&self) {
            cx.background_spawn(async move {
                if let Err(error) = handover::write(&saved_path(), &bytes) {
                    eprintln!("sikemux: what was copied was not saved: {error}");
                }
            })
            .detach();
        }
        cx.set_global(LastClip(Some(Rc::new(self))));
    }

    /// The clip `item` says it is, when Sikemux kept that one.
    fn of(item: &ClipboardItem, cx: &mut App) -> Option<Rc<Self>> {
        let [ClipboardEntry::String(written)] = item.entries() else {
            return None;
        };
        let id = written.metadata_json::<String>();
        if id.is_none() && !cfg!(target_os = "linux") {
            return None;
        }
        // The clipboard on Linux carries the text alone.
        let is_it = |clip: &Self| {
            id.as_ref().is_none_or(|id| *id == clip.id) && clip.draft.text() == written.text()
        };
        let last = cx.default_global::<LastClip>().0.clone();
        if let Some(clip) = last.filter(|clip| is_it(clip)) {
            return Some(clip);
        }
        let saved: Self = serde_json::from_slice(&std::fs::read(saved_path()?).ok()?).ok()?;
        if !is_it(&saved) {
            return None;
        }
        let clip = Rc::new(saved);
        cx.set_global(LastClip(Some(clip.clone())));
        Some(clip)
    }
}

/// A draft with pills in it.
#[derive(Clone)]
pub(super) enum Field {
    /// The composer so named.
    Composer(String),
    /// The sent message being written again in the chat of the agent so named.
    Edit(String),
}

impl Field {
    fn owner(&self) -> &str {
        match self {
            Self::Composer(owner) | Self::Edit(owner) => owner,
        }
    }
}

/// The part of what `draft` selects, when a pill is in it: its text as it stands there, and it
/// with what its pills stand for out of `all`.
fn selected(draft: &TextareaState, all: Sent) -> Option<(String, Sent)> {
    let range = draft.selected_range();
    let content = draft.content();
    let text = content.text().get(range.clone())?.to_owned();
    let spans = content.tokens().iter().filter_map(|span| {
        let held = span.range();
        let within = range.start <= held.start && held.end <= range.end;
        within.then(|| {
            let at = held.start - range.start..held.end - range.start;
            (at, span.token().clone())
        })
    });
    let part = pills::with_tokens(text.clone(), spans);
    if part.tokens().is_empty() {
        return None;
    }
    let named = |path: &Path| {
        (part.tokens().iter().map(InlineTokenSpan::token))
            .any(|token| path_in(token.id()).is_some_and(|held| Path::new(held) == path))
    };
    let attachments: Vec<PathBuf> = (all.attachments.into_iter())
        .filter(|path| named(path))
        .collect();
    let context = (all.context.into_iter())
        .filter(|note| (attachments.iter()).any(|path| note.uri == about_uri(path)))
        .collect();
    let sent = Sent {
        content: pills::numbered(part, &attachments),
        attachments,
        context,
        tracked_context: all.tracked_context,
    };
    Some((text, sent))
}

/// Puts `clip`, whose files are now where `at` says, in place of what `draft` selects. A pill
/// whose file did not come, or that goes past what a message carries, leaves its words alone.
fn land(
    draft: &Entity<TextareaState>,
    files: &mut Files,
    clip: &Sent,
    at: impl Fn(&Path) -> Option<PathBuf>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut held: Vec<_> = (draft.read(cx).tokens().iter())
        .map(|span| span.token().clone())
        .collect();
    let content = pills::rewritten(&clip.content, |token| {
        let token = match path_in(token.id()) {
            None => token.clone(),
            Some(path) => {
                let path = Path::new(path);
                let now = at(path)?;
                let number = match clip.attachments.iter().any(|file| file == path) {
                    true => files.add(&now)?,
                    false => 0,
                };
                pills::moved(token, &now, number, &held)
            }
        };
        held.push(token.clone());
        Some(token)
    });
    draft.update(cx, |draft, cx| {
        let start = draft.selected_range().start;
        draft.replace(content.text().clone(), window, cx);
        for span in content.tokens().iter().rev() {
            let range = span.range();
            let range = start + range.start..start + range.end;
            if let Err(error) =
                draft.replace_range_with_token(range, span.token().clone(), window, cx)
            {
                eprintln!("sikemux: a pasted pill did not go into the draft: {error}");
            }
        }
        let end = start + content.text().len();
        draft.set_selected_range(end..end, cx);
    });
}

/// The bytes of file `path` at `from`.
async fn bytes_of(from: &Target, path: &Path, cx: &gpui_kit::AsyncApp) -> Result<Vec<u8>, String> {
    let too_large = || {
        let megabytes = MAX_KEPT / (1024 * 1024);
        t!("it is over {megabytes} MB", megabytes = megabytes)
    };
    if *from.place() == Place::This {
        let path = path.to_owned();
        let read = cx.background_spawn(async move {
            let size = std::fs::metadata(&path)?.len();
            (size <= MAX_KEPT).then(|| std::fs::read(&path)).transpose()
        });
        let bytes = read.await.map_err(|error| error.to_string())?;
        return bytes.ok_or_else(too_large);
    }
    let mut bytes = Vec::new();
    loop {
        let piece = from
            .request_typed::<call::FilesBytes>(FileBytesParams {
                path: path.to_string_lossy().into_owned(),
                at: bytes.len() as u64,
                length: PIECE,
            })
            .await?;
        if piece.size > MAX_KEPT {
            return Err(too_large());
        }
        let read = base64::engine::general_purpose::STANDARD
            .decode(piece.base64)
            .map_err(|error| error.to_string())?;
        bytes.extend_from_slice(&read);
        if read.is_empty() || bytes.len() as u64 >= piece.size {
            return Ok(bytes);
        }
    }
}

/// Keeps what is cut or copied from `field` with what its pills stand for, once the draft has
/// put its text on the clipboard.
pub(super) fn copies<A: gpui::Action>(
    field: Field,
    bench: WeakEntity<Workbench>,
) -> impl Fn(&A, &mut Window, &mut App) {
    move |_, _, cx| {
        let part = bench.update(cx, |bench, cx| {
            let (draft, all) = bench.held_in(&field, cx)?;
            let draft = draft.read(cx);
            let (text, sent) = (draft.is_copyable())
                .then(|| selected(draft, all))
                .flatten()?;
            Some((text, sent, bench.chat_target(field.owner()).place().clone()))
        });
        let Ok(Some((text, sent, place))) = part else {
            return;
        };
        cx.defer(move |cx| {
            let written = cx.read_from_clipboard().and_then(|item| item.text());
            if written.as_ref() != Some(&text) {
                return;
            }
            let clip = Clip {
                id: uuid::Uuid::new_v4().to_string(),
                place,
                draft: Kept::of(sent),
            };
            let text = clip.draft.text().to_owned();
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(text, &clip.id));
            clip.keep(cx);
        });
    }
}

/// What a message being written again takes in place of its paste, for a clip of a draft.
pub(super) fn on_paste(
    field: Field,
    bench: WeakEntity<Workbench>,
) -> impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool {
    move |item, window, cx| {
        bench
            .update(cx, |bench, cx| bench.paste_clip(&field, item, window, cx))
            .unwrap_or(false)
    }
}

impl Workbench {
    fn held_in(&self, field: &Field, cx: &App) -> Option<(Entity<TextareaState>, Sent)> {
        match field {
            Field::Composer(owner) => {
                let composing = self.composing(owner)?;
                Some((composing.draft.clone(), composing.held(cx)))
            }
            Field::Edit(agent) => {
                let reading = self.chats.open.get(agent)?.reader.read(cx);
                let (draft, files) = reading.editing.as_ref()?.held();
                let sent = Sent {
                    content: draft.read(cx).content(),
                    attachments: files.to_vec(),
                    context: Vec::new(),
                    tracked_context: HashMap::new(),
                };
                Some((draft.clone(), sent))
            }
        }
    }

    /// Pastes the clip of a draft that `item` is into `field`; whether it is one.
    pub(super) fn paste_clip(
        &mut self,
        field: &Field,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(clip) = Clip::of(item, cx) else {
            return false;
        };
        let sent = restored(&clip.draft);
        let to = self.chat_target(field.owner());
        if *to.place() == clip.place {
            let here = clip.place == Place::This;
            let gone: Vec<_> = (sent.attachments.iter())
                .filter(|path| here && !path.exists())
                .collect();
            if !gone.is_empty() {
                let note = t!("{name} is no longer there", name = names(&gone));
                self.notify(crate::toast::error(note), cx);
            }
            let at = |path: &Path| (!gone.iter().any(|it| *it == path)).then(|| path.to_owned());
            self.land_clip(field, &sent, at, window, cx);
            return true;
        }
        let from = Target::new(&self.asks, clip.place.clone());
        if !sent.attachments.is_empty() {
            let name = names(&sent.attachments.iter().collect::<Vec<_>>());
            self.notify(
                crate::toast::info(t!("Bringing over {name}", name = name)),
                cx,
            );
        }
        let field = field.clone();
        cx.spawn_in(window, async move |bench, cx| {
            let mut moved = HashMap::new();
            for path in &sent.attachments {
                let brought = async {
                    let bytes = bytes_of(&from, path, cx).await?;
                    let kept = bench
                        .update(cx, |bench, _| {
                            let name = crate::ui::path::file_name(path);
                            bench.store(field.owner(), name, mime_of(path), bytes)
                        })
                        .map_err(|error| error.to_string())?;
                    kept.await.map_err(|error| error.to_string())?
                };
                match brought.await {
                    Ok(now) => {
                        moved.insert(path.clone(), now);
                    }
                    Err(error) => {
                        let name = names(&[path]);
                        let note = t!(
                            "{name} did not come along: {error}",
                            name = name,
                            error = error
                        );
                        let _ = bench.update(cx, |bench, cx| {
                            bench.notify(crate::toast::error(note), cx);
                        });
                    }
                }
            }
            let _ = bench.update_in(cx, |bench, window, cx| {
                let at = |path: &Path| moved.get(path).cloned();
                bench.land_clip(&field, &sent, at, window, cx);
            });
        })
        .detach();
        true
    }

    fn land_clip(
        &mut self,
        field: &Field,
        clip: &Sent,
        at: impl Fn(&Path) -> Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match field {
            Field::Composer(owner) => {
                let Some(composing) = self.composing_mut(owner) else {
                    return;
                };
                let draft = composing.draft.clone();
                land(&draft, &mut composing.attachments, clip, &at, window, cx);
                composing.settle(cx);
                for path in &clip.attachments {
                    let Some(now) = at(path).filter(|now| composing.attachments.contains(now))
                    else {
                        continue;
                    };
                    let (old, uri) = (about_uri(path), about_uri(&now));
                    let (was, is) = (path.to_string_lossy(), now.to_string_lossy());
                    for note in clip.context.iter().filter(|note| note.uri == old) {
                        if composing.context.iter().all(|held| held.uri != uri) {
                            let mut note = note.clone();
                            note.uri = uri.clone();
                            note.text = note.text.replace(&*was, &is);
                            composing.context.push(note);
                        }
                    }
                }
                let tracked = clip.tracked_context.clone();
                composing.tracked_context.extend(tracked);
                self.typed_draft(owner, cx);
            }
            Field::Edit(agent) => {
                let Some(chat) = self.chats.open.get(agent) else {
                    return;
                };
                chat.reader.update(cx, |reading, cx| {
                    if let Some(editing) = &mut reading.editing {
                        editing.land(
                            |draft, files, cx| land(draft, files, clip, &at, window, cx),
                            cx,
                        );
                    }
                });
            }
        }
        cx.notify();
    }
}

fn names(paths: &[&PathBuf]) -> String {
    (paths.iter())
        .map(|path| crate::ui::path::file_name(path))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;

    /// A draft holding `text`, with a pill at the end for each of `paths` after it.
    fn draft(
        cx: &mut VisualTestContext,
        text: &str,
        paths: &[&str],
    ) -> (Entity<TextareaState>, Files) {
        let mut files = Files::default();
        let draft = cx.update(|window, cx| {
            let draft = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 12));
            draft.update(cx, |draft, cx| draft.replace(text.to_owned(), window, cx));
            for path in paths {
                let path = Path::new(path);
                let number = files.add(path).unwrap();
                draft.update(cx, |draft, cx| {
                    draft.replace(" ", window, cx);
                    let pill = pills::pill_of(path, number, draft.tokens());
                    draft.replace_with_token(pill, window, cx).unwrap();
                });
            }
            draft
        });
        (draft, files)
    }

    fn named(draft: &Entity<TextareaState>, cx: &App) -> Vec<(String, String)> {
        (draft.read(cx).tokens().iter())
            .map(|span| {
                (
                    span.token().id().to_string(),
                    span.token().text().to_string(),
                )
            })
            .collect()
    }

    #[gpui::test]
    fn a_cut_draft_pastes_with_its_files(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let cx = cx.add_empty_window();
        let (from, held) = draft(cx, "Like this.", &["/shots/a.png", "/docs/README.md"]);
        let (to, mut files) = draft(cx, "Not this", &["/shots/b.png", "/other/README.md"]);
        let item = cx.update(|_, cx| {
            from.update(cx, |draft, cx| {
                draft.set_selected_range(0..draft.value().len(), cx)
            });
            let all = Sent {
                content: from.read(cx).content(),
                attachments: held.to_vec(),
                context: Vec::new(),
                tracked_context: HashMap::new(),
            };
            let (text, part) = selected(from.read(cx), all).unwrap();
            let clip = Clip {
                id: "ours".to_owned(),
                place: Place::This,
                draft: Kept::of(part),
            };
            let saved = serde_json::to_vec(&clip).unwrap();
            cx.set_global(LastClip(Some(Rc::new(
                serde_json::from_slice(&saved).unwrap(),
            ))));
            ClipboardItem::new_string_with_json_metadata(text, "ours")
        });
        let lookalikes = [
            ClipboardItem::new_string_with_json_metadata("[Image #1]".to_owned(), "ours"),
            ClipboardItem::new_string_with_json_metadata(item.text().unwrap(), "theirs"),
            ClipboardItem::new_string("Like this. [Image #9]".to_owned()),
        ];
        cx.update(|window, cx| {
            for item in &lookalikes {
                assert!(Clip::of(item, cx).is_none());
            }
            let clip = restored(&Clip::of(&item, cx).unwrap().draft);
            land(
                &to,
                &mut files,
                &clip,
                |path| Some(path.to_owned()),
                window,
                cx,
            );
            files.settle(to.read(cx).tokens(), &mut Vec::new());
            assert_eq!(
                named(&to, cx),
                [
                    ("image:/shots/b.png", "[Image #1]"),
                    ("file:/other/README.md", "+README.md"),
                    ("image:/shots/a.png", "[Image #3]"),
                    ("file:/docs/README.md", "+docs/README.md"),
                ]
                .map(|(id, text)| (id.to_owned(), text.to_owned()))
            );
            let sent = pills::numbered(to.read(cx).content(), &files);
            assert!(sent
                .text()
                .ends_with("Like this. [Image #2] +docs/README.md"));
        });
    }
}
