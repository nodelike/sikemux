//! Changed files reviewed in one stream: a card per file with its header, and its diff
//! under it while open, every card's rows in one virtualised list.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, list, px, AnyElement, App, Context, Div, Entity, InteractiveElement, IntoElement,
    ListAlignment, ListOffset, ListState, ParentElement, Pixels, ScrollEvent, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use serde_json::{json, Value};
use sikemux_core::git::diff::DiffRow;
use sikemux_core::git::status::GitFile;

use super::pane::{has_unstaged, is_staged, status_mark, GitPane, Side};
use super::{row, Colours, Compared, Diff, Notes, RowLook};
use crate::core::Target;
use crate::editor::OpenAt;
use crate::theme::{
    Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_3XS, TEXT_XS,
};
use crate::ui;

const TITLE_HEIGHT: Pixels = px(22.);
const CARD_GAP: Pixels = px(8.);
/// Cards past the first one showing whose diffs are read ahead of being scrolled to.
const READ_AHEAD: usize = 6;

/// One diff of a card: the whole change, or its staged or unstaged half.
struct Section {
    side: Option<Side>,
    compared: Compared,
    whole: bool,
    diff: Option<Result<Diff, String>>,
    asked: bool,
    colours: Colours,
    /// Each row's note, such as the agent that wrote it.
    notes: Vec<Option<SharedString>>,
    scrolled: Pixels,
}

struct Card {
    path: String,
    /// How git sees the file, for a working change; none for a commit's file.
    file: Option<GitFile>,
    sections: Vec<Section>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Header(usize),
    Title(usize, usize),
    Row(usize, usize, usize),
    Note(usize, usize),
    End(usize),
}

impl Item {
    fn card(self) -> usize {
        match self {
            Item::Header(card)
            | Item::Title(card, _)
            | Item::Row(card, _, _)
            | Item::Note(card, _)
            | Item::End(card) => card,
        }
    }
}

/// What a card compares: its file, how git sees it, and each of its diffs.
struct Wanted {
    path: String,
    file: Option<GitFile>,
    sections: Vec<(Option<Side>, Compared)>,
}

#[derive(Default)]
pub struct Review {
    repo: String,
    /// The commit whose files are shown, else the working changes.
    commit: Option<String>,
    cards: Vec<Card>,
    folded: HashSet<String>,
    items: Vec<Item>,
    list: Option<ListState>,
    focus: Option<String>,
}

impl Review {
    fn list(&mut self) -> ListState {
        self.list
            .get_or_insert_with(|| ListState::new(0, ListAlignment::Top, px(800.)))
            .clone()
    }

    /// Lists the working changes in `files`, keeping the diffs already read of files
    /// whose state is unchanged.
    pub fn show_changes(&mut self, repo: &str, files: &[GitFile]) {
        let compared = |path: &str, base: &str, head: Option<&str>| Compared {
            repo: repo.to_owned(),
            path: path.to_owned(),
            base: base.to_owned(),
            head: head.map(str::to_owned),
        };
        let wanted = files
            .iter()
            .map(|file| {
                let path = file.path.as_str();
                let sections = match (is_staged(file), has_unstaged(file)) {
                    (true, true) => vec![
                        (Some(Side::Staged), compared(path, "HEAD", Some(":index"))),
                        (Some(Side::Unstaged), compared(path, ":index", None)),
                    ],
                    (true, false) => vec![(None, compared(path, "HEAD", Some(":index")))],
                    _ => vec![(None, compared(path, "HEAD", None))],
                };
                Wanted {
                    path: file.path.clone(),
                    file: Some(file.clone()),
                    sections,
                }
            })
            .collect();
        self.show(repo, None, wanted);
    }

