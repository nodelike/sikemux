//! The card that opens over the top bar's CI chip: every workflow's run for the branch's newest
//! commit, the most pressing first, with how far they have got.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, deferred, div, point, px, Anchor, AnchoredPositionMode, AnyElement, App, Context,
    ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Task, WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::{Outcome, Run};

use crate::avatars::Avatars;
use crate::codehost::Area;
use crate::theme::{
    Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, TEXT_2XS, TEXT_SM,
};
use crate::ui::clock::{self, Slot};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

const CLOSE_AFTER: Duration = Duration::from_millis(140);
const WIDTH: f32 = 380.;

pub struct CiCard {
    bench: WeakEntity<Workbench>,
    open: bool,
    closing: Option<Task<()>>,
    /// How long each workflow's last passing run took, in seconds, by workflow.
    usual: HashMap<String, i64>,
    /// The names of each failed run's failed jobs, by run.
    failed_jobs: HashMap<String, String>,
    asked: HashSet<String>,
    rerunning: bool,
    pictures: Avatars,
}

impl CiCard {
    pub fn new(bench: WeakEntity<Workbench>) -> Self {
        Self {
            bench,
            open: false,
            closing: None,
            usual: HashMap::new(),
            failed_jobs: HashMap::new(),
            asked: HashSet::new(),
            rerunning: false,
            pictures: Avatars::default(),
        }
    }

