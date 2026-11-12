//! How an agent's turn stands, in the words and colours every place says it: the line at the
//! end of a chat, and a terminal agent's pane header. The core works the state out; this only
//! words it.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Context, Div, FontWeight, Hsla, IntoElement, ParentElement, Render,
    SharedString, Styled, Window,
};
use sikemux_view::chat::FailureKind;
use sikemux_view::turn::{
    ChecksStand, TurnPhase, TurnState, TurnStep, TurnWait, TurnWaitKind, WorkAt,
};

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1H, SPACE_2, TEXT_SM, TEXT_XS_H};
use crate::ui;

/// The state's word, in the colour of its mark, then what it is on and how long, said between dots.
pub struct Said {
    pub word: SharedString,
    pub phase: TurnPhase,
    pub parts: Vec<SharedString>,
}

/// What `turn` says, for an agent of `maker` (the agent's own name, such as Claude). Working
/// with nothing said about what it is on says nothing, so the caller draws the live activity.
pub fn said(turn: &TurnState, maker: &str, now: u64) -> Option<Said> {
    let minutes = |from: u64| (now.saturating_sub(from) / 60_000).max(1);
    let (word, parts): (String, Vec<String>) = match turn.state {
        TurnPhase::Working => {
            let on = turn.on.clone()?;
            let running = ui::time::elapsed(now.saturating_sub(turn.since) / 1000);
            ("Working".into(), vec![lowered(&on), running])
        }
        TurnPhase::Waiting => {
            let wait = turn.wait.as_ref()?;
            (waiting_on(wait), waited(wait, now).into_iter().collect())
        }
        TurnPhase::NeedsYou => {
            let call = turn.call.as_ref()?;
            let more = match call.count {
                0 | 1 => String::new(),
                count => format!(" and {} more", count - 1),
            };
            (
                "Needs you".into(),
                vec![
                    format!("{}{more}", call.question),
                    format!("{} min", minutes(call.waiting_since)),
                ],
            )
        }
        TurnPhase::Done => {
            let Some(done) = turn.done.as_ref() else {
                return Some(Said {
                    word: "Done".into(),
                    phase: turn.state,
                    parts: Vec::new(),
                });
            };
            let mut parts = vec![changed(done.changes).to_owned()];
            if done.changes != WorkAt::None {
                match (done.checks, done.tests) {
                    (ChecksStand::Pass, Some(tests)) => parts.push(format!("{tests} tests pass")),
                    (ChecksStand::Pass, None) => parts.push("checks pass".into()),
                    (ChecksStand::Failed, _) => parts.push("checks fail".into()),
                    _ => {}
                }
            }
            ("Done".into(), parts)
        }
        TurnPhase::Stopped => {
            let left = (turn.stopped.as_ref()).and_then(|stopped| stopped.left.as_deref());
            let mut parts: Vec<String> = left.map(lowered).into_iter().collect();
            parts.push("nothing set to wake it".into());
            ("Stopped".into(), parts)
        }
        TurnPhase::Failed => {
            let failed = turn.failed.as_ref()?;
            let why = match failed.why {
                FailureKind::Limit => format!("{maker}'s limit"),
                FailureKind::SignIn => format!("signed out of {maker}"),
                FailureKind::Network => format!("lost the connection to {maker}"),
                _ => failed
                    .said
                    .clone()
                    .unwrap_or_else(|| "the turn did not finish".into()),
            };
            let mut parts = vec![why];
            if let Some(at) = failed.resets_at.filter(|at| *at > now) {
                parts.push(format!("resets {}", moment(at)));
            }
            ("Failed".into(), parts)
        }
        TurnPhase::Unknown => return None,
    };
    Some(Said {
        word: word.into(),
        phase: turn.state,
        parts: parts.into_iter().map(SharedString::from).collect(),
    })
}

/// What a rail or notch row's mark shows for a turn: one state, the same everywhere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    /// The agent's own working mark.
    Working,
    /// A ring that fills as a wait on CI or the checks runs, `through` of the way, once Sikemux
    /// knows how long that takes.
    Waiting {
        through: f32,
    },
    /// The timer of any other wait.
    Timer,
    NeedsYou,
    Done,
    Failed,
    /// A hollow ring: the turn stopped short.
    Stopped,
}

/// The mark a row shows for `turn`; none before an agent's first turn.
pub fn mark(turn: &TurnState, now: u64) -> Option<Mark> {
    Some(match turn.state {
        TurnPhase::Working => Mark::Working,
        TurnPhase::Waiting => match turn.wait.as_ref().and_then(|wait| filling(wait, now)) {
            Some(through) => Mark::Waiting { through },
            None => Mark::Timer,
        },
        TurnPhase::NeedsYou => Mark::NeedsYou,
        TurnPhase::Done => Mark::Done,
        TurnPhase::Failed => Mark::Failed,
        TurnPhase::Stopped => Mark::Stopped,
        TurnPhase::Unknown => return None,
    })
}

