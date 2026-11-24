//! The tab rail's list of a project's pages.

use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled};
use sikemux_calls::harness::{ListedPage, PageLife};

use super::{folder_of, life_words};
use crate::agents::searched;
use crate::agents::{page_body, page_header, rail_empty};
use crate::icons::Icon;
use crate::theme::{Tokens, TEXT_2XS};
use crate::ui::{self, Row};
use crate::workbench::{actions, Workbench};

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
    let (shared, rest): (Vec<&ListedPage>, Vec<&ListedPage>) =
        (found.into_iter()).partition(|page| page.shared.is_some());
    let (kept, going): (Vec<&ListedPage>, Vec<&ListedPage>) =
        (rest.into_iter()).partition(|page| page.life == PageLife::Kept);
    let mut rows = |pages: Vec<&ListedPage>| {
        (pages.into_iter())
            .map(|page| {
                let chosen = in_front.as_deref() == Some(page.id.as_str());
                row(bench, page, chosen, cx)
            })
            .collect()
    };
    let sections = [
        (t!("Shared").into(), rows(shared)),
        (t!("Expiring").into(), rows(going)),
        (t!("Kept").into(), rows(kept)),
    ];
    searched::list(
        body,
        "pages-scroll",
        &bench.pages.search,
        None,
        empty,
        sections,
        cx,
    )
}

fn row(
    bench: &Workbench,
    page: &ListedPage,
    chosen: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let stays = match &page.shared {
        Some(share) => t!("{left} left", left = ui::time::left_short(share.expires_at)),
        None => life_words(page.life, page.shown_at),
    };
    let id = page.id.clone();
    Row::new(
        SharedString::from(format!("page-{}", page.id)),
        page.title.clone(),
    )
    .mark(
        22.,
        ui::icon(Icon::Presentation, 15.).text_color(t.text_tertiary),
    )
    .compact(bench.rail_compact())
    .chosen(chosen)
    .child(
        ui::mono(stays, TEXT_2XS, cx)
            .flex_none()
            .whitespace_nowrap(),
    )
    .on_click(cx.listener(move |bench, _, _, cx| {
        let at = bench.target();
        bench.open_page(&id, None, at, cx)
    }))
    .into_any_element()
}
