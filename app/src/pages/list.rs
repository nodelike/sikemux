//! The tab rail's list of a project's pages.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use gpui_kit::{
    canvas, div, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled,
};
use sikemux_calls::harness::{ListedPage, PageLife};

use super::{folder_of, life_words};
use crate::agents::searched;
use crate::agents::{page_body, page_header, rail_empty};
use crate::icons::Icon;
use crate::rail::Rail;
use crate::theme::{Tokens, TEXT_2XS};
use crate::ui::{self, Row};
use crate::workbench::{actions, Workbench};

const SECTIONS: usize = 4;

/// The rows each section was last drawn with, in order, and what they were found by.
#[derive(Default)]
pub(crate) struct Arranged {
    by: (String, String),
    sections: [Vec<u64>; SECTIONS],
}

pub(crate) fn page(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let body = page_body().child(page_header(t!("Artifacts"), bench, cx));
    let Some(project) = bench.project() else {
        return body
            .child(rail_empty(
                Icon::Presentation,
                t!("No project open"),
                Some((actions::OpenProject::ID, t!("to open one"))),
                cx,
            ))
            .into_any_element();
    };
    let folder = folder_of(project);
    let listed = bench.pages.listed_in(&folder);
    if !bench.pages.listed.contains_key(&folder) {
        let (this, folder) = (cx.weak_entity(), folder.clone());
        cx.defer(move |cx| {
            let _ = this.update(cx, |bench, cx| {
                let target = bench.target();
                bench.want_pages(&folder, target, cx)
            });
        });
    }
    let words = bench.pages.search.words(cx);
    let found: Vec<&ListedPage> = (listed.into_iter().flatten())
        .filter(|page| searched::holds(&format!("{} {}", page.title, page.agent), &words))
        .collect();
    let empty = (listed.is_some() && found.is_empty()).then(|| match words.is_empty() {
        true => rail_empty(Icon::Presentation, t!("No artifacts yet"), None, cx),
        false => rail_empty(Icon::Search, t!("No artifact matches"), None, cx),
    });
    let in_front = bench.front_page_id(cx);
    let open = bench.open_pages(&project.id, cx);
    let (working, found): (Vec<&ListedPage>, Vec<&ListedPage>) =
        (found.into_iter()).partition(|page| open.contains_key(&page.id));
    let (shared, rest): (Vec<&ListedPage>, Vec<&ListedPage>) =
        (found.into_iter()).partition(|page| page.shared.is_some());
    let (kept, going): (Vec<&ListedPage>, Vec<&ListedPage>) =
        (rest.into_iter()).partition(|page| page.life == PageLife::Kept);
    let [working, shared, going, kept] =
        arranged(bench, (folder, words), [working, shared, going, kept]);
    let mut rows = |pages: Vec<&ListedPage>| {
        (pages.into_iter())
            .map(|page| {
                let chosen = in_front.as_deref() == Some(page.id.as_str());
                let open_on = open.get(&page.id).map(String::as_str);
                row(bench, page, open_on, chosen, cx)
            })
            .collect()
    };
    let sections = [
        (t!("Working").into(), rows(working)),
        (t!("Shared").into(), rows(shared)),
        (t!("Expiring").into(), rows(going)),
        (t!("Kept").into(), rows(kept)),
    ];
    let listed = searched::list(
        body,
        "pages-scroll",
        &bench.pages.search,
        None,
        empty,
        sections,
        cx,
    );
    let this = cx.weak_entity();
    let drawn_away = canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            if bounds.contains(&window.mouse_position()) {
                return;
            }
            let this = this.clone();
            cx.defer(move |cx| {
                let _ = this.update(cx, |bench, cx| bench.point_at_pages(false, cx));
            });
        },
    )
    .absolute()
    .inset_0();
    div()
        .id("pages-list")
        .size_full()
        .relative()
        .on_hover(cx.listener(|bench, over: &bool, _, cx| bench.point_at_pages(*over, cx)))
        .child(listed)
        .child(drawn_away)
        .into_any_element()
}

impl Workbench {
    /// The pointer came over the list, or left it, when its rows go where they belong now.
    fn point_at_pages(&mut self, over: bool, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.pages.pointed, over) && !over {
            self.redraw_rail(Rail::Agents, cx);
        }
    }
}

/// `sections` as they are drawn: as they were while the pointer is over them, so no row moves
/// from under it. A page that came since waits until the pointer leaves.
fn arranged<'a>(
    bench: &Workbench,
    by: (String, String),
    sections: [Vec<&'a ListedPage>; SECTIONS],
) -> [Vec<&'a ListedPage>; SECTIONS] {
    let mut drawn = bench.pages.arranged.borrow_mut();
    let keys = sections
        .each_ref()
        .map(|pages| pages.iter().map(|page| key(page)).collect::<Vec<u64>>());
    let held = bench.pages.pointed && drawn.by == by;
    if !held || drawn.sections.iter().all(Vec::is_empty) {
        *drawn = Arranged { by, sections: keys };
        return sections;
    }
    let found: HashMap<u64, &ListedPage> = (keys
        .into_iter()
        .flatten()
        .zip(sections.into_iter().flatten()))
    .collect();
    (drawn.sections.each_ref()).map(|keys| {
        (keys.iter())
            .filter_map(|key| found.get(key).copied())
            .collect()
    })
}

/// The same for every version of a page.
fn key(page: &ListedPage) -> u64 {
    let mut hasher = DefaultHasher::new();
    (&page.agent, &page.title).hash(&mut hasher);
    hasher.finish()
}

fn row(
    bench: &Workbench,
    page: &ListedPage,
    open_on: Option<&str>,
    chosen: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let stays = match &page.shared {
        Some(share) => t!("{left} left", left = ui::time::left_short(share.expires_at)),
        None => life_words(page.life, page.shown_at),
    };
    let ink = match open_on {
        Some(_) => t.accent,
        None => t.text_tertiary,
    };
    let (id, open_on) = (page.id.clone(), open_on.map(str::to_owned));
    Row::new(
        SharedString::from(format!("page-{}", page.id)),
        page.title.clone(),
    )
    .mark(22., ui::icon(Icon::Presentation, 15.).text_color(ink))
    .compact(bench.rail_compact())
    .chosen(chosen)
    .child(
        ui::mono(stays, TEXT_2XS, cx)
            .flex_none()
            .whitespace_nowrap(),
    )
    .on_click(cx.listener(move |bench, _, _, cx| {
        let (at, front) = (bench.target(), bench.front_agent());
        let agent = open_on.clone().or(front.map(|(.., agent)| agent));
        bench.open_page(&id, agent.as_deref(), at, cx)
    }))
    .into_any_element()
}
