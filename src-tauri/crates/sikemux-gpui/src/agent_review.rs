//! What an agent did in its worktree, opened from its card in the rail: its branch, what it
//! said it did, the files it changed, its tests and the server it runs, as the core reads them.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, App, AppContext, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window,
};
use serde::Deserialize;
use serde_json::json;

use crate::core::Target;
use crate::editor::OpenAt;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

const SHOWN_FILES: usize = 6;

/// Closes the review.
type Dismiss = Rc<dyn Fn(&mut Window, &mut App)>;

/// What the core's `agentReview` answers.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    branch: String,
    path: PathBuf,
    summary: Option<String>,
    changed: Vec<String>,
    uncommitted: usize,
    tested: bool,
    passed: u64,
    failed: u64,
    url: Option<String>,
    failures: Option<String>,
}

pub struct AgentReview {
    bench: WeakEntity<Workbench>,
    agent: String,
    found: Option<Result<Found, SharedString>>,
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
        let read = core.request("agentReview", json!({ "agentId": agent }));
        cx.spawn(async move |review, cx| {
            let found = read.await.map_err(SharedString::from).and_then(|value| {
                serde_json::from_value(value).map_err(|error| error.to_string().into())
            });
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
                .gap(px(8.))
                .text_size(crate::theme::ui_text(12.))
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
            .map(|agent| agent.name.clone().unwrap_or_else(|| agent.title.clone()))
            .unwrap_or_else(|| "Agent".into());
        let mut body = div().flex().flex_col().gap(px(8.));
        let mut actions = div().flex().flex_wrap().gap(px(6.)).child(
            Button::new("review-show", "Show agent", Variant::Outline).on_click(cx.listener(
                |review, _, window, cx| {
                    let agent = review.agent.clone();
                    review.then(window, cx, move |bench, _, _| bench.open_agent(&agent));
                },
            )),
        );
        match &self.found {
            None => body = body.child(ui::meta("Reading the worktree…", cx)),
            Some(Err(error)) => {
                body = body.child(ui::meta(
                    format!("Could not read the worktree: {error}"),
                    cx,
                ))
            }
            Some(Ok(found)) => {
                body = body
                    .child(ui::mono(found.branch.clone(), 11.5, cx).text_color(t.text_secondary))
                    .children(found.summary.clone().map(|summary| {
                        div()
                            .text_size(crate::theme::ui_text(12.))
                            .text_color(t.text_primary)
                            .child(summary)
                    }));
                let changes = if found.changed.is_empty() {
                    "No changes yet".to_owned()
                } else {
                    let count = found.changed.len();
                    let mut words = format!("{count} file{}", if count == 1 { "" } else { "s" });
                    if found.uncommitted > 0 {
                        words.push_str(&format!(" · {} not committed", found.uncommitted));
                    }
                    words
                };
                body = body.child(row("Changes").child(changes));
                body = body.children(found.changed.iter().take(SHOWN_FILES).enumerate().map(
                    |(index, file)| {
                        let path = found.path.join(file);
                        div()
                            .id(("review-file", index))
                            .pl(px(72.))
                            .truncate()
                            .cursor_pointer()
                            .font_family(theme::MONO)
                            .text_size(crate::theme::ui_text(11.))
                            .text_color(t.accent)
                            .hover(|style| style.underline())
                            .on_click(cx.listener(move |review, _, window, cx| {
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
                    body = body.child(ui::meta(format!("and {more} more"), cx).pl(px(72.)));
                }
                let tests = if !found.tested {
                    ui::meta("No results yet", cx)
                } else if found.failed > 0 {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_color(t.danger)
                        .child(ui::icon(Icon::Close, 11.).text_color(t.danger))
                        .child(format!("{} failed · {} passed", found.failed, found.passed))
                } else {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_color(t.live)
                        .child(ui::icon(Icon::Check, 11.).text_color(t.live))
                        .child(format!("{} passed", found.passed))
                };
                body = body.child(row("Tests").child(tests));
                body = body.child(
                    row("Running").child(match &found.url {
                        Some(url) => {
                            ui::mono(url.trim_start_matches("http://").to_owned(), 11.5, cx)
                                .text_color(t.text_primary)
                        }
                        None => ui::meta("Nothing running", cx),
                    }),
                );
                if let Some(url) = found.url.clone() {
                    actions = actions.child(
                        Button::new("review-preview", "Open preview", Variant::Outline)
                            .icon(Icon::Globe)
                            .on_click(cx.listener(move |review, _, window, cx| {
                                let (agent, url) = (review.agent.clone(), url.clone());
                                review.then(window, cx, move |bench, _, _| {
                                    bench.open_on_desk(&agent, &url);
                                });
                            })),
                    );
                }
                if let Some(failures) = found.failures.clone() {
                    actions = actions.child(
                        Button::new(
                            "review-failures",
                            "Send failures to agent",
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
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        ui::text(title, 13., FontWeight::SEMIBOLD)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(t.text_primary),
                    )
                    .when_some(agent, |head, agent| {
                        head.child(ui::meta(
                            crate::agent_palette::status_word(agent.status),
                            cx,
                        ))
                    }),
            )
            .child(body)
            .child(actions)
    }
}

/// Opens the review of `agent`'s worktree over the window.
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
