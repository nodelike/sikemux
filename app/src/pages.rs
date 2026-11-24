//! A project's Pages: every page its agents showed, listed on the tab rail in sections by how
//! long Sikemux keeps them, and the one being read in a tab of its own.

pub(crate) mod list;
pub(crate) mod reader;

pub(crate) use reader::Reader;

use std::collections::HashMap;
use std::path::Path;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    div, AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, Styled, Subscription,
    Window,
};
use sikemux_calls::call;
use sikemux_calls::harness::{KeepPageParams, ListedPage, PageLife, PageShare, PagesParams};
use sikemux_view::layout::{At, OpenTab, PaneKind};
use sikemux_view::prefs::TabRailPage;
use sikemux_view::workspace::ProjectInfo;

use crate::core::Target;
use crate::hosts::{HostChoice, Place};
use crate::rail::Rail;
use crate::settings::rows;
use crate::ui;
use crate::workbench::{Strip, Workbench};

/// The pages of each project held, and the view each project's are read in, by folder.
pub(crate) struct Pages {
    listed: HashMap<String, Option<Vec<ListedPage>>>,
    readers: HashMap<String, Entity<Reader>>,
    filter: Entity<InputState>,
    _filtering: Subscription,
}

impl Pages {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Find a page")));
        let filtering = cx.subscribe(&filter, |bench, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                bench.redraw_rail(Rail::Agents, cx);
            }
        });
        Self {
            listed: HashMap::new(),
            readers: HashMap::new(),
            filter,
            _filtering: filtering,
        }
    }

    fn listed_in(&self, folder: &str) -> Option<&Vec<ListedPage>> {
        self.listed.get(folder)?.as_ref()
    }

    /// How long Sikemux keeps page `id`, as its project's list last said.
    pub(crate) fn life_of(&self, id: &str) -> Option<PageLife> {
        (self.listed.values().flatten().flatten())
            .find(|page| page.id == id)
            .map(|page| page.life)
    }

    /// The link page `id` is shared through, as its project's list last said.
    pub(crate) fn share_of(&self, id: &str) -> Option<&PageShare> {
        (self.listed.values().flatten().flatten())
            .find(|page| page.id == id)
            .and_then(|page| page.shared.as_ref())
    }

    /// Page `id` is shared through `share` now, or no longer, as the core answered, until its
    /// list is read again.
    pub(crate) fn shared_now(&mut self, id: &str, share: Option<PageShare>) {
        let page = (self.listed.values_mut().flatten().flatten()).find(|page| page.id == id);
        if let Some(page) = page {
            if share.is_some() {
                page.life = PageLife::Kept;
            }
            page.shared = share;
        }
    }
}

impl Workbench {
    /// Opens the Pages of the project in front.
    pub(crate) fn open_pages(&mut self, cx: &mut Context<Self>) {
        if let Some(project) = self.project().cloned() {
            self.bring_pages(&project, &self.place(), cx);
        }
    }

    /// Opens the Pages of the project in front at page `id`.
    pub(crate) fn open_page(&mut self, id: &str, title: &str, cx: &mut Context<Self>) {
        let Some(project) = self.project().cloned() else {
            return;
        };
        self.bring_pages(&project, &self.place(), cx);
        self.read_page(&folder_of(&project), id, title, self.target(), None, cx);
    }

    /// Opens page `page` of call `id` in the Pages of the call's project, with the way back to
    /// the tab the person opened it from.
    pub(crate) fn read_call_page(&mut self, id: &str, page: &str, cx: &mut Context<Self>) {
        let Some((place, call)) = self.call_at(id) else {
            return;
        };
        let title = (call.pages())
            .find(|shown| shown.id == page)
            .map_or_else(String::new, |shown| shown.title.clone());
        let of_call = self.view_at(&place).and_then(|view| {
            (view.workspace.projects.iter())
                .find(|project| project.path == Path::new(&call.project))
        });
        let Some(project) = of_call.or_else(|| self.project()).cloned() else {
            return;
        };
        let back = self.way_back(cx);
        self.bring_pages(&project, &place, cx);
        let target = Target::new(&self.asks, place);
        self.read_page(&folder_of(&project), page, &title, target, back, cx);
    }

    /// Brings the Pages tab of `project`, at `place`, to the front and its list to the tab rail.
    fn bring_pages(&mut self, project: &ProjectInfo, place: &Place, cx: &mut Context<Self>) {
        if *place != self.place() {
            let host = match place {
                Place::This => HostChoice::This,
                Place::Account(key) => HostChoice::Account(key.clone()),
            };
            self.choose_host(host, cx);
        }
        let target = Target::new(&self.asks, place.clone());
        let folder = folder_of(project);
        target.call_typed::<call::OpenTab>(OpenTab::new(
            At::project(&project.id),
            PaneKind::Pages,
            folder.clone(),
        ));
        self.want_pages(&folder, target, cx);
        self.open_rail_on(TabRailPage::Pages, cx);
    }

