//! A page of the tab rail that lists in sections what a search over it finds.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Div, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
};

use super::{faded, EDGE_FADE};
use crate::icons::Icon;
use crate::rail::Rail;
use crate::theme::{SPACE_0, SPACE_2H, SPACE_3};
use crate::ui::{Field, Group};
use crate::workbench::Workbench;

/// What is typed in a page's search field, which redraws the rail as it changes.
pub(crate) struct Search {
    state: Entity<InputState>,
    _typing: Subscription,
}

impl Search {
    pub(crate) fn new(
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) -> Self {
        let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let typing = cx.subscribe(&state, |bench, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                bench.redraw_rail(Rail::Agents, cx);
            }
        });
        Self {
            state,
            _typing: typing,
        }
    }

    pub(crate) fn words(&self, cx: &App) -> String {
        self.state.read(cx).value().trim().to_lowercase()
    }
}

/// Whether `text` holds every one of `words`, an agent's name with or without its @.
pub(crate) fn holds(text: &str, words: &str) -> bool {
    let text = text.to_lowercase();
    (words.split_whitespace()).all(|word| text.contains(word.trim_start_matches('@')))
}

/// Under `body`'s header: the search field, what makes a new one, then the sections that have
/// rows, or `empty`.
pub(crate) fn list(
    body: Div,
    id: &'static str,
    search: &Search,
    new: Option<AnyElement>,
    empty: Option<AnyElement>,
    sections: impl IntoIterator<Item = (SharedString, Vec<AnyElement>)>,
    cx: &App,
) -> AnyElement {
    let sections = (sections.into_iter())
        .filter(|(_, rows)| !rows.is_empty())
        .enumerate()
        .map(|(index, (name, rows))| {
            Group::new(name, "")
                .when(index == 0, Group::first)
                .children(rows)
        });
    body.child(
        div()
            .flex_none()
            .mt(SPACE_2H)
            .mx(SPACE_3)
            .mb(SPACE_0)
            .child(Field::new(&search.state).icon(Icon::Search)),
    )
    .children(new.map(|new| div().flex_none().pt(SPACE_0).px(SPACE_2H).child(new)))
    .child(faded(
        div()
            .id(id)
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
