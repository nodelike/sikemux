//! The Pull requests page: the repository's open pull requests. Choosing one can start an agent
//! that reviews it or answers its comments.

use crate::ui::Clicks;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, FontWeight, HoverListenerMode, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_calls::board::BoardHandOffParams;
use sikemux_calls::call;
use sikemux_calls::codehost::PullsParams;
use sikemux_plugin_api::codehost::{ListedPull, PullList};

use super::{GitPane, Pick};
use crate::code_host_marks::pull_look;
use crate::theme::{
    radius_2, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_5, TEXT_3XS, TEXT_XS,
};
use crate::toast;
use crate::ui;

/// What the page knows: the open pull requests.
#[derive(Default)]
pub(super) struct Pulls {
    pub(super) listed: Vec<ListedPull>,
    pub(super) read: bool,
    pub(super) chosen: usize,
}

impl GitPane {
    /// Reads the open pull requests.
    pub(super) fn read_pulls(&mut self, cx: &mut Context<Self>) {
        if self.host_of().is_none() {
            return;
        }
        let listed = self.core.request_typed::<call::CodeHostPulls>(PullsParams {
            project: self.repo.clone(),
            state: Some("open".to_owned()),
        });
        cx.spawn(async move |pane, cx| {
            let list: Option<PullList> = listed.await.ok().and_then(|answer| answer.read().ok());
            pane.update(cx, |pane, cx| {
                pane.pulls.listed = list.map(|list| list.pulls).unwrap_or_default();
                pane.pulls.read = true;
                pane.pulls.chosen = pane
                    .pulls
                    .chosen
                    .min(pane.pulls.listed.len().saturating_sub(1));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn step_pull(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.pulls.listed.len().saturating_sub(1);
        self.pulls.chosen = self.pulls.chosen.saturating_add_signed(by).min(last);
        cx.notify();
    }

    /// Offers to start an agent on the chosen pull request.
    pub(super) fn open_pull(&mut self, cx: &mut Context<Self>) {
        let Some(listed) = self.pulls.listed.get(self.pulls.chosen) else {
            return;
        };
        let number = listed.pull.number;
        self.menu(
            format!("#{number} · {}", listed.pull.title),
            vec![
                Pick::new('r', t!("review this"), |pane, cx| {
                    pane.start_on_pull(Task::Review, cx)
                }),
                Pick::new('f', t!("fix the comments"), |pane, cx| {
                    pane.start_on_pull(Task::Answer, cx)
                }),
            ],
            cx,
        );
    }

    /// Starts an agent in a checkout of its own on a card that works the chosen pull request.
    fn start_on_pull(&mut self, task: Task, cx: &mut Context<Self>) {
        let Some(listed) = self.pulls.listed.get(self.pulls.chosen) else {
            return;
        };
        let pull = &listed.pull;
        let branch = pull.head.clone().unwrap_or_default();
        let text = match task {
            Task::Review => format!(
                "Review pull request #{} \"{}\" ({}) and say what must change before it merges.",
                pull.number, pull.title, pull.url
            ),
            Task::Answer => format!(
                "Answer the review comments on pull request #{} \"{}\" ({}), pushing the fixes to its branch `{branch}`.",
                pull.number, pull.title, pull.url
            ),
        };
        let asked = self
            .core
            .request_typed::<call::BoardHandOff>(BoardHandOffParams {
                project: self.repo.clone(),
                text,
                files: Vec::new(),
                launcher: None,
                model: None,
                provider: None,
                permission_mode: None,
                effort: None,
            });
        let number = pull.number;
        cx.spawn(async move |pane, cx| {
            let handed = asked.await;
            pane.update(cx, |pane, cx| match handed {
                Ok(handed) => {
                    pane.say(
                        toast::success(t!(
                            "Started card #{card} on #{number}",
                            card = handed.card,
                            number = number
                        )),
                        cx,
                    );
                    pane.read_pulls(cx);
                }
                Err(error) => pane.say(toast::error(error), cx),
            })
            .ok();
        })
        .detach();
    }
}

#[derive(Clone, Copy)]
enum Task {
    Review,
    Answer,
}

pub(super) fn pulls_page(pane: &mut GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let note = match (
        pane.host_of().is_some(),
        pane.pulls.read,
        pane.pulls.listed.is_empty(),
    ) {
        (false, ..) => Some(t!(
            "This repository is on no code host Sikemux is signed in to."
        )),
        (true, false, _) => Some(t!("Reading pull requests…")),
        (true, true, true) => Some(t!("No open pull requests.")),
        _ => None,
    };
    let query = pane.filters[super::Page::Pulls as usize].query.clone();
    let rows: Vec<AnyElement> = (pane.pulls.listed.iter().enumerate())
        .filter(|(_, listed)| query.is_empty() || listed.pull.title.to_lowercase().contains(&query))
        .map(|(index, listed)| pull_row(&view, pane, listed, index, cx))
        .collect();
    div()
        .id("git-pulls")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(SPACE_1H)
        .pt(SPACE_1)
        .children(note.map(|note| {
            div()
                .p(SPACE_5)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(note)
        }))
        .children(rows)
}

fn pull_row(
    view: &Entity<GitPane>,
    pane: &GitPane,
    listed: &ListedPull,
    index: usize,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let chosen = index == pane.pulls.chosen;
    let key = SharedString::from(format!("pull:{index}"));
    let (icon, ink, _) = pull_look(listed.state, &t);
    let mark = ui::mark_box(14., ui::icon(icon, 13.).text_color(ink)).into_any_element();
    let mut meta: Vec<AnyElement> = Vec::new();
    let said = |text: String, ink| {
        div()
            .flex_none()
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .text_color(ink)
            .child(text)
            .into_any_element()
    };
    if let Some(author) = &listed.pull.author {
        meta.push(said(format!("@{author}"), t.text_tertiary));
    }
    let end = div()
        .flex_none()
        .px(SPACE_1H)
        .py(SPACE_0)
        .rounded(radius_2())
        .border(crate::theme::border())
        .border_color(t.border)
        .text_size(crate::theme::ui_text(TEXT_3XS))
        .text_color(t.text_secondary)
        .child(t!("Start"));
    let choosing = view.clone();
    div()
        .id(key.clone())
        .w_full()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .px(SPACE_1H)
        .py(SPACE_1H)
        .rounded(radius_2())
        .cursor_pointer()
        .map(|row| {
            if chosen {
                row.bg(t.surface_chosen)
            } else {
                row.hover(move |row| row.bg(t.surface_hover))
            }
        })
        .on_hover(super::hover(view, key))
        .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
        .clicks(move |_, window, cx| {
            choosing.update(cx, |pane, cx| {
                window.focus(&pane.focus, cx);
                pane.pulls.chosen = index;
                pane.open_pull(cx);
            })
        })
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    ui::text(
                        format!("#{} {}", listed.pull.number, listed.pull.title),
                        TEXT_XS,
                        FontWeight::MEDIUM,
                    )
                    .truncate()
                    .text_color(t.text_primary),
                )
                .child(div().flex().items_center().gap(SPACE_1H).children(meta)),
        )
        .child(end)
        .into_any_element()
}