    /// Lists what commit `rev` changed in `paths`, every file folded at first.
    pub fn show_commit(&mut self, repo: &str, rev: &str, paths: &[String]) {
        let fresh = self.commit.as_deref() != Some(rev);
        let wanted = paths
            .iter()
            .map(|path| Wanted {
                path: path.clone(),
                file: None,
                sections: vec![(
                    None,
                    Compared {
                        repo: repo.to_owned(),
                        path: path.clone(),
                        base: format!("{rev}~1"),
                        head: Some(rev.to_owned()),
                    },
                )],
            })
            .collect();
        self.show(repo, Some(rev.to_owned()), wanted);
        if fresh {
            self.fold_all(true);
        }
    }

    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    fn show(&mut self, repo: &str, commit: Option<String>, wanted: Vec<Wanted>) {
        if self.repo != repo || self.commit != commit {
            *self = Review {
                repo: repo.to_owned(),
                commit,
                list: self.list.take(),
                ..Review::default()
            };
        }
        let mut old: HashMap<String, Card> = std::mem::take(&mut self.cards)
            .into_iter()
            .map(|card| (card.path.clone(), card))
            .collect();
        self.cards = wanted
            .into_iter()
            .map(|wanted| {
                let kept = old.remove(&wanted.path).filter(|card| {
                    card.file == wanted.file
                        && card
                            .sections
                            .iter()
                            .map(|section| (section.side, &section.compared))
                            .eq(wanted
                                .sections
                                .iter()
                                .map(|(side, compared)| (*side, compared)))
                });
                kept.unwrap_or_else(|| Card {
                    path: wanted.path,
                    file: wanted.file,
                    sections: wanted
                        .sections
                        .into_iter()
                        .map(|(side, compared)| Section {
                            side,
                            compared,
                            whole: false,
                            diff: None,
                            asked: false,
                            colours: Colours::default(),
                            notes: Vec::new(),
                            scrolled: px(0.),
                        })
                        .collect(),
                })
            })
            .collect();
        let listed: HashSet<&str> = self.cards.iter().map(|card| card.path.as_str()).collect();
        self.folded.retain(|path| listed.contains(path.as_str()));
        self.lay_out();
    }

    /// Reads every card's diffs again, as after the repository's head or index moved.
    pub fn reread(&mut self) {
        for section in self.cards.iter_mut().flat_map(|card| &mut card.sections) {
            section.asked = false;
        }
    }

    /// Reads again the diffs of the cards whose files are among `moved`, by full path.
    pub fn reread_files(&mut self, moved: &[PathBuf]) {
        let repo = Path::new(&self.repo);
        for card in &mut self.cards {
            if moved.contains(&repo.join(&card.path)) {
                card.sections
                    .iter_mut()
                    .for_each(|section| section.asked = false);
            }
        }
    }

    /// Opens the card of `path` and scrolls it to the top, when it is a new focus.
    pub fn focus(&mut self, path: Option<&str>) {
        if self.focus.as_deref() == path {
            return;
        }
        self.focus = path.map(str::to_owned);
        let Some(path) = path else {
            return;
        };
        if self.folded.remove(path) {
            self.lay_out();
        }
        if let Some(at) = self.items.iter().position(|item| match item {
            Item::Header(card) => self.cards[*card].path == path,
            _ => false,
        }) {
            self.list().scroll_to(ListOffset {
                item_ix: at,
                offset_in_item: px(0.),
            });
        }
    }

    pub fn fold_all(&mut self, folded: bool) {
        self.folded = if folded {
            self.cards.iter().map(|card| card.path.clone()).collect()
        } else {
            HashSet::new()
        };
        self.lay_out();
    }

    fn toggle(&mut self, card: usize) {
        let path = &self.cards[card].path;
        if !self.folded.remove(path) {
            self.folded.insert(path.clone());
        }
        self.lay_out();
    }