    /// The pointer came onto the chip or the card, or left it: the card opens at once and
    /// closes a moment after the pointer is off both.
    pub fn hover(&mut self, inside: bool, cx: &mut Context<Self>) {
        if !inside {
            self.closing = Some(cx.spawn(async move |card, cx| {
                cx.background_executor().timer(CLOSE_AFTER).await;
                card.update(cx, |card, cx| card.close(cx)).ok();
            }));
            return;
        }
        self.closing = None;
        if !self.open {
            self.open = true;
            self.read_more(cx);
            cx.notify();
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = None;
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    /// Asks once for what the runs alone do not say: which jobs of a failed run failed, and
    /// how long a running workflow usually takes.
    fn read_more(&mut self, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let runs = bench.read(cx).head_runs();
        for run in runs {
            if run.outcome == Outcome::Failure && self.asked.insert(run.id.clone()) {
                let asked = plugin(bench.read(cx), "run", json!({ "runId": run.id }));
                let id = run.id.clone();
                cx.spawn(async move |card, cx| {
                    let Some(detail) = asked.await else {
                        return;
                    };
                    let failed: Vec<&str> = detail["jobs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|job| job["outcome"] == "failure")
                        .filter_map(|job| job["name"].as_str())
                        .collect();
                    let named = match failed.as_slice() {
                        [] => return,
                        [one] => (*one).to_owned(),
                        [first, rest @ ..] => format!("{first} +{}", rest.len()),
                    };
                    card.update(cx, |card, cx| {
                        card.failed_jobs.insert(id, named);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            let going = matches!(run.outcome, Outcome::Running | Outcome::Queued);
            if going && self.asked.insert(format!("usual {}", run.workflow_id)) {
                let params =
                    json!({ "workflowId": run.workflow_id, "status": "success", "perPage": 1 });
                let asked = plugin(bench.read(cx), "runs", params);
                let workflow = run.workflow_id.clone();
                cx.spawn(async move |card, cx| {
                    let Some(last) = asked.await else {
                        return;
                    };
                    let Some(took) = serde_json::from_value::<Run>(last["runs"][0].clone())
                        .ok()
                        .and_then(|run| took(&run))
                    else {
                        return;
                    };
                    card.update(cx, |card, cx| {
                        card.usual.insert(workflow, took);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn rerun_failed(&mut self, runs: Vec<String>, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        self.rerunning = true;
        for id in runs {
            let asked = plugin(
                bench.read(cx),
                "rerun",
                json!({ "runId": id, "failedOnly": true }),
            );
            cx.background_executor()
                .spawn(async move {
                    asked.await;
                })
                .detach();
        }
        cx.notify();
    }

    fn fetch_pictures(&self, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let bench = bench.read(cx);
        let asks = bench.asks.clone();
        let plugin = bench.ci.watched.as_ref().map(|ci| ci.plugin.clone());
        self.pictures.fetch(
            |url| {
                let asked = plugin.as_ref().map(|plugin| {
                    let call =
                        json!({ "plugin": plugin, "method": "image", "params": { "url": url } });
                    crate::core::request(&asks, "plugin", call)
                });
                async move { asked?.await.ok()?["value"].as_str().map(str::to_owned) }
            },
            |card| &card.pictures,
            cx,
        );
    }

    fn open_run(&mut self, id: String, cx: &mut Context<Self>) {
        self.close(cx);
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let host = bench.update(cx, |bench, cx| {
            bench.open_area(Area::Actions, cx);
            bench.git.host.clone()
        });
        if let Some(host) = host {
            host.update(cx, |host, cx| host.open_run(id, None, cx));
        }
    }

    fn open_actions(&mut self, cx: &mut Context<Self>) {
        self.close(cx);
        if let Some(bench) = self.bench.upgrade() {
            bench.update(cx, |bench, cx| bench.open_area(Area::Actions, cx));
        }
    }
}

/// Asks the code host's plugin, in the repository the chip shows, and answers with what it
/// said, or nothing when it could not.
fn plugin(
    bench: &Workbench,
    method: &str,
    mut params: Value,
) -> impl Future<Output = Option<Value>> {
    let asked = bench.ci.watched.as_ref().map(|ci| {
        params["owner"] = json!(ci.owner);
        params["name"] = json!(ci.name);
        params["account"] = json!(ci.account);
        let call = json!({ "plugin": ci.plugin, "method": method, "params": params });
        crate::core::request(&bench.asks, "plugin", call)
    });
    async move { asked?.await.ok()?.get("value").cloned() }
}

/// The runs in the order the card lists them: the most pressing first, then by name.
pub fn ordered(mut runs: Vec<Run>) -> Vec<Run> {
    runs.sort_by(|a, b| {
        urgency(a.outcome)
            .cmp(&urgency(b.outcome))
            .then_with(|| a.name.cmp(&b.name))
    });
    runs
}

pub fn urgency(outcome: Outcome) -> u8 {
    match outcome {
        Outcome::Failure => 0,
        Outcome::Running => 1,
        Outcome::Queued => 2,
        Outcome::Blocked => 3,
        Outcome::Cancelled => 4,
        Outcome::Success => 5,
        Outcome::Skipped | Outcome::Unknown => 6,
    }
}

pub fn outcome_key(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "running",
        Outcome::Queued => "queued",
        Outcome::Success => "success",
        Outcome::Failure => "failure",
        Outcome::Cancelled => "cancelled",
        Outcome::Skipped => "skipped",
        Outcome::Blocked => "blocked",
        Outcome::Unknown => "unknown",
    }
}

fn outcome_word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "running",
        Outcome::Queued => "queued",
        Outcome::Success => "passed",
        Outcome::Failure => "failed",
        Outcome::Cancelled => "cancelled",
        Outcome::Skipped => "skipped",
        Outcome::Blocked => "waiting",
        Outcome::Unknown => "unknown",
    }
}

pub fn is_going(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Running | Outcome::Queued | Outcome::Blocked
    )
}

fn millis(at: &str) -> Option<i64> {
    Some(
        chrono::DateTime::parse_from_rfc3339(at)
            .ok()?
            .timestamp_millis(),
    )
}

/// How long a finished run took, in seconds.
fn took(run: &Run) -> Option<i64> {
    let started = millis(run.started_at.as_deref()?)?;
    Some((millis(&run.updated_at)? - started).max(0) / 1000)
}

/// How long a run has gone, or went, in seconds.
fn elapsed(run: &Run) -> Option<i64> {
    match is_going(run.outcome) {
        true => {
            let started = millis(run.started_at.as_deref()?)?;
            Some((chrono::Utc::now().timestamp_millis() - started).max(0) / 1000)
        }
        false => took(run),
    }
}

fn span(seconds: i64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

impl Render for CiCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let runs = match (self.open, self.bench.upgrade()) {
            (true, Some(bench)) => ordered(bench.read(cx).head_runs()),
            _ => Vec::new(),
        };
        let Some(head) = runs
            .iter()
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
            .cloned()
        else {
            return div().into_any_element();
        };
        let t = Tokens::get(cx).clone();
        let going = runs.iter().any(|run| is_going(run.outcome));
        if going {
            clock::second(window, cx);
        }
        let failed: Vec<String> = runs
            .iter()
            .filter(|run| run.outcome == Outcome::Failure)
            .map(|run| run.id.clone())
            .collect();
        if failed.is_empty() {
            self.rerunning = false;
        }
        let ago = millis(&head.created_at)
            .map(|at| match crate::ui::time::age(at.max(0) as u64) {
                now if now == "now" => now,
                age => format!("{age} ago"),
            })
            .unwrap_or_default();
        let actor = head.actor.clone().unwrap_or_default();
        let face = self
            .pictures
            .face(&actor, head.avatar_url.as_deref(), 14., cx);
        let tint = |outcome: Outcome| match outcome {
            Outcome::Success => t.live,
            Outcome::Failure => t.danger,
            Outcome::Running | Outcome::Queued | Outcome::Blocked => t.warn,
            _ => t.text_disabled,
        };
        let meter = div()
            .mt(SPACE_2H)
            .h(px(3.))
            .flex()
            .gap(SPACE_0)
            .children(runs.iter().map(|run| {
                let segment = div().flex_1().h_full();
                match run.outcome {
                    Outcome::Running => segment.child(Slot::new(
                        ElementId::Name(format!("ci-card-pulse-{}", run.id).into()),
                        Pulse(t.warn),
                    )),
                    outcome => segment.rounded(px(2.)).bg(tint(outcome)),
                }
            }));
        let summary = match going {
            true => {
                let done = runs.iter().filter(|run| !is_going(run.outcome)).count();
                let left = runs
                    .iter()
                    .filter(|run| is_going(run.outcome))
                    .filter_map(|run| Some(self.usual.get(&run.workflow_id)? - elapsed(run)?))
                    .max();
                div()
                    .flex()
                    .child(format!("{done} of {} done", runs.len()))
                    .child(div().flex_1())
                    .children(left.map(|left| {
                        ui::mono(
                            match left {
                                l if l <= 0 => "finishing".to_owned(),
                                l if l < 60 => "<1m left".to_owned(),
                                l => format!("~{}m left", (l + 59) / 60),
                            },
                            11.,
                            cx,
                        )
                    }))
            }
            false => {
                let mut counts: Vec<(Outcome, usize)> = Vec::new();
                for run in &runs {
                    match counts
                        .iter_mut()
                        .find(|(outcome, _)| *outcome == run.outcome)
                    {
                        Some((_, count)) => *count += 1,
                        None => counts.push((run.outcome, 1)),
                    }
                }
                div()
                    .flex()
                    .gap(SPACE_2H)
                    .children(counts.into_iter().map(|(outcome, count)| {
                        div()
                            .when(outcome == Outcome::Failure, |word| {
                                word.text_color(t.danger)
                            })
                            .child(format!("{count} {}", outcome_word(outcome)))
                    }))
            }
        };
        let header = div()
            .pt(SPACE_3)
            .px(SPACE_3H)
            .pb(SPACE_2H)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_SM))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_primary)
                    .line_clamp(2)
                    .child(head.title.clone()),
            )
            .child(
                div()
                    .mt(SPACE_1H)
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_tertiary)
                    .child(
                        ui::mono(head.short_sha.clone(), 11., cx)
                            .px(SPACE_1H)
                            .py(px(1.))
                            .rounded(crate::theme::RADIUS_1)
                            .bg(t.surface_hover)
                            .text_color(t.text_primary),
                    )
                    .when(!actor.is_empty(), |meta| {
                        meta.child(div().ml(SPACE_0).child(face))
                            .child(actor.clone())
                            .child(div().text_color(t.text_disabled).child("·"))
                    })
                    .child(ago),
            )
            .child(meter)
            .child(
                summary
                    .mt(SPACE_1H)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_tertiary),
            );
        let rows = div()
            .p(SPACE_1)
            .flex()
            .flex_col()
            .children(runs.iter().map(|run| {
                let extra = match run.outcome {
                    Outcome::Failure => self.failed_jobs.get(&run.id).cloned(),
                    _ => run
                        .branch
                        .clone()
                        .filter(|branch| Some(branch) != head.branch.as_ref()),
                };
                let time = elapsed(run).map(span).unwrap_or_default();
                let id = run.id.clone();
                div()
                    .id(SharedString::from(format!("ci-card-run-{}", run.id)))
                    .h(px(32.))
                    .px(SPACE_2H)
                    .flex()
                    .items_center()
                    .gap(SPACE_2H)
                    .rounded(crate::theme::RADIUS_2)
                    .cursor_pointer()
                    .hover(|row| row.bg(t.surface_hover))
                    .on_click(cx.listener(move |card, _, _, cx| {
                        cx.stop_propagation();
                        card.open_run(id.clone(), cx)
                    }))
                    .child(ui::run_mark(
                        format!("ci-card-mark-{}", run.id),
                        outcome_key(run.outcome),
                        12.,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(SPACE_1H)
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(t.text_primary)
                                    .child(run.name.clone()),
                            )
                            .children(extra.map(|extra| {
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::theme::ui_text(TEXT_2XS))
                                    .text_color(t.text_tertiary)
                                    .child(extra)
                            })),
                    )
                    .child(
                        ui::mono(format!("#{}", run.run_number), 11., cx)
                            .min_w(px(40.))
                            .flex()
                            .justify_end()
                            .text_color(t.text_disabled),
                    )
                    .child(
                        ui::mono(time, 11., cx)
                            .min_w(px(58.))
                            .flex()
                            .justify_end()
                            .text_color(match is_going(run.outcome) {
                                true => t.warn,
                                false => t.text_tertiary,
                            }),
                    )
            }));
        let footer = div()
            .py(SPACE_1H)
            .pl(SPACE_2)
            .pr(SPACE_1H)
            .flex()
            .items_center()
            .border_t_1()
            .border_color(t.border)
            .when(!failed.is_empty(), |footer| {
                let rerunning = self.rerunning;
                footer.child(
                    Button::new(
                        "ci-card-rerun",
                        match rerunning {
                            true => "Re-running…",
                            false => "Re-run failed jobs",
                        },
                        Variant::Outline,
                    )
                    .disabled(rerunning)
                    .on_click({
                        let card = cx.weak_entity();
                        move |_, _, cx| {
                            cx.stop_propagation();
                            let failed = failed.clone();
                            card.update(cx, |card, cx| card.rerun_failed(failed, cx))
                                .ok();
                        }
                    }),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("ci-card-actions")
                    .px(SPACE_2)
                    .py(px(3.))
                    .rounded(crate::theme::RADIUS_2)
                    .cursor_pointer()
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_tertiary)
                    .hover(|link| link.text_color(t.text_primary))
                    .on_click(cx.listener(|card, _, _, cx| {
                        cx.stop_propagation();
                        card.open_actions(cx)
                    }))
                    .child("Open in Actions ↗"),
            );
        let card = div()
            .id("ci-card")
            .occlude()
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .bg(t.surface_overlay)
            .border_1()
            .border_color(t.border)
            .rounded(crate::theme::RADIUS_3)
            .shadow(t.shadow_float())
            .overflow_hidden()
            .text_size(crate::theme::ui_text(TEXT_SM))
            .text_color(t.text_secondary)
            .on_hover(cx.listener(|card, inside: &bool, _, cx| card.hover(*inside, cx)))
            .child(header)
            .child(rows)
            .child(footer);
        cx.defer_in(window, |card, _, cx| card.fetch_pictures(cx));
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Local)
                .position(point(px(0.), px(6.)))
                .anchor(Anchor::TopRight)
                .snap_to_window()
                .child(card),
        )
        .with_priority(1)
        .into_any_element()
    }
}

/// The chip's words: the runs done of all while any goes, how many failed, or how many ran.
pub fn chip_count(runs: &[Run], cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let total = runs.len();
    let failed = runs
        .iter()
        .filter(|run| run.outcome == Outcome::Failure)
        .count();
    if failed > 0 {
        return ui::mono(format!("{failed} failed"), 11., cx)
            .text_color(t.danger)
            .into_any_element();
    }
    if runs.iter().any(|run| is_going(run.outcome)) {
        let done = runs.iter().filter(|run| !is_going(run.outcome)).count();
        return div()
            .flex()
            .child(ui::mono(done.to_string(), 11., cx).text_color(t.text_primary))
            .child(ui::mono(format!("/{total}"), 11., cx).text_color(t.text_secondary))
            .into_any_element();
    }
    ui::mono(total.to_string(), 11., cx)
        .text_color(t.text_secondary)
        .into_any_element()
}

/// A running run's share of the card's meter, brightening and dimming on the shared clock.
struct Pulse(Hsla);

impl Render for Pulse {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lit = match !cx.reduce_motion() && clock::tick(window, cx) {
            true => {
                0.35 + 0.65 * (0.5 + 0.5 * (clock::seconds() / 1.4 * std::f32::consts::TAU).cos())
            }
            false => 1.,
        };
        div().size_full().rounded(px(2.)).bg(self.0).opacity(lit)
    }
}
