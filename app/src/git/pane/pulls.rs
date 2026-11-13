//! The Pull requests page: the repository's open pull requests, each with how the slice behind
//! it stands, the evidence its gate leads with and its checks. Choosing one opens its slice; one
//! with no slice can start one that reviews it or answers its comments.

use std::collections::HashMap;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, FontWeight, HoverListenerMode, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_calls::board::BoardHandOffParams;
use sikemux_calls::board::BoardParams;
use sikemux_calls::board::ProjectBoard;
use sikemux_calls::call;
use sikemux_calls::codehost::PullsParams;
use sikemux_plugin_api::codehost::{ListedPull, PullList};
use sikemux_view::slice::{Evidence, GateState, Proof, SliceState, Verdict};

use super::{GitPane, Pick};
use crate::code_host_marks::pull_look;
use crate::theme::{
    Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_5, TEXT_3XS, TEXT_XS,
};
use crate::toast;
use crate::ui;

/// What the page knows: the open pull requests and the slice behind each, by its address.
#[derive(Default)]
pub(super) struct Pulls {
    pub(super) listed: Vec<ListedPull>,
    pub(super) read: bool,
    slices: HashMap<String, Behind>,
    pub(super) chosen: usize,
}

/// The slice a pull request's work is, as the core keeps it.
#[derive(Clone)]
struct Behind {
    /// The agent at work on it, whose chat choosing the pull request opens.
    owner: Option<String>,
    state: SliceState,
    gate: Option<&'static str>,
    lead: Option<Evidence>,
    passed: usize,
    failed: usize,
}

fn behind(board: &ProjectBoard) -> HashMap<String, Behind> {
    let mut found = HashMap::new();
    for card in &board.cards {
        let Some(slice) = &card.slice else { continue };
        let Some(url) = &slice.pull else { continue };
        let ruled = |verdict| {
            (slice.evidence.iter())
                .filter(|evidence| evidence.verdict == Some(verdict))
                .count()
        };
        let (passed, failed) = (ruled(Verdict::Passed), ruled(Verdict::Failed));
        let gate = (slice.gates.iter())
            .find(|gate| gate.state == GateState::Waiting)
            .map(|gate| crate::ui::slice_state::gate_name(gate.gate));
        let lead = (slice.leading)
            .and_then(|id| slice.evidence.iter().find(|evidence| evidence.id == id))
            .filter(|evidence| matches!(evidence.proof, Proof::Picture { seconds: None, .. }))
            .cloned();
        found.insert(
            url.clone(),
            Behind {
                owner: card.owner.clone(),
                state: slice.state,
                gate,
                lead,
                passed,
                failed,
            },
        );
    }
    found
}

impl GitPane {
    /// Reads the open pull requests and the board that says which slice each is.
    pub(super) fn read_pulls(&mut self, cx: &mut Context<Self>) {
        if self.host_of().is_none() {
            return;
        }
        let listed = self.core.request_typed::<call::CodeHostPulls>(PullsParams {
            project: self.repo.clone(),
            state: Some("open".to_owned()),
        });
        let board = self.core.request_typed::<call::Board>(BoardParams {
            project: self.repo.clone(),
        });
        cx.spawn(async move |pane, cx| {
            let list: Option<PullList> = listed.await.ok().and_then(|answer| answer.read().ok());
            let slices = board.await.map(|board| behind(&board)).unwrap_or_default();
            pane.update(cx, |pane, cx| {
                pane.pulls.listed = list.map(|list| list.pulls).unwrap_or_default();
                pane.pulls.read = true;
                pane.pulls.slices = slices;
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

    /// Opens the chat of the agent at work on the chosen pull request, or offers to start one
    /// on it when none is.
    pub(super) fn open_pull(&mut self, cx: &mut Context<Self>) {
        let Some(listed) = self.pulls.listed.get(self.pulls.chosen) else {
            return;
        };
        let owner = (self.pulls.slices.get(&listed.pull.url)).and_then(|slice| slice.owner.clone());
        if let Some(owner) = owner {
            let _ = self
                .bench
                .update(cx, |bench, cx| bench.bring_agent(&owner, cx).detach());
            return;
        }
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

    /// Starts an agent in a checkout of its own on a slice that works the chosen pull request.
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
    let slice = pane.pulls.slices.get(&listed.pull.url);
    let mark = match slice {
        Some(slice) => crate::ui::slice_state::state_mark(slice.state, cx),
        None => {
            let (icon, ink, _) = pull_look(listed.state, &t);
            ui::mark_box(14., ui::icon(icon, 13.).text_color(ink)).into_any_element()
        }
    };
    let mut meta: Vec<AnyElement> = Vec::new();
    let said = |text: String, ink| {
        div()
            .flex_none()
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .text_color(ink)
            .child(text)
            .into_any_element()
    };
    match slice {
        Some(slice) => {
            meta.push(said(
                slice.gate.unwrap_or(t!("Slice")).to_owned(),
                t.text_secondary,
            ));
            if slice.passed > 0 {
                meta.push(said(format!("✓{}", slice.passed), t.live));
            }
            if slice.failed > 0 {
                meta.push(said(format!("✗{}", slice.failed), t.danger));
            }
        }
        None => meta.push(said(t!("No slice").to_owned(), t.text_tertiary)),
    }
    if let Some(author) = &listed.pull.author {
        meta.push(said(format!("@{author}"), t.text_tertiary));
    }
    let end: Option<AnyElement> = match slice {
        Some(slice) => (slice.lead.as_ref()).map(|lead| {
            crate::ui::evidence::thumb_face(lead, false, cx)
                .w(px(44.))
                .h(px(26.))
                .into_any_element()
        }),
        None => Some(
            div()
                .flex_none()
                .px(SPACE_1H)
                .py(SPACE_0)
                .rounded(RADIUS_2)
                .border_1()
                .border_color(t.border)
                .text_size(crate::theme::ui_text(TEXT_3XS))
                .text_color(t.text_secondary)
                .child(t!("Start"))
                .into_any_element(),
        ),
    };
    let choosing = view.clone();
    div()
        .id(key.clone())
        .w_full()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .px(SPACE_1H)
        .py(SPACE_1H)
        .rounded(RADIUS_2)
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
        .on_click(move |_, window, cx| {
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
        .children(end)
        .into_any_element()
}
