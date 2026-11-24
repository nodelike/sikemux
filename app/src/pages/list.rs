//! The tab rail's list of a project's pages.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled,
};
use sikemux_calls::harness::{ListedPage, PageLife};
use sikemux_view::layout::PaneKind;

use super::{folder_of, left, life_words};
use crate::agents::{faded, page_body, page_header, rail_empty, EDGE_FADE};
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_0, SPACE_2H, SPACE_3, TEXT_2XS};
use crate::ui::{self, Field, Group, Row};
use crate::workbench::{actions, Workbench};

pub(crate) fn page(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let body = page_body().child(page_header(t!("Pages"), bench, cx));
    let Some(project) = bench.project() else {
        return body
            .child(rail_empty(
                Icon::Tabs,
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
    let words = bench.pages.filter.read(cx).value().trim().to_lowercase();
    let found: Vec<&ListedPage> = (listed.into_iter().flatten())
        .filter(|page| matches(page, &words))
        .collect();
    let empty = (listed.is_some() && found.is_empty()).then(|| match words.is_empty() {
        true => rail_empty(Icon::Tabs, t!("No pages yet"), None, cx),
        false => rail_empty(Icon::Search, t!("No page matches"), None, cx),
    });
    let in_front = (bench.tab())
        .and_then(|tab| crate::tabs::first_view(&tab.pane))
        .is_some_and(|(kind, _)| kind == PaneKind::Pages);
    let reading = (bench.pages.readers.get(&folder))
        .filter(|_| in_front)
        .and_then(|reader| reader.read(cx).shows().map(str::to_owned));
    let (shared, rest): (Vec<&ListedPage>, Vec<&ListedPage>) =
        (found.into_iter()).partition(|page| page.shared.is_some());
    let (kept, going): (Vec<&ListedPage>, Vec<&ListedPage>) =
        (rest.into_iter()).partition(|page| page.life == PageLife::Kept);
    let mut sections: Vec<AnyElement> = Vec::new();
    for (name, pages) in [
        (t!("Shared"), shared),
        (t!("Expiring"), going),
        (t!("Kept"), kept),
    ] {
        if pages.is_empty() {
            continue;
        }
        let rows: Vec<AnyElement> = (pages.into_iter())
            .map(|page| {
                row(
                    bench,
                    page,
                    reading.as_deref() == Some(page.id.as_str()),
                    cx,
                )
            })
            .collect();
        let group = Group::new(name, "").when(sections.is_empty(), Group::first);
        sections.push(group.children(rows).into_any_element());
    }
    body.child(
        div()
            .flex_none()
            .mt(SPACE_2H)
            .mx(SPACE_3)
            .mb(SPACE_0)
            .child(Field::new(&bench.pages.filter).icon(Icon::Search)),
    )
    .child(faded(
        div()
            .id("pages-scroll")
            .size_full()
            .pt(px(EDGE_FADE))
            .px(SPACE_2H)
            .pb(px(EDGE_FADE))
            .overflow_y_scroll()
            .children(empty)
            .children(sections),
        cx,
    ))
    .into_any_element()
}

fn row(
    bench: &Workbench,
    page: &ListedPage,
    chosen: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let stays = match &page.shared {
        Some(share) => t!("{left} left", left = left(share.expires_at)),
        None => life_words(page.life, page.shown_at),
    };
    let (id, title) = (page.id.clone(), page.title.clone());
    Row::new(
        SharedString::from(format!("page-{}", page.id)),
        page.title.clone(),
    )
    .mark(22., ui::icon(Icon::Tabs, 15.).text_color(t.text_tertiary))
    .compact(bench.rail_compact())
    .chosen(chosen)
    .child(
        ui::mono(stays, TEXT_2XS, cx)
            .flex_none()
            .whitespace_nowrap(),
    )
    .on_click(cx.listener(move |bench, _, _, cx| bench.open_page(&id, &title, cx)))
    .into_any_element()
}

/// Whether `page` holds every word of `words` in its title or its agent's name.
fn matches(page: &ListedPage, words: &str) -> bool {
    let named = format!("{} {}", page.title, page.agent).to_lowercase();
    (words.split_whitespace()).all(|word| named.contains(word.trim_start_matches('@')))
}
