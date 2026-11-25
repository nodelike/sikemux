//! A project's Pages: every page its agents showed, listed on the tab rail in sections by how
//! long Sikemux keeps them. Each opens as a tab on an agent's desk.

pub(crate) mod list;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use gpui_kit::{App, Context, Window};
use sikemux_calls::call;
use sikemux_calls::harness::{
    KeepPageParams, ListedPage, PageLife, PageParams, PageShare, PagesParams,
};
use sikemux_view::workspace::ProjectInfo;

use crate::agents::searched::Search;
use crate::core::Target;
use crate::rail::Rail;
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

/// The pages of each project held, by folder.
pub(crate) struct Pages {
    listed: HashMap<String, Option<Vec<ListedPage>>>,
    search: Search,
    /// Each artifact in a desk's tab: its agent, its id and whether its tab is in front.
    on_desks: BTreeSet<(String, String, bool)>,
    /// Whether the pointer is over the list, which keeps every row where it is.
    pointed: bool,
    arranged: RefCell<list::Arranged>,
}

impl Pages {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        Self {
            listed: HashMap::new(),
            search: Search::new(t!("Find an artifact"), window, cx),
            on_desks: BTreeSet::new(),
            pointed: false,
            arranged: RefCell::default(),
        }
    }

    /// The list came into sight or left it, so the pointer isn't holding its rows.
    pub(crate) fn leave(&mut self) {
        self.pointed = false;
    }

    fn listed_in(&self, folder: &str) -> Option<&Vec<ListedPage>> {
        self.listed.get(folder)?.as_ref()
    }

    /// Page `id`, as its project's list last said.
    pub(crate) fn page(&self, id: &str) -> Option<&ListedPage> {
        (self.listed.values().flatten().flatten()).find(|page| page.id == id)
    }

    /// The link page `id` is shared through, as its project's list last said.
    pub(crate) fn share_of(&self, id: &str) -> Option<&PageShare> {
        self.page(id)?.shared.as_ref()
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
    /// Opens artifact `id`, which the core `at` keeps, as a tab on the desk of `agent`, such as
    /// the one whose chat links it, else of the one that showed it.
    pub(crate) fn open_page(
        &mut self,
        id: &str,
        agent: Option<&str>,
        at: Target,
        cx: &mut Context<Self>,
    ) {
        let asked = at.request_typed::<call::ReadPage>(PageParams {
            id: id.to_owned(),
            agent: agent.map(str::to_owned),
        });
        let id = id.to_owned();
        cx.spawn(async move |bench, cx| {
            let opened = match asked.await {
                Ok(page) => {
                    let opening = bench.update(cx, |bench, cx| {
                        bench.saw_page(&id, cx);
                        let agent = page.desk.clone();
                        let agent = agent.or_else(|| Some(bench.nearest_agent(cx)?.2))?;
                        let files = bench.desks.read(cx).page_files.clone();
                        Some(bench.bring_page_forward(&agent, cx, move |_| async move {
                            files.kept_url(&id, &page)
                        }))
                    });
                    match opening {
                        Ok(Some(opening)) => opening.await,
                        _ => return,
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            if let Err(error) = opened {
                let note = t!("The artifact did not open: {error}", error = error);
                let _ = bench.update(cx, |bench, cx| bench.notify(toast::error(note), cx));
            }
        })
        .detach();
    }

    /// The artifact on the desk of the agent in front, while that desk is in sight.
    pub(crate) fn front_page_id(&self, cx: &Context<Self>) -> Option<String> {
        let (.., agent) = self.front_agent()?;
        let in_sight = self.desk_in_front(&agent);
        in_sight.then(|| self.page_on_desk(&agent, cx)).flatten()
    }

    /// Each artifact open on a desk of `project`, with the agent whose desk it is: the one in
    /// front's when several hold it.
    fn open_pages(&self, project: &str, cx: &App) -> HashMap<String, String> {
        let desks = self.desks.read(cx);
        let front = self.front_agent().map(|(.., agent)| agent);
        let agents = (self.shown().into_iter())
            .flat_map(|view| &view.workspace.agents)
            .filter(|agent| agent.project == project);
        let mut open: HashMap<String, String> = HashMap::new();
        for agent in agents {
            for (page, _) in desks.artifacts_on(&agent.id) {
                if front.as_ref() == Some(&agent.id) || !open.contains_key(page) {
                    open.insert(page.to_owned(), agent.id.clone());
                }
            }
        }
        open
    }

    /// Draws the list again once an artifact's tab opened, closed or came in front.
    pub(crate) fn open_pages_moved(&mut self, cx: &mut Context<Self>) {
        let desks = self.desks.read(cx);
        let now: BTreeSet<(String, String, bool)> = (desks.strips.keys())
            .flat_map(|agent| {
                (desks.artifacts_on(agent))
                    .map(|(page, front)| (agent.clone(), page.to_owned(), front))
            })
            .collect();
        if self.pages.on_desks != now {
            self.pages.on_desks = now;
            self.redraw_rail(Rail::Agents, cx);
        }
    }

    /// Reads every project's pages held again, as after the core says one moved.
    pub(crate) fn pages_moved(&self, cx: &mut Context<Self>) {
        for folder in self.pages.listed.keys() {
            self.reload_pages(folder, self.target(), cx);
        }
    }

    /// Reads the pages of the project in `folder` once, to keep them from then on.
    pub(crate) fn want_pages(&mut self, folder: &str, target: Target, cx: &mut Context<Self>) {
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
            true => t!("The artifact was not kept"),
            false => t!("The artifact was not let go"),
        };
        self.report(keeping, what, |_, _, _| {}, cx);
    }

    /// Draws the list again, and the desks that show one of its pages.
    pub(crate) fn redraw_pages(&self, cx: &mut Context<Self>) {
        self.redraw_rail(Rail::Agents, cx);
        self.parts.redraw_stage(cx);
    }
}

fn folder_of(project: &ProjectInfo) -> String {
    project.path.to_string_lossy().into_owned()
}

/// How long Sikemux keeps a page, in a list's tight column: the time left, or when it was shown.
pub(crate) fn life_words(life: PageLife, shown_at: u64) -> String {
    match life {
        PageLife::Kept => ui::time::age(shown_at),
        PageLife::WithCall => t!("with its ask").into(),
        PageLife::Goes { at } => t!("{left} left", left = ui::time::left_short(at)),
    }
}

/// What an open page's Keep says: how long the page has left, or that it is kept.
pub(crate) fn keep_label(life: PageLife) -> String {
    match life {
        PageLife::Kept => t!("Kept · press to let it go").into(),
        PageLife::WithCall => t!("Keep · goes a day after you answer").into(),
        PageLife::Goes { at } => t!("Keep · goes in {left}", left = ui::time::left_short(at)),
    }
}
