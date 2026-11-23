//! A project's Pages in a tab of its own: every page its agents showed, in sections by how long
//! Sikemux keeps them, beside the one view a picked page is read in.

pub(crate) mod reader;

pub(crate) use reader::Reader;

use std::collections::HashMap;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
};
use sikemux_calls::call;
use sikemux_calls::harness::{KeepPageParams, ListedPage, PageLife, PagesParams};
use sikemux_view::layout::{At, OpenTab, PaneKind};

use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_2, SPACE_3};
use crate::ui;
use crate::workbench::Workbench;

/// The pages of each project held, by folder, and its tab while one is open.
#[derive(Default)]
pub(crate) struct Pages {
    listed: HashMap<String, Option<Vec<ListedPage>>>,
    tabs: HashMap<String, Library>,
}

struct Library {
    filter: Entity<InputState>,
    reader: Entity<Reader>,
    _filtering: Subscription,
}

impl Pages {
    /// How long Sikemux keeps page `id`, as its project's list last said.
    pub(crate) fn life_of(&self, id: &str) -> Option<PageLife> {
        (self.listed.values().flatten().flatten())
            .find(|page| page.id == id)
            .map(|page| page.life)
    }
}

impl Workbench {
    /// Opens the Pages of the project in front.
    pub(crate) fn open_pages(&self) {
        let Some(project) = self.project() else {
            return;
        };
        self.open_in_front(OpenTab::new(
            At::project(&project.id),
            PaneKind::Pages,
            project.path.to_string_lossy(),
        ));
    }

    /// Reads every project's pages held again, as after the core says one moved.
    pub(crate) fn pages_moved(&self, cx: &mut Context<Self>) {
        for folder in self.pages.listed.keys() {
            self.reload_pages(folder, cx);
        }
    }

    /// Reads the pages of the project in `folder` once, to keep them from then on.
    pub(crate) fn want_pages(&mut self, folder: &str, cx: &mut Context<Self>) {
        if self.pages.listed.contains_key(folder) {
            return;
        }
        self.pages.listed.insert(folder.to_owned(), None);
        self.reload_pages(folder, cx);
    }

    fn reload_pages(&self, folder: &str, cx: &mut Context<Self>) {
        let asked = self.target().request_typed::<call::ListPages>(PagesParams {
            project: folder.to_owned(),
            words: String::new(),
        });
        let folder = folder.to_owned();
        cx.spawn(async move |bench, cx| {
            let Ok(pages) = asked.await else {
                return;
            };
            let _ = bench.update(cx, |bench, cx| {
                bench.pages.listed.insert(folder, Some(pages));
                bench.parts.redraw_stage(cx);
            });
        })
        .detach();
    }

    /// Keeps page `id` until the person lets it go, or lets it go again.
    pub(crate) fn keep_page(&self, id: &str, keep: bool, cx: &mut Context<Self>) {
        let keeping = self
            .target()
            .request_typed::<call::KeepPage>(KeepPageParams {
                id: id.to_owned(),
                keep,
            });
        let what = match keep {
            true => t!("The page was not kept"),
            false => t!("The page was not let go"),
        };
        self.report(keeping, what, |_, _, _| {}, cx);
    }

    fn read_in_library(&mut self, folder: &str, page: &ListedPage, cx: &mut Context<Self>) {
        let Some(library) = self.pages.tabs.get(folder) else {
            return;
        };
        let (target, files) = (self.target(), self.desks.read(cx).page_files.clone());
        let (id, title) = (page.id.clone(), page.title.clone());
        library.reader.update(cx, |reader, cx| {
            reader.open(id, title, target, files, None, cx)
        });
        self.saw_page(&page.id, cx);
    }
}

fn make(bench: &mut Workbench, folder: String, window: &mut Window, cx: &mut Context<Workbench>) {
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Find a page")));
    let filtering = cx.subscribe(&filter, |bench, _, event: &InputEvent, cx| {
        if let InputEvent::Change = event {
            bench.parts.redraw_stage(cx);
        }
    });
    let reader = Reader::new(cx.entity().downgrade(), cx);
    bench.pages.tabs.insert(
        folder.clone(),
        Library {
            filter,
            reader,
            _filtering: filtering,
        },
    );
    bench.want_pages(&folder, cx);
    bench.parts.redraw_stage(cx);
}

