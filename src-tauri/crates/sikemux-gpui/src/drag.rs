//! What can be dragged around the window, and what each place does with it
//! when it lands: tabs reorder in their strip, projects in the rail, and files
//! move between folders, or go to a chat or a terminal.

use std::path::{Path, PathBuf};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnimationExt, AnyElement, App, AppContext, Context, Entity, ExternalPaths,
    InteractiveElement, IntoElement, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use serde_json::json;

use crate::core::Ask;
use crate::theme::Tokens;
use crate::workbench::{Strip, Workbench};

/// A tab being dragged along its strip.
#[derive(Clone)]
pub struct DraggedTab {
    pub strip: Strip,
    pub tab: String,
    pub label: SharedString,
}

/// A tab on its way along its strip: where it came from, the place it would drop into,
/// and how wide it is, which is how far the tabs it passes slide aside.
#[derive(Clone, PartialEq)]
pub struct Reorder {
    pub strip: Strip,
    pub tab: String,
    pub from: usize,
    pub to: usize,
    pub width: Pixels,
}

impl Reorder {
    /// How far the tab at `index` slides to open the gap, while the drag goes on.
    pub fn shift(&self, index: usize) -> f32 {
        let width = f32::from(self.width);
        match () {
            _ if self.from < index && index <= self.to => -width,
            _ if self.to <= index && index < self.from => width,
            _ => 0.,
        }
    }

    /// How far the tab at `index` would have slid had the gap opened past it, which is where
    /// it slides back from when the gap moves away.
    pub fn slid(&self, index: usize) -> f32 {
        let width = f32::from(self.width);
        match index.cmp(&self.from) {
            std::cmp::Ordering::Greater => -width,
            std::cmp::Ordering::Less => width,
            std::cmp::Ordering::Equal => 0.,
        }
    }
}

/// How long the tabs take to slide aside.
const SLIDE: std::time::Duration = std::time::Duration::from_millis(160);

/// A project being dragged up or down the rail.
#[derive(Clone)]
pub struct DraggedProject {
    pub id: String,
    pub label: SharedString,
}

/// A card dragged to another column of its board.
#[derive(Clone)]
pub struct DraggedCard {
    pub id: u64,
    pub label: SharedString,
}

/// A file or folder dragged out of the tree.
#[derive(Clone)]
pub struct DraggedPath {
    pub path: PathBuf,
}

/// What follows the pointer while something is dragged: its name.
pub struct Ghost {
    label: SharedString,
}

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx);
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(crate::theme::RADIUS_2)
            .border_1()
            .border_color(t.border)
            .bg(t.surface_overlay)
            .text_size(crate::theme::ui_text(12.))
            .text_color(t.text_primary)
            .child(self.label.clone())
    }
}

pub(crate) fn ghost(label: SharedString, cx: &mut App) -> Entity<Ghost> {
    cx.new(|_| Ghost { label })
}

fn name_of(path: &Path) -> SharedString {
    path.file_name()
        .map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
        .into()
}

/// A strip's tab that can be dragged to another place in its strip. While a tab of
/// its strip passes over, the tabs between slide aside to open the gap it drops into.
pub fn tab(
    tab: AnyElement,
    dragged: DraggedTab,
    index: usize,
    reorder: Option<&Reorder>,
    this: &Entity<Workbench>,
) -> impl IntoElement {
    let (strip, this) = (dragged.strip.clone(), this.clone());
    let lifted = reorder.is_some_and(|reorder| reorder.tab == dragged.tab);
    let shift = reorder.map_or(0., |reorder| reorder.shift(index));
    let back = reorder.map_or(0., |reorder| reorder.slid(index));
    let own = dragged.tab.clone();
    let slid = div()
        .flex()
        .min_w_0()
        .when(lifted, |tab| tab.opacity(0.))
        .child(tab)
        .with_animation(
            SharedString::from(format!("tab-slide-{own}-{}", shift != 0.)),
            gpui_kit::Animation::new(SLIDE).with_easing(gpui_kit::ease_out_quint()),
            move |tab, delta| {
                let from = if shift != 0. { 0. } else { back };
                tab.relative().left(px(from + (shift - from) * delta))
            },
        );
    div()
        .id(SharedString::from(format!("drag-tab-{}", dragged.tab)))
        .on_drag(dragged, |dragged: &DraggedTab, _: Point<Pixels>, _, cx| {
            ghost(dragged.label.clone(), cx)
        })
        .can_drop({
            let strip = strip.clone();
            move |value, _, _| {
                value
                    .downcast_ref::<DraggedTab>()
                    .is_some_and(|dragged| dragged.strip == strip)
            }
        })
        .on_drag_move::<DraggedTab>({
            let this = this.clone();
            move |event, _, cx| {
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                let dragged = event.drag(cx).clone();
                let width = event.bounds.size.width;
                this.update(cx, |bench, cx| {
                    over(bench, &dragged, index, own == dragged.tab, width, cx)
                });
            }
        })
        .on_drop(move |dropped: &DraggedTab, _, cx| {
            let dropped = dropped.clone();
            this.update(cx, |bench, cx| {
                bench.reorder = None;
                bench.move_tab(&dropped.strip, &dropped.tab, index);
                cx.notify();
            })
        })
        .child(slid)
}