/// How far a wait on CI or the checks has run, once Sikemux knows how long it takes; none for
/// any other wait, which shows the timer.
fn filling(wait: &TurnWait, now: u64) -> Option<f32> {
    let expected = wait.expected_ms.filter(|ms| *ms > 0)?;
    matches!(wait.kind, TurnWaitKind::Ci | TurnWaitKind::Check)
        .then(|| (now.saturating_sub(wait.started_at) as f32 / expected as f32).min(1.))
}

/// The time a row shows beside its mark: the minutes a filling wait has left ("4m"), when a
/// timer goes off, or how long a call has waited on the person ("12m").
pub fn short(turn: &TurnState, now: u64) -> Option<String> {
    match turn.state {
        TurnPhase::Waiting => {
            let wait = turn.wait.as_ref()?;
            match (filling(wait, now), wait.expected_ms, wait.until) {
                (Some(_), Some(expected), _) => {
                    let left = (wait.started_at + expected).saturating_sub(now);
                    Some(format!("{}m", left.div_ceil(60_000).max(1)))
                }
                (_, _, Some(until)) => Some(ui::time::until(until)),
                _ => None,
            }
        }
        TurnPhase::NeedsYou => {
            let call = turn.call.as_ref()?;
            Some(format!(
                "{}m",
                (now.saturating_sub(call.waiting_since) / 60_000).max(1)
            ))
        }
        _ => None,
    }
}

/// A row's state in a few words, as its hover card says it: "needs you 12m", "CI 4m".
pub fn brief(turn: &TurnState, now: u64) -> Option<String> {
    let time = short(turn, now);
    let with = |word: &str| match &time {
        Some(time) => format!("{word} {time}"),
        None => word.to_owned(),
    };
    Some(match turn.state {
        TurnPhase::Working => "working".to_owned(),
        TurnPhase::Waiting => match turn.wait.as_ref().map(|wait| wait.kind) {
            Some(TurnWaitKind::Ci) => with("CI"),
            Some(TurnWaitKind::Deploy) => with("deploy"),
            Some(TurnWaitKind::Check) => with("checks"),
            Some(TurnWaitKind::Timer) => with("timer"),
            _ => with("waiting"),
        },
        TurnPhase::NeedsYou => with("needs you"),
        TurnPhase::Done => "done".to_owned(),
        TurnPhase::Failed => "failed".to_owned(),
        TurnPhase::Stopped => "stopped".to_owned(),
        TurnPhase::Unknown => return None,
    })
}

/// The agent's own words, as they read after a dot: first letter small unless it starts a name.
fn lowered(words: &str) -> String {
    let mut letters = words.trim().trim_end_matches('.').chars();
    match (letters.next(), letters.clone().next()) {
        (Some(first), Some(second)) if !second.is_uppercase() => {
            first.to_lowercase().chain(letters).collect()
        }
        _ => words.trim().trim_end_matches('.').to_owned(),
    }
}

/// A moment as the chat's own times say it.
pub fn moment(at: u64) -> String {
    crate::chat::clock(at).unwrap_or_default()
}

fn waiting_on(wait: &TurnWait) -> String {
    match wait.kind {
        TurnWaitKind::Ci => "Waiting on CI".into(),
        TurnWaitKind::Deploy => "Waiting on the deploy".into(),
        TurnWaitKind::Check => "Waiting on the checks".into(),
        TurnWaitKind::Agent if wait.what.starts_with('@') => format!("Waiting on {}", wait.what),
        TurnWaitKind::Agent => format!("Waiting on @{}", wait.what),
        TurnWaitKind::Timer => match wait.until {
            Some(until) => format!("Waiting until {}", moment(until)),
            None => "Waiting".into(),
        },
        TurnWaitKind::Process | TurnWaitKind::Unknown => format!("Waiting on {}", wait.what),
    }
}

/// How long a wait has gone, against how long it usually takes once that is known.
fn waited(wait: &TurnWait, now: u64) -> Option<String> {
    if wait.kind == TurnWaitKind::Timer {
        return None;
    }
    let gone = now.saturating_sub(wait.started_at) / 60_000;
    Some(match wait.expected_ms {
        Some(expected) => format!("{gone} of ~{} min", expected.div_ceil(60_000)),
        None => format!("{gone} min"),
    })
}