    /// Works out the list's items again and tells the list only the run that changed,
    /// so the reader stays where they were.
    fn lay_out(&mut self) {
        let mut items = Vec::new();
        for (index, card) in self.cards.iter().enumerate() {
            items.push(Item::Header(index));
            if self.folded.contains(&card.path) {
                continue;
            }
            for (at, section) in card.sections.iter().enumerate() {
                if section.side.is_some() {
                    items.push(Item::Title(index, at));
                }
                match &section.diff {
                    Some(Ok(diff)) if !diff.is_empty() => {
                        items.extend((0..diff.len()).map(|row| Item::Row(index, at, row)))
                    }
                    _ => items.push(Item::Note(index, at)),
                }
            }
            items.push(Item::End(index));
        }
        super::relist(&self.list(), &self.items, &items);
        self.items = items;
    }

    /// Reads the diffs of the open cards from the one at the top of the list on, so a
    /// card has its diff by the time it is scrolled to.
    fn read(&mut self, core: &Target, cx: &mut Context<GitPane>) {
        let top = self.list().logical_scroll_top().item_ix;
        let first = self.items.get(top).map_or(0, |item| item.card());
        for index in first..(first + READ_AHEAD).min(self.cards.len()) {
            if self.folded.contains(&self.cards[index].path) {
                continue;
            }
            for at in 0..self.cards[index].sections.len() {
                let section = &mut self.cards[index].sections[at];
                if section.asked {
                    continue;
                }
                section.asked = true;
                let Compared {
                    repo,
                    path,
                    base,
                    head,
                } = &section.compared;
                let params = json!({ "repo": repo, "path": path, "baseRev": base, "headRev": head, "full": section.whole });
                let answer = core.request("git.fileDiff", params);
                let (path, side) = (path.clone(), section.side);
                cx.spawn(async move |pane, cx| {
                    let diff = answer.await.and_then(|rows| {
                        serde_json::from_value::<Vec<DiffRow>>(rows)
                            .map_err(|error| error.to_string())
                    });
                    pane.update(cx, |pane, cx| {
                        let notes = Notes::of(pane.owners_of(side), &path);
                        pane.review
                            .read_as(&path, side, diff.map(Diff::new), &notes);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn read_as(
        &mut self,
        path: &str,
        side: Option<Side>,
        diff: Result<Diff, String>,
        notes: &Notes,
    ) {
        let Some(section) = self
            .cards
            .iter_mut()
            .find(|card| card.path == path)
            .and_then(|card| {
                card.sections
                    .iter_mut()
                    .find(|section| section.side == side)
            })
        else {
            return;
        };
        section.notes = diff
            .as_ref()
            .map_or_else(|_| Vec::new(), |diff| diff.notes(notes));
        section.diff = Some(diff);
        section.colours = Colours::default();
        self.lay_out();
    }

    /// Notes beside each diff's lines who wrote them, as `owners` says, or `staged` for
    /// the staged half of a file with unstaged changes too.
    pub fn annotate(&mut self, owners: &Value, staged: &Value) {
        for card in &mut self.cards {
            for section in &mut card.sections {
                let owners = if section.side == Some(Side::Staged) {
                    staged
                } else {
                    owners
                };
                if let Some(Ok(diff)) = &section.diff {
                    section.notes = diff.notes(&Notes::of(owners, &card.path));
                }
            }
        }
    }

    fn expand(&mut self, card: usize, at: usize) {
        let section = &mut self.cards[card].sections[at];
        if !section.whole {
            section.whole = true;
            section.asked = false;
        }
    }

    fn section_mut(&mut self, card: usize, at: usize) -> Option<&mut Section> {
        self.cards.get_mut(card)?.sections.get_mut(at)
    }
}

/// The review column: every changed file's card, in the order the files are listed.
pub fn review(pane: &mut GitPane, window: &mut Window, cx: &mut Context<GitPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    if pane.review.cards.is_empty() {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_secondary)
            .child("Nothing to review.")
            .into_any_element();
    }
    let core = pane.core.clone();
    pane.review.read(&core, cx);
    let language = |path: &str, cx: &App| {
        crate::file_icons::language(path, cx).filter(|language| crate::syntax::knows(language))
    };
    for card in 0..pane.review.cards.len() {
        let read = |section: &Section| matches!(section.diff, Some(Ok(_)));
        if !pane.review.cards[card].sections.iter().any(read) {
            continue;
        }
        let language = language(&pane.review.cards[card].path, cx);
        for at in 0..pane.review.cards[card].sections.len() {
            let section = &mut pane.review.cards[card].sections[at];
            if let Some(Ok(diff)) = &section.diff {
                section
                    .colours
                    .ask(diff, language.as_deref(), cx, move |pane: &mut GitPane| {
                        pane.review
                            .section_mut(card, at)
                            .map(|section| &mut section.colours)
                    });
            }
        }
    }
    let advance = super::advance(window);
    let view = cx.entity();
    let rows = list(pane.review.list(), move |index, _, cx| {
        let cx = &*cx;
        let pane = view.read(cx);
        match pane.review.items.get(index).copied() {
            Some(item) => draw(&view, pane, item, advance, cx),
            None => div().into_any_element(),
        }
    })
    .size_full();
    let pinned = pinned_header(pane, &cx.entity(), cx);
    div()
        .size_full()
        .relative()
        .font_family(crate::theme::MONO)
        .child(rows)
        .children(pinned)
        .into_any_element()
}

/// The header of the card whose top has scrolled out of sight, held at the top.
fn pinned_header(pane: &GitPane, view: &Entity<GitPane>, cx: &App) -> Option<AnyElement> {
    let review = &pane.review;
    let top = review.list.as_ref()?.logical_scroll_top();
    let item = *review.items.get(top.item_ix)?;
    if matches!(item, Item::Header(_)) && top.offset_in_item == px(0.) {
        return None;
    }
    if matches!(item, Item::End(_)) {
        return None;
    }
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .child(header(view, pane, item.card(), cx))
            .into_any_element(),
    )
}

fn draw(
    view: &Entity<GitPane>,
    pane: &GitPane,
    item: Item,
    advance: Pixels,
    cx: &App,
) -> AnyElement {
    let review = &pane.review;
    let t = Tokens::get(cx).clone();
    let body = |inner: Div| {
        div().child(
            inner
                .border_l_1()
                .border_r_1()
                .border_color(t.border)
                .bg(t.surface),
        )
    };
    match item {
        Item::Header(card) => header(view, pane, card, cx),
        Item::End(_) => div()
            .pb(CARD_GAP)
            .child(
                div()
                    .h(px(1.))
                    .border_b_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(t.border)
                    .rounded_b(RADIUS_2)
                    .bg(t.surface),
            )
            .into_any_element(),
        Item::Title(card, at) => {
            let section = &review.cards[card].sections[at];
            let file = review.cards[card].file.as_ref();
            let (code, word) = match section.side {
                Some(Side::Staged) => (file.map_or("", |file| file.index.as_str()), "STAGED"),
                _ => (file.map_or("", |file| file.worktree.as_str()), "UNSTAGED"),
            };
            body(
                div()
                    .h(TITLE_HEIGHT)
                    .px(SPACE_2H)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .when(at > 0, |title| title.border_t_1())
                    .bg(t.surface.opacity(0.55))
                    .font_family(crate::theme::UI)
                    .text_size(crate::theme::ui_text(TEXT_3XS))
                    .text_color(t.text_tertiary)
                    .children(status_mark(code, cx))
                    .child(div().child(word)),
            )
            .into_any_element()
        }
        Item::Note(card, at) => {
            let section = &review.cards[card].sections[at];
            let (text, colour) = match &section.diff {
                None => ("loading diff...".to_owned(), t.text_tertiary),
                Some(Err(error)) => (format!("x {error}"), t.danger),
                Some(Ok(_)) => ("No changes.".to_owned(), t.text_tertiary),
            };
            body(
                div()
                    .p(SPACE_3)
                    .font_family(crate::theme::UI)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(colour)
                    .child(text),
            )
            .into_any_element()
        }
        Item::Row(card, at, index) => {
            let section = &review.cards[card].sections[at];
            let Some(Ok(diff)) = &section.diff else {
                return div().into_any_element();
            };
            let look = RowLook {
                note: section.notes.get(index).cloned().flatten(),
                ..RowLook::new(&t, advance, section.scrolled)
            };
            let file = Path::new(&review.repo).join(&review.cards[card].path);
            let width = diff.width(advance);
            let (expanding, wheeling) = (view.clone(), view.clone());
            body(
                div()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .line_height(crate::theme::ui_text(19.))
                    .when(index == 0, |rows| rows.pt(SPACE_2))
                    .when(index + 1 == diff.len(), |rows| rows.pb(SPACE_2))
                    .on_scroll(move |event: &ScrollEvent, window, cx| {
                        let delta = event.delta.pixel_delta(window.line_height());
                        if delta.x.abs() <= delta.y.abs() {
                            return;
                        }
                        cx.stop_propagation();
                        wheeling.update(cx, |pane, cx| {
                            if let Some(section) = pane.review.section_mut(card, at) {
                                section.scrolled = (section.scrolled - delta.x)
                                    .clamp(px(0.), (width - px(200.)).max(px(0.)));
                                cx.notify();
                            }
                        });
                    })
                    .child(row(
                        diff,
                        &section.colours,
                        index,
                        &look,
                        SharedString::from(format!("review-{card}-{at}-{index}")),
                        Some(&file),
                        move |_, cx| {
                            expanding.update(cx, |pane, cx| {
                                pane.review.expand(card, at);
                                cx.notify();
                            })
                        },
                    )),
            )
            .into_any_element()
        }
    }
}

/// Takes the pane rather than reading `view`, since the pinned header draws inside the
/// pane's own render, where reading it panics.
fn header(view: &Entity<GitPane>, pane: &GitPane, card: usize, cx: &App) -> AnyElement {
    let review = &pane.review;
    let path = &review.cards[card].path;
    let file = review.cards[card].file.as_ref();
    let open = !review.folded.contains(path);
    let target = OpenAt {
        path: Path::new(&review.repo).join(path),
        line: 0,
        column: None,
    };
    let toggling = view.clone();
    let key = SharedString::from(format!("card:{path}"));
    let opens: crate::ui::Opens = Box::new(move |window, cx| {
        window.dispatch_action(gpui_kit::Action::boxed_clone(&target), cx)
    });
    div()
        .when(!open, |card| card.pb(CARD_GAP))
        .child(
            ui::file_head(format!("card-{path}"), path, open, Some(opens), cx)
                .on_hover(super::pane::hover(view, key.clone()))
                .on_click(move |_, _, cx| {
                    toggling.update(cx, |pane, cx| {
                        pane.review.toggle(card);
                        cx.notify();
                    })
                })
                .when(pane.hovered.as_ref() == Some(&key), |head| {
                    head.child(
                        div().flex().gap(SPACE_0).pr(SPACE_1).children(
                            file.map(|file| super::pane::file_actions(view, file, None, cx))
                                .into_iter()
                                .flatten(),
                        ),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .pr(px(1.))
                        .children(file.map(|file| badges(file, cx)).into_iter().flatten()),
                ),
        )
        .into_any_element()
}

/// A file's status letters: one for each side that changed, or one for an untracked or
/// conflicted file, whose sides say the same.
fn badges(file: &GitFile, cx: &App) -> Vec<Div> {
    if file.index == "U" || file.worktree == "U" {
        return status_mark("U", cx).into_iter().collect();
    }
    if file.index == "?" && file.worktree == "?" {
        return status_mark("?", cx).into_iter().collect();
    }
    [
        status_mark(&file.index, cx),
        status_mark(&file.worktree, cx),
    ]
    .into_iter()
    .flatten()
    .collect()
}