/// Notes the place in its strip a dragged tab is over, which the tabs between slide aside for.
fn over(
    bench: &mut Workbench,
    dragged: &DraggedTab,
    index: usize,
    itself: bool,
    width: Pixels,
    cx: &mut Context<Workbench>,
) {
    let Some(from) = bench
        .tabs(&dragged.strip)
        .iter()
        .position(|tab| tab.id == dragged.tab)
    else {
        return;
    };
    let width = match (&bench.reorder, itself) {
        (_, true) => width,
        (Some(reorder), false) if reorder.tab == dragged.tab => reorder.width,
        _ => width,
    };
    let next = Reorder {
        strip: dragged.strip.clone(),
        tab: dragged.tab.clone(),
        from,
        to: index,
        width,
    };
    if bench.reorder.as_ref() != Some(&next) {
        bench.reorder = Some(next);
        cx.notify();
    }
}

/// A project's row in the rail that can be dragged to another place in the
/// list, and takes a project dropped on it, which goes where it is.
pub fn project(
    row: AnyElement,
    dragged: DraggedProject,
    this: &Entity<Workbench>,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx);
    let (target, this) = (dragged.id.clone(), this.clone());
    let hover = t.surface_hover;
    div()
        .id(SharedString::from(format!("drag-project-{}", dragged.id)))
        .on_drag(
            dragged,
            |dragged: &DraggedProject, _: Point<Pixels>, _, cx| ghost(dragged.label.clone(), cx),
        )
        .drag_over::<DraggedProject>(move |style, _, _, _| {
            style.bg(hover).rounded(crate::theme::RADIUS_2)
        })
        .on_drop(move |dropped: &DraggedProject, _, cx| {
            let (id, target) = (dropped.id.clone(), target.clone());
            this.update(cx, |bench, _| {
                let Some(index) = bench
                    .projects()
                    .iter()
                    .position(|project| project.id == target)
                else {
                    return;
                };
                if id != target {
                    let _ = bench.asks.send(Ask::Call {
                        method: "moveProject",
                        params: json!({ "id": id, "index": index }),
                    });
                }
            })
        })
        .child(row)
}

/// A row of the file tree that can be dragged onto a folder, a chat or a
/// terminal. A folder's row takes a row dropped on it, which moves into it,
/// and files dropped from outside, which are copied into it.
pub fn tree_row(
    row: AnyElement,
    path: &Path,
    folder: bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let hover = t.surface_hover;
    let dragged = DraggedPath {
        path: path.to_owned(),
    };
    let draggable = div()
        .id(SharedString::from(format!("drag-file-{}", path.display())))
        .on_drag(dragged, |dragged: &DraggedPath, _: Point<Pixels>, _, cx| {
            ghost(name_of(&dragged.path), cx)
        })
        .child(row);
    if !folder {
        return draggable.into_any_element();
    }
    let (into, into_outside, this, outside) =
        (path.to_owned(), path.to_owned(), this.clone(), this.clone());
    draggable
        .can_drop({
            let folder = path.to_owned();
            move |value, _, _| match value.downcast_ref::<DraggedPath>() {
                Some(dragged) => {
                    !folder.starts_with(&dragged.path)
                        && dragged.path.parent() != Some(folder.as_path())
                }
                None => value.downcast_ref::<ExternalPaths>().is_some(),
            }
        })
        .drag_over::<DraggedPath>(move |style, _, _, _| style.bg(hover))
        .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(hover))
        .on_drop(move |dropped: &DraggedPath, _, cx| {
            let Some(name) = dropped.path.file_name() else {
                return;
            };
            let params = json!({
                "from": dropped.path.to_string_lossy(),
                "to": into.join(name).to_string_lossy(),
            });
            this.update(cx, |bench, _| {
                let _ = bench.asks.send(Ask::Call {
                    method: "files.rename",
                    params,
                });
            })
        })
        .on_drop(move |dropped: &ExternalPaths, _, cx| {
            let dir = into_outside.to_string_lossy().into_owned();
            outside.update(cx, |bench, _| {
                for from in dropped.paths() {
                    let _ = bench.asks.send(Ask::Call {
                        method: "files.copyInto",
                        params: json!({ "from": from.to_string_lossy(), "dir": dir }),
                    });
                }
            })
        })
        .into_any_element()
}