/// The Pages tab of the project in `folder`.
pub(crate) fn pane(
    bench: &Workbench,
    folder: &str,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let Some(library) = bench.pages.tabs.get(folder) else {
        let folder = folder.to_owned();
        cx.defer_in(window, move |bench, window, cx| {
            make(bench, folder, window, cx)
        });
        return ui::pane(cx).into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let listed = bench.pages.listed.get(folder).cloned().flatten();
    let words = library.filter.read(cx).value().to_lowercase();
    let found: Vec<&ListedPage> = (listed.iter().flatten())
        .filter(|page| matches(page, &words))
        .collect();
    let reading = library.reader.read(cx).shows().map(str::to_owned);
    if let (None, Some(first)) = (&reading, found.first()) {
        let (folder, first) = (folder.to_owned(), (*first).clone());
        cx.defer_in(window, move |bench, _, cx| {
            bench.read_in_library(&folder, &first, cx)
        });
    }
    let this = cx.entity();
    let row = |page: &&ListedPage| {
        let (this, folder, picked) = (this.clone(), folder.to_owned(), (*page).clone());
        let chosen = reading.as_deref() == Some(page.id.as_str());
        let by = match page.versions {
            0 | 1 => t!("@{agent}", agent = page.agent),
            versions => t!(
                "@{agent} · {count} versions",
                agent = page.agent,
                count = versions
            ),
        };
        ui::Row::new(
            SharedString::from(format!("page-{}", page.id)),
            page.title.clone(),
        )
        .mark(22., ui::icon(Icon::Tabs, 13.).text_color(t.text_tertiary))
        .subtitle(ui::meta(by, cx))
        .detail(life_words(page.life, page.shown_at))
        .chosen(chosen)
        .on_click(move |_, _, cx| {
            this.update(cx, |bench, cx| {
                bench.read_in_library(&folder, &picked, cx);
                bench.parts.redraw_stage(cx);
            })
        })
        .into_any_element()
    };
    let (kept, going): (Vec<&ListedPage>, Vec<&ListedPage>) =
        (found.iter()).partition(|page| page.life == PageLife::Kept);
    let mut sections: Vec<AnyElement> = Vec::new();
    if !going.is_empty() {
        let group = ui::Group::new(t!("Expiring"), "").first();
        sections.push(group.children(going.iter().map(row)).into_any_element());
    }
    if !kept.is_empty() {
        let group = ui::Group::new(t!("Kept"), "").when(going.is_empty(), ui::Group::first);
        sections.push(group.children(kept.iter().map(row)).into_any_element());
    }
    let none = match (&listed, words.trim().is_empty()) {
        (None, _) => None,
        (Some(_), _) if !found.is_empty() => None,
        (Some(_), true) => Some(t!("No pages yet")),
        (Some(_), false) => Some(t!("No page matches")),
    };
    let side = div()
        .w(px(300.))
        .flex_none()
        .flex()
        .flex_col()
        .border_r(crate::theme::border())
        .border_color(t.border)
        .child(
            div()
                .flex_none()
                .p(SPACE_3)
                .child(ui::Field::new(&library.filter)),
        )
        .child(
            div()
                .id(SharedString::from(format!("pages-{folder}")))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(SPACE_2)
                .pb(SPACE_3)
                .children(sections)
                .children(none.map(|none| div().p(SPACE_3).child(ui::meta(none, cx)))),
        );
    ui::pane(cx)
        .flex()
        .child(side)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .bg(rows::card_fill(cx))
                .child(library.reader.clone()),
        )
        .into_any_element()
}

/// Whether `page` holds every word of `words` in its title or its agent's name.
fn matches(page: &ListedPage, words: &str) -> bool {
    let named = format!("{} {}", page.title, page.agent).to_lowercase();
    (words.split_whitespace()).all(|word| named.contains(word.trim_start_matches('@')))
}

/// How long Sikemux keeps a page, in a list's tight column: the time left, or when it was shown.
pub(crate) fn life_words(life: PageLife, shown_at: u64) -> String {
    match life {
        PageLife::Kept => ui::time::age(shown_at),
        PageLife::WithCall => t!("with its call").into(),
        PageLife::Goes { at } => t!("{left} left", left = left(at)),
    }
}

/// The time until `at`, in Unix milliseconds: `40m`, `5h`, `6d`.
fn left(at: u64) -> String {
    let seconds = at.saturating_sub(ui::time::now_ms()) / 1000;
    match seconds {
        0..3600 => t!("{count}m", count = (seconds / 60).max(1)),
        3600..86_400 => t!("{count}h", count = seconds / 3600),
        _ => t!("{count}d", count = seconds / 86_400),
    }
}