fn changed(changes: WorkAt) -> &'static str {
    match changes {
        WorkAt::None | WorkAt::Unknown => "no changes",
        WorkAt::Uncommitted => "uncommitted",
        WorkAt::Committed => "committed",
        WorkAt::Landed => "landed",
        WorkAt::Staged => "on staging",
        WorkAt::Shipped => "shipped",
    }
}

/// The words on a step's button.
pub fn step_label(step: TurnStep) -> Option<&'static str> {
    Some(match step {
        TurnStep::Commit => "Commit",
        TurnStep::Push => "Push",
        TurnStep::DeployStaging => "Deploy to staging",
        TurnStep::ShipProduction => "Ship to production…",
        TurnStep::RollBack => "Roll back",
        TurnStep::TryIt => "Try it",
        TurnStep::CarryOn => "Carry on",
        TurnStep::Undo => "Undo",
        TurnStep::Unknown => return None,
    })
}

/// The colour of a state's mark, which its word takes; working has none of its own.
pub fn tint(phase: TurnPhase, cx: &App) -> Option<Hsla> {
    let t = Tokens::get(cx);
    match phase {
        TurnPhase::Done => Some(t.accent),
        TurnPhase::Waiting => Some(t.cmd),
        TurnPhase::NeedsYou => Some(t.warn),
        TurnPhase::Failed => Some(t.danger),
        TurnPhase::Stopped => Some(t.text_secondary),
        TurnPhase::Working | TurnPhase::Unknown => None,
    }
}

/// The word and the parts after it, at `size`, the first part giving way when the room runs out.
/// `key` names the working word's shimmer.
pub fn words(said: &Said, key: SharedString, size: f32, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let dot = || div().flex_none().text_color(t.text_disabled).child("·");
    let word = div()
        .flex_none()
        .font_weight(FontWeight::MEDIUM)
        .map(|word| match tint(said.phase, cx) {
            Some(tint) => word.text_color(tint).child(said.word.clone()),
            None => word.child(ui::shimmer::shimmer(key, said.word.clone())),
        });
    let last = said.parts.len().saturating_sub(1);
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(crate::theme::ui_text(size))
        .child(word)
        .children(said.parts.iter().enumerate().flat_map(|(index, part)| {
            let shown = div()
                .text_color(if index == 0 {
                    t.text_secondary
                } else {
                    t.text_tertiary
                })
                .text_size(crate::theme::ui_text(size - 0.5))
                .map(|shown| match index < last {
                    true => shown.min_w_0().truncate(),
                    false => shown.flex_none(),
                })
                .child(part.clone());
            [dot().into_any_element(), shown.into_any_element()]
        }))
}

/// The line at the end of a chat: the agent's `mark`, what `said` says, and `buttons` on the
/// same row.
pub fn line(
    mark: AnyElement,
    said: &Said,
    key: SharedString,
    buttons: Vec<AnyElement>,
    cx: &App,
) -> Div {
    div()
        .min_h(px(32.))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(mark)
        .child(words(said, key, TEXT_SM, cx).flex_1())
        .when(!buttons.is_empty(), |line| {
            line.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .children(buttons),
            )
        })
}

/// The agent's own name for `provider`, such as Claude, as a failure names it.
pub fn maker(provider: &str) -> &'static str {
    sikemux_core::agent_catalog::agent(provider).map_or("the agent", |agent| agent.label)
}

/// Whether the words count time by the second while the turn stands so.
pub fn ticks(phase: TurnPhase) -> bool {
    matches!(
        phase,
        TurnPhase::Working | TurnPhase::Waiting | TurnPhase::NeedsYou
    )
}

/// What a terminal agent's header says after its name, drawn again each second on its own
/// while its time runs.
pub fn header(turn: &TurnState, provider: &str, key: SharedString) -> AnyElement {
    ui::clock::Slot::new(
        key.clone(),
        HeaderFact {
            turn: turn.clone(),
            maker: maker(provider),
            key,
        },
    )
    .into_any_element()
}

struct HeaderFact {
    turn: TurnState,
    maker: &'static str,
    key: SharedString,
}

impl Render for HeaderFact {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if ticks(self.turn.state) {
            ui::clock::second(window, cx);
        }
        match said(&self.turn, self.maker, ui::time::now_ms()) {
            Some(said) => header_fact(&said, self.key.clone(), cx).into_any_element(),
            None => div().into_any_element(),
        }
    }
}

/// A wait's timer mark, then the line's words.
fn header_fact(said: &Said, key: SharedString, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(5.))
        .when(said.phase == TurnPhase::Waiting, |fact| {
            fact.child(ui::icon(Icon::Hourglass, 12.).text_color(t.cmd))
        })
        .child(words(said, key, TEXT_XS_H, cx))
}
