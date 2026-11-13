//! What an agent did in its worktree, opened from its card in the rail: its branch, what it
//! said it did, the files it changed, its tests and the server it runs, as the core reads them.

use crate::ui::Clicks;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, App, AppContext, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Styled, WeakEntity, Window,
};

use crate::core::Target;
use crate::editor::OpenAt;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_2XS_H, TEXT_SM,
    TEXT_XS,
};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;
use sikemux_calls::call;
use sikemux_calls::worktree::WorktreeParams;
use sikemux_calls::worktree::WorktreeReview;

const SHOWN_FILES: usize = 6;

type Dismiss = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct AgentReview {
    bench: WeakEntity<Workbench>,
    agent: String,
    found: Option<Result<WorktreeReview, SharedString>>,
    close: Dismiss,
    /// Takes the keyboard while the review shows, so Escape closes it.
    focus: FocusHandle,
    /// What had the keyboard before, once the review took it.
    before: Option<Option<FocusHandle>>,
}

impl AgentReview {
    fn new(
        bench: WeakEntity<Workbench>,
        core: &Target,
        agent: String,
        close: Dismiss,
        cx: &mut Context<Self>,
    ) -> Self {
        let read = core.request_typed::<call::AgentReview>(WorktreeParams {
            agent_id: agent.clone(),
        });
        cx.spawn(async move |review, cx| {
            let found = read.await.map_err(SharedString::from);
            review
                .update(cx, |review, cx| {
                    review.found = Some(found);
                    cx.notify();
                })
                .ok();
        })
        .detach();
        Self {
            bench,
            agent,
            found: None,
            close,
            focus: cx.focus_handle(),
            before: None,
        }
    }

    /// Closes the review and gives the keyboard back to what had it.
    fn leave(&mut self, window: &mut Window, cx: &mut App) {
        (self.close)(window, cx);
        if let Some(Some(before)) = self.before.take() {
            window.focus(&before, cx);
        }
    }

    /// Closes the review, then does `then` on the workbench.
    fn then(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        then: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>),
    ) {
        self.leave(window, cx);
        self.bench
            .update(cx, |bench, cx| then(bench, window, cx))
            .ok();
    }
}

