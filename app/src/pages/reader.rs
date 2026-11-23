//! The one view a page is read in, inside the window: its title, how long Sikemux keeps it and
//! Keep over it, and the page under them, its links opening in the person's browser.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, App, AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    Styled, WeakEntity, Window,
};
use sikemux_calls::call;
use sikemux_calls::harness::{PageLife, PageParams};

use crate::core::Target;
use crate::desk::PageFiles;
use crate::icons::Icon;
use crate::page_view::{self, PageEvent, Room};
use crate::theme::{Tokens, SPACE_2};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// What the reader's Back does, as the Calls page's return to its call.
pub(crate) type Back = Rc<dyn Fn(&mut Window, &mut App)>;

pub(crate) struct Reader {
    bench: WeakEntity<Workbench>,
    shown: Option<Shown>,
    back: Option<Back>,
    /// The page as read, until the next frame makes its view.
    ready: Option<(String, String)>,
    page: Option<page_view::View>,
    problem: Option<SharedString>,
}

struct Shown {
    id: String,
    title: String,
}

impl Reader {
    pub(crate) fn new(bench: WeakEntity<Workbench>, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            bench,
            shown: None,
            back: None,
            ready: None,
            page: None,
            problem: None,
        })
    }

    /// The id of the page it shows.
    pub(crate) fn shows(&self) -> Option<&str> {
        self.shown.as_ref().map(|shown| shown.id.as_str())
    }

    /// Shows page `id`, read through `target`, with `back` before its title.
    pub(crate) fn open(
        &mut self,
        id: String,
        title: String,
        target: Target,
        files: Arc<PageFiles>,
        back: Option<Back>,
        cx: &mut Context<Self>,
    ) {
        self.back = back;
        if self.shows() == Some(id.as_str()) {
            cx.notify();
            return;
        }
        self.shown = Some(Shown {
            id: id.clone(),
            title,
        });
        (self.ready, self.page, self.problem) = (None, None, None);
        cx.notify();
        let asked = target.request_typed::<call::ReadPage>(PageParams { id: id.clone() });
        cx.spawn(async move |this, cx| {
            let read = match asked.await {
                Ok(page) => {
                    let id = id.clone();
                    (cx.background_spawn(async move { files.served(&id, &page) })).await
                }
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |reader, cx| {
                if reader.shows() != Some(id.as_str()) {
                    return;
                }
                match read {
                    Ok(served) => reader.ready = Some(served),
                    Err(error) => reader.problem = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn make_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((url, html)) = self.ready.take() else {
            return;
        };
        let (events, mut heard) = tokio::sync::mpsc::unbounded_channel();
        self.page = page_view::open(&html, Room::Served(&url), events, window, cx);
        cx.spawn(async move |_, cx| {
            while let Some(event) = heard.recv().await {
                if let PageEvent::Link(url) = event {
                    cx.update(|cx| cx.open_url(&url));
                }
            }
        })
        .detach();
    }
}

impl Render for Reader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.make_view(window, cx);
        let t = Tokens::get(cx).clone();
        let Some(shown) = &self.shown else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(SPACE_2)
                .child(ui::icon(Icon::Tabs, 18.).text_color(t.text_disabled))
                .child(ui::meta(t!("No page open"), cx))
                .into_any_element();
        };
        let life = (self.bench.upgrade()).and_then(|bench| bench.read(cx).pages.life_of(&shown.id));
        let keep = life.map(|life| {
            let (id, bench, kept) = (shown.id.clone(), self.bench.clone(), life == PageLife::Kept);
            let (label, variant) = match kept {
                true => (t!("Kept"), Variant::Ghost),
                false => (t!("Keep"), Variant::Outline),
            };
            Button::new("page-keep", label, variant)
                .icon(Icon::Pin)
                .on_click(move |_, _, cx| {
                    let _ = bench.update(cx, |bench, cx| bench.keep_page(&id, !kept, cx));
                })
        });
        let fact = life.map(|life| ui::meta(stays(life), cx));
        let back = self.back.clone().map(|back| {
            Button::new("page-back", t!("Back to call"), Variant::Ghost)
                .icon(Icon::ArrowLeft)
                .on_click(move |_, window, cx| back(window, cx))
        });
        let body = match (&self.page, &self.problem) {
            (_, Some(problem)) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::meta(problem.clone(), cx))
                .into_any_element(),
            (Some(page), None) => gpui_native_view::native_view(
                SharedString::from(format!("page-reader-{}", shown.id)),
                &page.surface,
            )
            .size_full()
            .into_any_element(),
            (None, None) => div().size_full().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                ui::PaneHeader::new(shown.title.clone())
                    .when_some(back, |header, back| header.back(back))
                    .when_some(fact, |header, fact| header.fact(fact))
                    .when_some(keep, |header, keep| header.action(keep)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_t(crate::theme::border())
                    .border_color(t.border)
                    .child(body),
            )
            .into_any_element()
    }
}

/// How long Sikemux keeps a page, as its header says.
fn stays(life: PageLife) -> String {
    match life {
        PageLife::Kept => t!("Kept until you let it go").into(),
        PageLife::WithCall => t!("Goes a day after its call settles").into(),
        PageLife::Goes { at } => t!("Goes in {left}", left = super::left(at)),
    }
}
