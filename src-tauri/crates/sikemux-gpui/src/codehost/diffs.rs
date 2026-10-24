//! Changed files' diffs in cards, one list drawn only as far as it shows: a pull
//! request's files, or what a branch changes since its base.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, list, px, AnyElement, App, ClipboardItem, Context, Div, Entity, InteractiveElement,
    IntoElement, ListAlignment, ListState, MouseButton, MouseMoveEvent, ParentElement, Pixels,
    SharedString, Styled, WeakEntity, Window,
};
use sikemux_core::git::diff::{DiffRow, ROW_ADDED, ROW_DELETED, ROW_HIDDEN};
use sikemux_wire::codehost::ChangedFile;

use super::HostArea;
use crate::git::{row, Colours, Diff, RowLook};
use crate::icons::Icon;
use crate::send_to;
use crate::theme::{self, Tokens, RADIUS_2};
use crate::workbench::Workbench;

/// The pull request a list of files belongs to, which a line handed to an agent is quoted from.
#[derive(Clone)]
pub struct PullOf {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub bench: WeakEntity<Workbench>,
}

pub struct Changed {
    pub file: ChangedFile,
    /// None while it is read.
    pub diff: Option<Diff>,
    pub colours: Colours,
}

impl Changed {
    pub fn new(file: ChangedFile, diff: Option<Diff>) -> Self {
        Self {
            file,
            diff,
            colours: Colours::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Item {
    Header(usize),
    Row(usize, usize),
    End(usize),
}

pub struct Diffs {
    pub files: Vec<Changed>,
    rows: ListState,
    items: Vec<Item>,
    /// The rows a press and drag picked: the file, then where the drag began and where it is.
    picked: Rc<Cell<Option<(usize, usize, usize)>>>,
}

impl Diffs {
    pub fn new(files: Vec<Changed>) -> Self {
        let mut diffs = Self {
            files,
            rows: ListState::new(0, ListAlignment::Top, px(600.)),
            items: Vec::new(),
            picked: Rc::default(),
        };
        diffs.lay_out();
        diffs
    }

    /// Lists every file's rows again, keeping what is in view where it is.
    pub fn lay_out(&mut self) {
        let mut items = Vec::new();
        for (at, changed) in self.files.iter().enumerate() {
            items.push(Item::Header(at));
            let rows = changed.diff.as_ref().map_or(0, Diff::len);
            items.extend((0..rows).map(|index| Item::Row(at, index)));
            items.push(Item::End(at));
        }
        crate::git::relist(&self.rows, &self.items, &items);
        self.items = items;
    }

    /// Scrolls to `path`'s card.
    pub fn reveal(&self, path: &str) {
        let Some(at) = self
            .files
            .iter()
            .position(|changed| changed.file.path == path)
        else {
            return;
        };
        if let Some(item) = self.items.iter().position(|item| *item == Item::Header(at)) {
            self.rows.scroll_to_reveal_item(item);
        }
    }

    /// Colours each read diff as its file's language, once per theme.
    pub fn colour(
        &mut self,
        cx: &mut Context<HostArea>,
        find: impl Fn(&mut HostArea) -> Option<&mut Diffs> + Clone + 'static,
    ) {
        for (at, changed) in self.files.iter_mut().enumerate() {
            let Some(diff) = &changed.diff else {
                continue;
            };
            let language = crate::file_icons::language(&changed.file.path, cx)
                .filter(|language| crate::syntax::knows(language));
            let find = find.clone();
            changed
                .colours
                .ask(diff, language.as_deref(), cx, move |area: &mut HostArea| {
                    Some(&mut find(area)?.files.get_mut(at)?.colours)
                });
        }
    }

    /// The cards, opening a line in `folder`'s copy of its file.
    pub fn list(
        &self,
        view: &Entity<HostArea>,
        find: impl Fn(&HostArea) -> Option<&Diffs> + 'static,
        folder: Option<&str>,
        pull: Option<PullOf>,
        window: &mut Window,
    ) -> AnyElement {
        let advance = crate::git::advance(window);
        let (view, folder) = (view.clone(), folder.map(str::to_owned));
        list(self.rows.clone(), move |index, _, cx| {
            let cx = &*cx;
            let found =
                find(view.read(cx)).and_then(|diffs| Some((diffs, *diffs.items.get(index)?)));
            match found {
                Some((diffs, item)) => item_of(
                    &view,
                    diffs,
                    item,
                    folder.as_deref(),
                    pull.as_ref(),
                    advance,
                    cx,
                ),
                None => div().into_any_element(),
            }
        })
        .size_full()
        .font_family(theme::MONO)
        .into_any_element()
    }
}

/// How many lines a file adds and takes out: its diff's, else what the host counted.
fn stat(changed: &Changed) -> (u64, u64) {
    match &changed.diff {
        Some(diff) if !diff.is_empty() => diff.stat(),
        _ => (changed.file.additions, changed.file.deletions),
    }
}

/// One item of the list; its rows open the file in the editor while it is in `folder`.
fn item_of(
    view: &Entity<HostArea>,
    diffs: &Diffs,
    item: Item,
    folder: Option<&str>,
    pull: Option<&PullOf>,
    advance: Pixels,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let frame = |inner: Div| {
        div().px(px(12.)).child(
            inner
                .border_l_1()
                .border_r_1()
                .border_color(t.border)
                .bg(t.surface),
        )
    };
    match item {
        Item::Header(at) => {
            let changed = &diffs.files[at];
            let file = &changed.file;
            let (added, removed) = stat(changed);
            div()
                .px(px(12.))
                .pt(px(12.))
                .child(
                    div()
                        .h(px(30.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded_t(RADIUS_2)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.surface_raised)
                        .font_family(theme::UI)
                        .text_size(crate::theme::ui_text(12.))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_primary)
                                .child(file.path.clone()),
                        )
                        .children(file.previous_path.as_ref().map(|from| {
                            div()
                                .text_color(t.text_tertiary)
                                .child(format!("from {from}"))
                        }))
                        .child(div().flex_1())
                        .child(div().text_color(t.live).child(format!("+{added}")))
                        .child(div().text_color(t.danger).child(format!("−{removed}"))),
                )
                .into_any_element()
        }
        Item::Row(at, index) => {
            let changed = &diffs.files[at];
            let Some(diff) = &changed.diff else {
                return div().into_any_element();
            };
            let look = RowLook::new(&t, advance, px(0.));
            let path = folder.map(|folder| Path::new(folder).join(&changed.file.path));
            let range = match diffs.picked.get() {
                Some((file, from, to)) if file == at && from != to => {
                    Some(from.min(to)..=from.max(to))
                }
                _ => None,
            };
            let ranged = range.as_ref().is_some_and(|range| range.contains(&index));
            let picked: Vec<DiffRow> = match &range {
                Some(range) if ranged => range
                    .clone()
                    .filter_map(|at| diff.row(at))
                    .collect::<Vec<_>>(),
                _ => diff.row(index).into_iter().collect::<Vec<_>>(),
            }
            .into_iter()
            .filter(|picked| picked.0 != ROW_HIDDEN)
            .map(|picked| DiffRow(picked.0, picked.1, picked.2.clone()))
            .collect();
            let file = changed.file.path.clone();
            let (pressed, dragged) = (diffs.picked.clone(), diffs.picked.clone());
            let (pressing, dragging) = (view.clone(), view.clone());
            let drawn = frame(
                div()
                    .when(ranged, |rows| rows.bg(t.surface_chosen))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        if pressed.replace(Some((at, index, index))).is_some() {
                            pressing.update(cx, |_, cx| cx.notify());
                        }
                    })
                    .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button != Some(MouseButton::Left) {
                            return;
                        }
                        if let Some((file, from, to)) = dragged.get() {
                            if file == at && to != index {
                                dragged.set(Some((file, from, index)));
                                dragging.update(cx, |_, cx| cx.notify());
                            }
                        }
                    })
                    .text_size(crate::theme::ui_text(12.))
                    .line_height(crate::theme::ui_text(19.))
                    .when(index == 0, |rows| rows.pt(px(6.)))
                    .when(index + 1 == diff.len(), |rows| rows.pb(px(6.)))
                    .child(row(
                        diff,
                        &changed.colours,
                        index,
                        &look,
                        SharedString::from(format!("host-row-{at}-{index}")),
                        path.as_deref(),
                        |_, _| {},
                    )),
            );
            match pull.cloned().filter(|_| !picked.is_empty()) {
                Some(pull) => drawn
                    .id(SharedString::from(format!("host-row-menu-{at}-{index}")))
                    .context_menu(move |popup, window, cx| {
                        line_menu(popup, &pull, &file, &picked, window, cx)
                    })
                    .into_any_element(),
                None => drawn.into_any_element(),
            }
        }
        Item::End(at) => {
            let changed = &diffs.files[at];
            let said = match &changed.diff {
                None => Some("Reading the diff…".to_owned()),
                Some(diff) if diff.is_empty() => Some(
                    changed
                        .file
                        .note
                        .clone()
                        .unwrap_or_else(|| "The host shows no diff for this file.".into()),
                ),
                Some(_) => None,
            };
            div()
                .px(px(12.))
                .child(
                    div()
                        .map(|end| match said {
                            Some(said) => end
                                .px(px(10.))
                                .py(px(8.))
                                .font_family(theme::UI)
                                .text_size(crate::theme::ui_text(11.))
                                .text_color(t.text_tertiary)
                                .child(said),
                            None => end.h(px(1.)),
                        })
                        .border_b_1()
                        .border_l_1()
                        .border_r_1()
                        .border_color(t.border)
                        .rounded_b(RADIUS_2)
                        .bg(t.surface),
                )
                .into_any_element()
        }
    }
}