impl Render for AgentReview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.before.is_none() {
            self.before = Some(window.focused(cx));
            window.focus(&self.focus, cx);
        }
        let t = Tokens::get(cx).clone();
        let row = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_color(t.text_tertiary)
                        .child(label),
                )
        };
        let agent = self.bench.upgrade().and_then(|bench| {
            let bench = bench.read(cx);
            bench
                .projects()
                .iter()
                .flat_map(|project| bench.agents_of(&project.id))
                .find(|row| row.id == self.agent)
        });
        let title = agent
            .as_ref()
            .map(|agent| {
                agent
                    .handle()
                    .map_or_else(|| agent.title.clone(), str::to_owned)
            })
            .unwrap_or_else(|| t!("Agent").into());
        let mut body = div().flex().flex_col().gap(SPACE_2);
        let mut actions = div().flex().flex_wrap().gap(SPACE_1H).child(
            Button::new("review-show", t!("Show agent"), Variant::Outline).on_click(cx.listener(
                |review, _, window, cx| {
                    let agent = review.agent.clone();
                    review.then(window, cx, move |bench, _, cx| bench.open_agent(&agent, cx));
                },
            )),
        );
        match &self.found {
            None => body = body.child(ui::meta(t!("Reading the worktree…"), cx)),
            Some(Err(error)) => {
                body = body.child(ui::meta(
                    t!("Could not read the worktree: {error}", error = error),
                    cx,
                ))
            }
            Some(Ok(found)) => {
                body = body
                    .child(
                        ui::mono(found.branch.clone(), TEXT_2XS_H, cx).text_color(t.text_secondary),
                    )
                    .children(found.summary.clone().map(|summary| {
                        div()
                            .text_size(crate::theme::ui_text(TEXT_XS))
                            .text_color(t.text_primary)
                            .child(summary)
                    }));
                let changes = if found.changed.is_empty() {
                    t!("No changes yet").to_owned()
                } else {
                    let mut words = match found.changed.len() {
                        1 => t!("1 file").to_owned(),
                        count => t!("{count} files", count = count),
                    };
                    if found.uncommitted > 0 {
                        words.push_str(" · ");
                        words.push_str(&t!("{count} not committed", count = found.uncommitted));
                    }
                    words
                };
                body = body.child(row(t!("Changes")).child(changes));
                body = body.children(found.changed.iter().take(SHOWN_FILES).enumerate().map(
                    |(index, file)| {
                        let path = found.path.join(file);
                        div()
                            .id(("review-file", index))
                            .pl(px(72.))
                            .truncate()
                            .font_family(theme::MONO)
                            .text_size(crate::theme::ui_text(TEXT_2XS))
                            .text_color(t.accent)
                            .hover(|style| style.underline())
                            .clicks(cx.listener(move |review, _, window, cx| {
                                let at = OpenAt {
                                    path: path.clone(),
                                    line: 1,
                                    column: None,
                                };
                                review.leave(window, cx);
                                window.dispatch_action(Box::new(at), cx);
                            }))
                            .child(file.clone())
                    },
                ));
                if found.changed.len() > SHOWN_FILES {
                    let more = found.changed.len() - SHOWN_FILES;
                    body = body.child(ui::meta(t!("and {more} more", more = more), cx).pl(px(72.)));
                }
                let tests = if !found.tested {
                    ui::meta(t!("No results yet"), cx)
                } else if found.failed > 0 {
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1)
                        .text_color(t.danger)
                        .child(ui::icon(Icon::Close, 11.).text_color(t.danger))
                        .child(t!(
                            "{failed} failed · {passed} passed",
                            failed = found.failed,
                            passed = found.passed
                        ))
                } else {
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1)
                        .text_color(t.live)
                        .child(ui::icon(Icon::Check, 11.).text_color(t.live))
                        .child(t!("{passed} passed", passed = found.passed))
                };
                body = body.child(row(t!("Tests")).child(tests));
                body = body.child(
                    row(t!("Running")).child(match &found.url {
                        Some(url) => {
                            ui::mono(url.trim_start_matches("http://").to_owned(), TEXT_2XS_H, cx)
                                .text_color(t.text_primary)
                        }
                        None => ui::meta(t!("Nothing running"), cx),
                    }),
                );
                if let Some(url) = found.url.clone() {
                    actions = actions.child(
                        Button::new("review-preview", t!("Open preview"), Variant::Outline)
                            .icon(Icon::Globe)
                            .on_click(cx.listener(move |review, _, window, cx| {
                                let (agent, url) = (review.agent.clone(), url.clone());
                                review.then(window, cx, move |bench, _, cx| {
                                    bench.open_on_desk(&agent, &url, cx);
                                });
                            })),
                    );
                }
                if let Some(failures) = found.failures.clone() {
                    actions = actions.child(
                        Button::new(
                            "review-failures",
                            t!("Send failures to agent"),
                            Variant::Primary,
                        )
                        .on_click(cx.listener(
                            move |review, _, window, cx| {
                                let (agent, failures) = (review.agent.clone(), failures.clone());
                                review.then(window, cx, move |bench, window, cx| {
                                    bench.deliver(&agent, failures, window, cx);
                                });
                            },
                        )),
                    );
                }
            }
        }
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|review, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    review.leave(window, cx);
                }
            }))
            .w(px(360.))
            .p(SPACE_3)
            .flex()
            .flex_col()
            .gap(SPACE_2H)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        ui::text(title, TEXT_SM, FontWeight::SEMIBOLD)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(t.text_primary),
                    )
                    .when_some(agent, |head, agent| {
                        head.child(ui::meta(
                            crate::agents::palette::status_word(agent.status),
                            cx,
                        ))
                    }),
            )
            .child(body)
            .child(actions)
    }
}

pub fn open(bench: &mut Workbench, agent: &str, cx: &mut Context<Workbench>) {
    let weak = cx.entity().downgrade();
    let closing = weak.clone();
    let close: Dismiss = Rc::new(move |_, cx| {
        closing.update(cx, |bench, cx| bench.close_review(cx)).ok();
    });
    let target = bench.target();
    let agent = agent.to_owned();
    let review = cx.new(|cx| AgentReview::new(weak, &target, agent, close, cx));
    bench.show_review(review, cx);
}
