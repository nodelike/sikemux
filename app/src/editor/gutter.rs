//! What changed in a file since its last commit, marked beside its lines and
//! along the editor's right edge, which jumps there when clicked.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, Position, RopeExt};
use gpui_kit::{
    canvas, div, fill, point, px, relative, App, Bounds, Context, Entity, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Task,
};

use super::{lsp, place_cursor, Document, Editors, FileKey, OpenAt};
use crate::git::{self, DiffKind, Hunk};
use crate::theme::Tokens;
use crate::workbench::Workbench;

/// How long typing pauses before the marks are worked out again.
const DEBOUNCE: Duration = Duration::from_millis(500);
/// A file longer than this, in characters, is not marked.
const MAX_CHARACTERS: usize = 256 * 1024;

/// A file's text at its last commit, and how the editor's text differs.
#[derive(Default)]
pub(super) struct Gutter {
    base: Option<SharedString>,
    hunks: Rc<Vec<Hunk>>,
    refresh: Option<Task<()>>,
}

fn colour(kind: DiffKind, t: &Tokens) -> Hsla {
    match kind {
        DiffKind::Add => t.git_added,
        DiffKind::Mod => t.git_modified,
        DiffKind::Del => t.git_deleted,
    }
}

/// Reads the file as it was at its last commit, then marks what differs.
pub(super) fn read_base(bench: &mut Workbench, key: &FileKey, cx: &mut Context<Workbench>) {
    let Some(project) = lsp::project_of(bench, &key.path) else {
        return;
    };
    let Some(target) =
        (bench.editors.read(cx).open.get(key)).map(|document| document.target.clone())
    else {
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
    let (editors, key) = (bench.editors.downgrade(), key.clone());
    cx.spawn(async move |_, cx| {
        let base = git::file_at(&target, &project, "HEAD", &relative)
            .await
            .ok();
        let _ = editors.update(cx, |editors, cx| {
            if let Some(document) = editors.open.get_mut(&key) {
                document.gutter.base = base.map(SharedString::from);
                mark(document, &key, cx);
            }
        });
    })
    .detach();
}

/// Marks again what differs from the last commit, once typing pauses.
pub(super) fn changed(editors: &Entity<Editors>, key: &FileKey, cx: &mut App) {
    editors.update(cx, |editors, cx| {
        if let Some(document) = editors.open.get_mut(key) {
            mark(document, key, cx);
        }
    });
}

fn mark(document: &mut Document, key: &FileKey, cx: &mut Context<Editors>) {
    let Some(base) = document.gutter.base.clone() else {
        document.gutter.hunks = Rc::default();
        return cx.notify();
    };
    let (state, key) = (document.state.clone(), key.clone());
    document.gutter.refresh = Some(cx.spawn(async move |editors, cx| {
        cx.background_executor().timer(DEBOUNCE).await;
        let Ok(Some(task)) = editors.update(cx, |_, cx| {
            let live = state.read(cx).value();
            (live.len() <= MAX_CHARACTERS).then(|| git::hunks(base, live.to_string(), cx))
        }) else {
            return;
        };
        let hunks = task.await;
        let _ = editors.update(cx, |editors, cx| {
            if let Some(document) = editors.open.get_mut(&key) {
                document.gutter.hunks = Rc::new(hunks);
                cx.notify();
            }
        });
    }));
}

/// The marks over an editor: a strip beside each changed run of lines, and
/// the same runs along the right edge, in proportion to the file.
pub(super) fn overlay(
    this: &Entity<Workbench>,
    key: &FileKey,
    state: &Entity<EditorState>,
    gutter: &Gutter,
    lines: usize,
    t: &Tokens,
) -> impl IntoElement {
    let strip = {
        let t = t.clone();
        let (state, hunks) = (state.clone(), gutter.hunks.clone());
        canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let state = state.read(cx);
                let line_top = |line: u32| -> Option<Bounds<Pixels>> {
                    let offset = state.text().position_to_offset(&Position::new(line, 0));
                    state.range_to_bounds(&(offset..offset))
                };
                for hunk in hunks.iter() {
                    let Some(first) = line_top(hunk.lines.start) else {
                        continue;
                    };
                    let bottom = match hunk.lines.is_empty() {
                        true => first.top() + px(3.),
                        false => line_top(hunk.lines.end - 1)
                            .map_or(first.bottom(), |last| last.bottom()),
                    };
                    let width = if hunk.lines.is_empty() {
                        px(6.)
                    } else {
                        px(3.)
                    };
                    let left = first.left() - px(6.);
                    let bounds =
                        Bounds::from_corners(point(left, first.top()), point(left + width, bottom));
                    window.paint_quad(fill(bounds, colour(hunk.kind, &t)));
                }
            },
        )
        .absolute()
        .size_full()
    };
    let total = lines.max(1) as f32;
    let marks = gutter.hunks.iter().enumerate().map(|(index, hunk)| {
        let (this, at) = (
            this.clone(),
            OpenAt {
                path: key.path.clone(),
                line: hunk.lines.start,
                column: Some(0),
            },
        );
        let key = key.clone();
        let length = (hunk.lines.end - hunk.lines.start).max(1) as f32;
        div()
            .id(("change", index))
            .absolute()
            .right_0()
            .top(relative(hunk.lines.start as f32 / total))
            .h(relative((length / total).max(0.004)))
            .min_h(px(2.))
            .w(px(if hunk.kind == DiffKind::Del { 4. } else { 6. }))
            .bg(colour(hunk.kind, t))
            .cursor_pointer()
            .hover(|style| style.w(px(8.)))
            .on_click(move |_, window, cx| {
                let at = at.clone();
                this.update(cx, |bench, cx| place_cursor(bench, &key, &at, window, cx))
            })
    });
    div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .child(strip)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(8.))
                .children(marks),
        )
}