/// A pull request's picked lines' menu: copy them, or hand them to one of the project's agents.
fn line_menu(
    popup: PopupMenu,
    pull: &PullOf,
    path: &str,
    picked: &[DiffRow],
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let copied = picked
        .iter()
        .map(|row| row.2.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let one = picked.len() == 1;
    let popup = popup.item(PopupMenuItem::new("Copy Lines").icon(Icon::Copy).on_click(
        move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone())),
    ));
    send_to::submenu_with_text(
        popup,
        if one {
            "Send Line to Agent…"
        } else {
            "Send Lines to Agent…"
        },
        pull.bench.clone(),
        lines_delivery(pull, path, picked),
        window,
        cx,
    )
}

/// Lines of a pull request as an agent is told them: where they are, then the lines as a diff.
fn lines_delivery(pull: &PullOf, path: &str, picked: &[DiffRow]) -> String {
    let span = |numbers: Vec<u32>| -> Option<String> {
        let (first, last) = (*numbers.iter().min()?, *numbers.iter().max()?);
        Some(if first == last {
            format!("line {first}")
        } else {
            format!("lines {first}–{last}")
        })
    };
    let numbers = |deleted: bool| {
        picked
            .iter()
            .filter(|row| (row.0 == ROW_DELETED) == deleted)
            .map(|row| row.1)
            .collect::<Vec<_>>()
    };
    let place = [
        span(numbers(true)).map(|span| format!("old {span}")),
        span(numbers(false)).map(|span| format!("new {span}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" and ");
    let lines = picked
        .iter()
        .map(|row| {
            let mark = match row.0 {
                ROW_ADDED => "+",
                ROW_DELETED => "-",
                _ => " ",
            };
            format!("{mark}{}", row.2)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let diff = send_to::code_fence(&lines).replacen('\n', "diff\n", 1);
    format!(
        "From pull request #{} \"{}\" ({}), {path}, {place}:\n\n{diff}\n",
        pull.number, pull.title, pull.url
    )
}
