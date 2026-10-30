//! Who last changed the line the cursor is on, written quietly at its end.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, RopeExt};
use gpui_kit::{
    canvas, point, px, Context, Entity, FontStyle, Hsla, IntoElement, Pixels, SharedString, Styled,
    Task, TextAlign, TextRun,
};
use serde_json::{json, Value};

use super::{lsp, FileKey};
use crate::theme;
use crate::workbench::Workbench;

/// How long typing pauses before the lines are blamed again.
const DEBOUNCE: Duration = Duration::from_millis(1200);
/// The most characters a line's note shows.
const MAX_NOTE: usize = 60;
/// How far right of the line's end the note starts.
const GAP: f32 = 26.;

/// Each line's last commit, as a short note.
#[derive(Default)]
pub(super) struct Blame {
    notes: Rc<Vec<Note>>,
    lines: Rc<Vec<u32>>,
    /// Typing since the last read leaves the notes unshown until the next.
    typing: bool,
    reading: Option<Task<()>>,
}

pub(super) struct Note {
    text: SharedString,
    uncommitted: bool,
}

fn note(commit: &Value) -> Note {
    let uncommitted = commit["uncommitted"].as_bool().unwrap_or(false);
    let text = match uncommitted {
        true => "You · Uncommitted changes".to_owned(),
        false => format!(
            "{}, {} · {}",
            commit["author"].as_str().unwrap_or_default(),
            commit["time"].as_str().unwrap_or_default(),
            commit["summary"].as_str().unwrap_or_default()
        ),
    };
    let text = match text.chars().count() > MAX_NOTE {
        true => format!("{}…", text.chars().take(MAX_NOTE - 1).collect::<String>()),
        false => text,
    };
    Note {
        text: text.into(),
        uncommitted,
    }
}

/// Blames the editor's text now, as it stands.
pub(super) fn read(bench: &mut Workbench, key: &FileKey, cx: &mut Context<Workbench>) {
    blame(bench, key, None, cx);
}

/// Hides the note while typing and blames again once it pauses.
pub(super) fn typed(bench: &mut Workbench, key: &FileKey, cx: &mut Context<Workbench>) {
    bench.editors.update(cx, |editors, _| {
        if let Some(document) = editors.open.get_mut(key) {
            document.blame.typing = true;
        }
    });
    blame(bench, key, Some(DEBOUNCE), cx);
}

fn blame(
    bench: &mut Workbench,
    key: &FileKey,
    wait: Option<Duration>,
    cx: &mut Context<Workbench>,
) {
    let Some(project) = lsp::project_of(bench, &key.path) else {
        return;
    };
    let Some(relative) = key
        .path
        .strip_prefix(&project)
        .ok()
        .map(|relative| relative.to_string_lossy().into_owned())
    else {
        return;
    };
    bench.editors.update(cx, |editors, cx| {
        let Some(document) = editors.open.get_mut(key) else {
            return;
        };
        let (state, target, key) = (document.state.clone(), document.target.clone(), key.clone());
        document.blame.reading = Some(cx.spawn(async move |editors, cx| {
            if let Some(wait) = wait {
                cx.background_executor().timer(wait).await;
            }
            let contents = state.read_with(cx, |state, _| state.value().to_string());
            let asked = json!({ "repo": project, "path": relative, "contents": contents });
            let Ok(blamed) = target.request("git.blame", asked).await else {
                return;
            };
            let notes: Vec<Note> = blamed["commits"]
                .as_array()
                .into_iter()
                .flatten()
                .map(note)
                .collect();
            let lines: Vec<u32> = blamed["lines"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|index| index.as_u64().unwrap_or(0) as u32)
                .collect();
            let _ = editors.update(cx, |editors, cx| {
                if let Some(document) = editors.open.get_mut(&key) {
                    document.blame.notes = Rc::new(notes);
                    document.blame.lines = Rc::new(lines);
                    document.blame.typing = false;
                    cx.notify();
                }
            });
        }));
    });
}

/// The note at the end of the cursor's line, while nothing is selected and
/// nothing typed since the lines were last blamed.
pub(super) fn overlay(
    state: &Entity<EditorState>,
    blame: &Blame,
    text_size: Pixels,
    ink: Hsla,
) -> impl IntoElement {
    let (state, notes, lines, typing) = (
        state.clone(),
        blame.notes.clone(),
        blame.lines.clone(),
        blame.typing,
    );
    canvas(
        |_, _, _| (),
        move |_, _, window, cx| {
            if typing {
                return;
            }
            let state = state.read(cx);
            if !state.selected_range().is_empty() {
                return;
            }
            let line = state.cursor_position().line;
            let Some(note) = lines
                .get(line as usize)
                .and_then(|index| notes.get(*index as usize))
            else {
                return;
            };
            let text = state.text();
            let end = text.line_end_offset(line as usize);
            let Some(bounds) = state.range_to_bounds(&(end..end)) else {
                return;
            };
            let mut font = gpui_kit::font(theme::MONO);
            font.style = FontStyle::Italic;
            let colour = Hsla {
                a: if note.uncommitted { ink.a * 0.7 } else { ink.a },
                ..ink
            };
            let run = TextRun {
                len: note.text.len(),
                font,
                color: colour,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped =
                window
                    .text_system()
                    .shape_line(note.text.clone(), text_size * 0.92, &[run], None);
            let origin = point(bounds.right() + px(GAP), bounds.top());
            let _ = shaped.paint(
                origin,
                bounds.size.height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        },
    )
    .absolute()
    .size_full()
}