    /// Brings back the tab in front now and the tab rail's page beside it; none while that tab
    /// is a project's Pages.
    fn way_back(&self, cx: &mut Context<Self>) -> Option<reader::Back> {
        let (strip, tab) = (self.strip()?, self.tab()?);
        if crate::tabs::first_view(&tab.pane).is_some_and(|(kind, _)| kind == PaneKind::Pages) {
            return None;
        }
        let (tab, place, page, bench) = (
            tab.id.clone(),
            self.place(),
            self.rails.page,
            cx.weak_entity(),
        );
        Some(std::rc::Rc::new(move |_, cx| {
            let _ = bench.update(cx, |bench, cx| {
                if place != bench.place() {
                    bench.choose_host(
                        match &place {
                            Place::This => HostChoice::This,
                            Place::Account(key) => HostChoice::Account(key.clone()),
                        },
                        cx,
                    );
                }
                if let Strip::Project(project) = &strip {
                    bench.activate_project(project);
                }
                bench.activate_tab(&strip, &tab);
                bench.show_rail_page(page, cx);
            });
        }))
    }

    /// Reads every project's pages held again, as after the core says one moved.
    pub(crate) fn pages_moved(&self, cx: &mut Context<Self>) {
        for folder in self.pages.listed.keys() {
            self.reload_pages(folder, self.target(), cx);
        }
    }

    /// Reads the pages of the project in `folder` once, to keep them from then on.
    fn want_pages(&mut self, folder: &str, target: Target, cx: &mut Context<Self>) {
        if self.pages.listed.contains_key(folder) {
            return;
        }
        self.pages.listed.insert(folder.to_owned(), None);
        self.reload_pages(folder, target, cx);
    }

    fn reload_pages(&self, folder: &str, target: Target, cx: &mut Context<Self>) {
        let asked = target.request_typed::<call::ListPages>(PagesParams {
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
                bench.redraw_pages(cx);
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

    /// Reads page `id` through `target` in the Pages of the project in `folder`, `back` before
    /// its title.
    fn read_page(
        &mut self,
        folder: &str,
        id: &str,
        title: &str,
        target: Target,
        back: Option<reader::Back>,
        cx: &mut Context<Self>,
    ) {
        let files = self.desks.read(cx).page_files.clone();
        let bench = cx.entity().downgrade();
        let reader = (self.pages.readers.entry(folder.to_owned()))
            .or_insert_with(|| Reader::new(bench, cx))
            .clone();
        let (page, title) = (id.to_owned(), title.to_owned());
        reader.update(cx, |reader, cx| {
            reader.open(page, title, target, files, back, cx)
        });
        self.saw_page(id, cx);
        self.redraw_pages(cx);
    }

    /// Draws the open page again and the list it is picked from.
    pub(crate) fn redraw_pages(&self, cx: &mut Context<Self>) {
        self.redraw_rail(Rail::Agents, cx);
        self.parts.redraw_stage(cx);
    }
}

fn folder_of(project: &ProjectInfo) -> String {
    project.path.to_string_lossy().into_owned()
}

/// The page open in the Pages of the project in `folder`: the one picked, else the newest.
pub(crate) fn pane(bench: &Workbench, folder: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let reader = bench.pages.readers.get(folder);
    let listed = bench.pages.listed_in(folder);
    if reader.is_none() {
        let newest = (listed.into_iter().flatten()).max_by_key(|page| page.shown_at);
        let newest = newest.map(|page| (page.id.clone(), page.title.clone()));
        let (folder, this) = (folder.to_owned(), cx.weak_entity());
        cx.defer(move |cx| {
            let _ = this.update(cx, |bench, cx| {
                let target = bench.target();
                match newest {
                    Some((id, title)) => bench.read_page(&folder, &id, &title, target, None, cx),
                    None => bench.want_pages(&folder, target, cx),
                }
            });
        });
    }
    let empty = (listed.is_some_and(Vec::is_empty) && reader.is_none())
        .then(|| reader::no_page(cx).into_any_element());
    ui::pane(cx)
        .child(
            div()
                .size_full()
                .bg(rows::card_fill(cx))
                .children(reader.cloned())
                .children(empty),
        )
        .into_any_element()
}

/// How long Sikemux keeps a page, in a list's tight column: the time left, or when it was shown.
pub(crate) fn life_words(life: PageLife, shown_at: u64) -> String {
    match life {
        PageLife::Kept => ui::time::age(shown_at),
        PageLife::WithCall => t!("with its ask").into(),
        PageLife::Goes { at } => t!("{left} left", left = ui::time::left_short(at)),
    }
}
