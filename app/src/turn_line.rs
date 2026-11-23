//! How an agent's turn stands, in the words and colours every place says it: the line at the
//! end of a chat, and a terminal agent's pane header. The core works the state out; this only
//! words it.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, Context, Div, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window,
};
use sikemux_view::chat::FailureKind;
use sikemux_view::turn::{
    ChecksStand, TurnPhase, TurnState, TurnStep, TurnWait, TurnWaitKind, WorkAt,
};

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1H, SPACE_2, TEXT_SM, TEXT_XS_H};
use crate::ui;
use crate::workbench::Workbench;

/// The state's word, in the colour of its mark, then what it is on and how long, said between dots.
pub struct Said {
    pub word: SharedString,
    pub phase: TurnPhase,
    pub parts: Vec<SharedString>,
    /// The agents the word names, by id, which a line that can open them draws as their chips.
    pub agents: Vec<String>,
}

/// What `turn` says, for an agent of `maker` (the agent's own name, such as Claude). Working
/// with nothing said about what it is on says nothing, so the caller draws the live activity.
pub fn said(turn: &TurnState, maker: &str, now: u64) -> Option<Said> {
    let minutes = |from: u64| (now.saturating_sub(from) / 60_000).max(1);
    let (word, parts): (String, Vec<String>) = match turn.state {
        TurnPhase::Working => {
            let on = turn.on.clone()?;
            let running = ui::time::elapsed(now.saturating_sub(turn.since) / 1000);
            (t!("Working").into(), vec![lowered(&on), running])
        }
        TurnPhase::Waiting => {
            let wait = turn.wait.as_ref()?;
            let agents = match (wait.kind, wait.agents.is_empty()) {
                (TurnWaitKind::Agent, true) => wait.agent.clone().into_iter().collect(),
                (TurnWaitKind::Agent, false) => wait.agents.clone(),
                _ => Vec::new(),
            };
            return Some(Said {
                word: waiting_on(wait).into(),
                phase: turn.state,
                parts: (waited(wait, now).into_iter()).map(Into::into).collect(),
                agents,
            });
        }
        TurnPhase::NeedsYou => {
            let call = turn.call.as_ref()?;
            let more = match call.count {
                0 | 1 => String::new(),
                count => t!(" and {more} more", more = count - 1),
            };
            (
                t!("Needs you").into(),
                vec![
                    format!("{}{more}", call.question),
                    t!("{minutes} min", minutes = minutes(call.waiting_since)),
                ],
            )
        }
        TurnPhase::Done => {
            let Some(done) = turn.done.as_ref() else {
                return Some(Said {
                    word: t!("Done").into(),
                    phase: turn.state,
                    parts: Vec::new(),
                    agents: Vec::new(),
                });
            };
            let mut parts = vec![changed(done.changes).to_owned()];
            if done.changes != WorkAt::None {
                match (done.checks, done.tests) {
                    (ChecksStand::Pass, Some(tests)) => {
                        parts.push(t!("{tests} tests pass", tests = tests))
                    }
                    (ChecksStand::Pass, None) => parts.push(t!("checks pass").into()),
                    (ChecksStand::Failed, _) => parts.push(t!("checks fail").into()),
                    _ => {}
                }
            }
            (t!("Done").into(), parts)
        }
        TurnPhase::Stopped => {
            let left = (turn.stopped.as_ref()).and_then(|stopped| stopped.left.as_deref());
            let mut parts: Vec<String> = left.map(lowered).into_iter().collect();
            parts.push(t!("nothing set to wake it").into());
            (t!("Stopped").into(), parts)
        }
        TurnPhase::Failed => {
            let failed = turn.failed.as_ref()?;
            let why = match failed.why {
                FailureKind::Limit => t!("{maker}'s limit", maker = maker),
                FailureKind::SignIn => t!("signed out of {maker}", maker = maker),
                FailureKind::Payment => t!("{maker} plan is unpaid", maker = maker),
                FailureKind::Network => t!("lost the connection to {maker}", maker = maker),
                _ => failed
                    .said
                    .clone()
                    .unwrap_or_else(|| t!("the turn did not finish").into()),
            };
            let mut parts = vec![why];
            if let Some(at) = failed.resets_at.filter(|at| *at > now) {
                parts.push(t!("resets {when}", when = moment(at)));
            }
            (t!("Failed").into(), parts)
        }
        TurnPhase::Unknown => return None,
    };
    Some(Said {
        word: word.into(),
        phase: turn.state,
        parts: parts.into_iter().map(SharedString::from).collect(),
        agents: Vec::new(),
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
/// timer goes off, or how long a call has waited on the person ("12m", "3h", "5d").
pub fn short(turn: &TurnState, now: u64) -> Option<String> {
    match turn.state {
        TurnPhase::Waiting => {
            let wait = turn.wait.as_ref()?;
            match (filling(wait, now), wait.expected_ms, wait.until) {
                (Some(_), Some(expected), _) => {
                    let left = (wait.started_at + expected).saturating_sub(now);
                    Some(t!("{minutes}m", minutes = left.div_ceil(60_000).max(1)))
                }
                (_, _, Some(until)) => Some(ui::time::until(until)),
                _ => None,
            }
        }
        TurnPhase::NeedsYou => {
            let call = turn.call.as_ref()?;
            Some(ui::time::age(call.waiting_since))
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
        TurnPhase::Working => t!("working").to_owned(),
        TurnPhase::Waiting => match turn.wait.as_ref().map(|wait| wait.kind) {
            Some(TurnWaitKind::Ci) => with(t!("CI")),
            Some(TurnWaitKind::Deploy) => with(t!("deploy")),
            Some(TurnWaitKind::Check) => with(t!("checks")),
            Some(TurnWaitKind::Timer) => with(t!("timer")),
            Some(TurnWaitKind::Watch) => with(t!("watching")),
            Some(TurnWaitKind::Line | TurnWaitKind::Land) => {
                match turn.wait.as_ref().and_then(|wait| wait.place) {
                    Some(place) => in_line(place),
                    None => t!("in line").to_owned(),
                }
            }
            _ => with(t!("waiting")),
        },
        TurnPhase::NeedsYou => with(t!("needs you")),
        TurnPhase::Done => t!("done").to_owned(),
        TurnPhase::Failed => t!("failed").to_owned(),
        TurnPhase::Stopped => t!("stopped").to_owned(),
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

/// The agent a wait names first, with how many more it waits on: `@Otter +14`.
fn agents_waited_on(wait: &TurnWait) -> String {
    let first = match wait.what.starts_with('@') {
        true => wait.what.clone(),
        false => format!("@{}", wait.what),
    };
    match wait.agents.len() {
        0 | 1 => first,
        count => format!("{first} +{}", count - 1),
    }
}

fn waiting_on(wait: &TurnWait) -> String {
    match wait.kind {
        TurnWaitKind::Ci => t!("Waiting on CI").into(),
        TurnWaitKind::Deploy => t!("Waiting on the deploy").into(),
        TurnWaitKind::Check => t!("Waiting on the checks").into(),
        TurnWaitKind::Agent => t!("Waiting on {what}", what = agents_waited_on(wait)),
        TurnWaitKind::Timer => match wait.until {
            Some(until) => t!("Waiting until {when}", when = moment(until)),
            None => t!("Waiting").into(),
        },
        TurnWaitKind::Line => wait.what.clone(),
        TurnWaitKind::InUse => t!("Waiting for {what}", what = wait.what),
        TurnWaitKind::Land => t!("Waiting to land on {branch}", branch = wait.what),
        TurnWaitKind::Watch | TurnWaitKind::Process | TurnWaitKind::Unknown => {
            t!("Waiting on {what}", what = wait.what)
        }
    }
}

/// How long a wait has gone, against how long it usually takes once that is known; for a
/// command in line, its place and about how long it has left.
fn waited(wait: &TurnWait, now: u64) -> Vec<String> {
    let gone = now.saturating_sub(wait.started_at) / 60_000;
    match wait.kind {
        TurnWaitKind::Timer => Vec::new(),
        TurnWaitKind::Line | TurnWaitKind::Land => (wait.place.map(in_line))
            .into_iter()
            .chain(wait.expected_ms.map(about))
            .collect(),
        TurnWaitKind::InUse => (wait.by.as_ref())
            .map(|by| t!("in use by @{name}", name = by))
            .into_iter()
            .chain((gone > 0).then(|| t!("{minutes} min", minutes = gone)))
            .collect(),
        _ => vec![match wait.expected_ms {
            Some(expected) => t!(
                "{gone} of ~{expected} min",
                gone = gone,
                expected = expected.div_ceil(60_000)
            ),
            None => t!("{minutes} min", minutes = gone),
        }],
    }
}

/// A place in the host's line on its own: "next in line", "2nd in line".
pub fn in_line(place: u32) -> String {
    match place {
        0 | 1 => t!("next in line").to_owned(),
        place => t!("{place} in line", place = ordinal(place)),
    }
}

fn ordinal(place: u32) -> String {
    match (place % 100, place % 10) {
        (11..=13, _) => t!("{place}th", place = place),
        (_, 1) => t!("{place}st", place = place),
        (_, 2) => t!("{place}nd", place = place),
        (_, 3) => t!("{place}rd", place = place),
        _ => t!("{place}th", place = place),
    }
}

/// How long a wait has left, roughly: "about 4 min".
pub fn about(ms: u64) -> String {
    t!("about {minutes} min", minutes = ms.div_ceil(60_000).max(1))
}

fn changed(changes: WorkAt) -> &'static str {
    match changes {
        WorkAt::None | WorkAt::Unknown => t!("no changes"),
        WorkAt::Uncommitted => t!("uncommitted"),
        WorkAt::Committed => t!("committed"),
        WorkAt::Landed => t!("landed"),
        WorkAt::Staged => t!("on staging"),
        WorkAt::Shipped => t!("shipped"),
    }
}

/// The words on a step's button.
pub fn step_label(step: TurnStep) -> Option<&'static str> {
    Some(match step {
        TurnStep::Commit => t!("Commit"),
        TurnStep::Push => t!("Push"),
        TurnStep::DeployStaging => t!("Deploy to staging"),
        TurnStep::ShipProduction => t!("Ship to production…"),
        TurnStep::RollBack => t!("Roll back"),
        TurnStep::TryIt => t!("Try it"),
        TurnStep::CarryOn => t!("Carry on"),
        TurnStep::Undo => t!("Undo"),
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

/// The word, wrapping to at most `lines`, and the parts after it, at `size`, the first part giving
/// way when the room runs out, unless `wraps` lets every part wrap whole. `key` names the working
/// word's shimmer. Given the `bench`, an agent the word names is drawn as its chip, which opens it.
pub fn words(
    said: &Said,
    key: SharedString,
    size: f32,
    lines: usize,
    wraps: bool,
    bench: Option<&WeakEntity<Workbench>>,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx);
    let dot = || div().flex_none().text_color(t.text_disabled).child("·");
    let pills = bench.and_then(|bench| agent_pills(&said.agents, size, bench, cx));
    let word =
        div()
            .font_weight(FontWeight::MEDIUM)
            .map(|word| match (tint(said.phase, cx), pills) {
                (Some(tint), Some(pills)) => word
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_color(tint)
                    .child(t!("Waiting on"))
                    .children(pills),
                (tint, _) => word
                    .flex_shrink_0()
                    .max_w(relative(0.6))
                    .line_clamp(lines)
                    .text_ellipsis()
                    .map(|word| match tint {
                        Some(tint) => word.text_color(tint).child(said.word.clone()),
                        None => word.child(ui::shimmer::shimmer(key, said.word.clone())),
                    }),
            });
    let last = said.parts.len().saturating_sub(1);
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.))
        .when(wraps, |words| words.flex_wrap())
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
                .map(|shown| match (wraps, index < last) {
                    (true, _) => shown.min_w_0(),
                    (false, true) => shown.min_w_0().truncate(),
                    (false, false) => shown.flex_none(),
                })
                .child(part.clone());
            [dot().into_any_element(), shown.into_any_element()]
        }))
}

/// The most agents a line names by their chips; the rest are counted in one chip that lists them.
const SHOWN_AGENTS: usize = 2;

/// The chips of the agents a line waits on: the first two, then a count of the rest, which
/// lists them on hover. None when the bench knows none of them.
fn agent_pills(
    agents: &[String],
    size: f32,
    bench: &WeakEntity<Workbench>,
    cx: &App,
) -> Option<Vec<AnyElement>> {
    let rows: Vec<_> = {
        let bench = bench.upgrade()?;
        let bench = bench.read(cx);
        (agents.iter())
            .filter_map(|agent| bench.agent_by_id(agent))
            .collect()
    };
    if rows.is_empty() {
        return None;
    }
    let (shown, hidden) = rows.split_at(rows.len().min(SHOWN_AGENTS));
    let mut pills: Vec<AnyElement> = (shown.iter())
        .map(|row| crate::chat::agent_pill(row, size, bench, cx).into_any_element())
        .collect();
    let hidden = hidden.to_vec();
    if hidden.is_empty() {
        return Some(pills);
    }
    let t = Tokens::get(cx);
    let more = div()
        .id(SharedString::from(format!(
            "waiting-more:{}",
            agents.first().map(String::as_str).unwrap_or_default()
        )))
        .flex_none()
        .px(SPACE_1H)
        .rounded(crate::theme::radius_2())
        .bg(t.surface_raised)
        .text_color(t.text_secondary)
        .text_size(crate::theme::ui_text(size))
        .child(format!("+{}", hidden.len()))
        .tooltip(ui::card_tooltip(180., {
            let bench = bench.clone();
            move |cx| hidden_list(&hidden, &bench, cx)
        }));
    pills.push(more.into_any_element());
    Some(pills)
}

/// The agents a count of more stands for, as their chips, longest at work first.
fn hidden_list(
    rows: &[crate::agents::rows::AgentRow],
    bench: &WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(4.))
        .children((rows.iter()).map(|row| crate::chat::agent_pill(row, TEXT_SM, bench, cx)))
        .into_any_element()
}

/// The line at the end of a chat: the agent's `mark`, what `said` says, and `buttons` on the
/// same row. A failure's reason wraps whole. Given the `bench`, an agent it names opens from its
/// chip.
pub fn line(
    mark: AnyElement,
    said: &Said,
    key: SharedString,
    bench: Option<&WeakEntity<Workbench>>,
    buttons: Vec<AnyElement>,
    cx: &App,
) -> Div {
    let wraps = said.phase == TurnPhase::Failed;
    div()
        .min_h(px(32.))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(mark)
        .child(words(said, key, TEXT_SM, 2, wraps, bench, cx).flex_1())
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
    sikemux_agents::agent_catalog::agent(provider).map_or(t!("the agent"), |agent| agent.label)
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
        .child(words(said, key, TEXT_XS_H, 1, false, None, cx))
}
