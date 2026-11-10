//! Which of the person's accounts with one agent a chat goes to. A new chat goes to the account
//! whose week ends soonest, as what a week leaves unused is lost at its reset; a chat at its
//! limit goes where the most is left, so it can keep going. Only accounts with room in every
//! window that limits the chat's model take one. What is left counts in plans of the smallest
//! tier, so a week half left on a bigger plan holds more than one half left on a small one.

use super::usage::{Usage, UsageWindow};

/// An account a chat could go to, with its last reading of its plan, if any.
pub struct Candidate<'a> {
    pub id: &'a str,
    pub usage: Option<&'a Usage>,
    /// The share of this account's week a typical chat in the project takes, once known.
    pub typical: Option<f64>,
    /// How fast the chats running on it fill its windows shorter than a week, in percent a
    /// minute.
    pub burn: f64,
}

/// How many plans of the agent's smallest tier `plan` holds, as reported rather than as the
/// providers state it; one for a plan not known here.
pub fn weight(provider: &str, plan: Option<&str>) -> f64 {
    let plan = plan.unwrap_or_default().to_ascii_lowercase();
    match (provider, plan.as_str()) {
        ("claude", "max") => 5.0,
        ("claude", "ultra") => 10.0,
        ("codex", "pro") => 10.0,
        _ => 1.0,
    }
}

/// What the chat is placed for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    /// A new chat, which can go where the week would otherwise go to waste.
    Soonest,
    /// A chat at its limit, which must land where it can keep going.
    Headroom,
}

/// The account chosen, and why, as a person reads it.
#[derive(Debug, PartialEq)]
pub struct Placement {
    pub id: String,
    pub why: String,
}

const WEEK_MINUTES: u64 = 10_080;
/// How full a window shorter than a week may be for a new chat to start under it.
const SHORT_WINDOW_CAP: f64 = 80.0;
const WEEK_CAP: f64 = 95.0;
/// The share of a week a typical chat takes, until chats' own use says otherwise.
const ENOUGH_FOR_A_CHAT: f64 = 10.0;
/// Weeks that end this close together count as ending together.
const SAME_DEADLINE_MS: u64 = 60 * 60 * 1000;

struct Standing<'a> {
    id: &'a str,
    /// The share of the week left on the tightest window over the chat's model.
    left: f64,
    /// That share in plans of the smallest tier.
    headroom: f64,
    /// The share of the week a typical chat takes here.
    typical: f64,
    /// When that week resets, in Unix milliseconds.
    deadline: Option<u64>,
}

/// Whether `window` limits a chat on `model`: every window for the whole plan, and a model's
/// own week only for that model.
fn governs(window: &UsageWindow, model: Option<&str>) -> bool {
    let Some((scope, _)) = window.label.split_once(" · ") else {
        return true;
    };
    let (scope, model) = (
        scope.to_lowercase(),
        model.unwrap_or_default().to_lowercase(),
    );
    !model.is_empty() && (model.contains(&scope) || scope.contains(&model))
}

/// Where the account stands for a chat on `model`, or none when a window over that model is
/// too full to start one.
fn standing<'a>(
    candidate: &Candidate<'a>,
    model: Option<&str>,
    now_ms: u64,
) -> Option<Standing<'a>> {
    let usage = candidate.usage?;
    let governing: Vec<&UsageWindow> = (usage.windows.iter())
        .filter(|window| governs(window, model))
        .collect();
    let weekly = |window: &UsageWindow| window.window_minutes.unwrap_or(0) >= WEEK_MINUTES;
    let room = governing.iter().all(|window| {
        if weekly(window) {
            return window.used_at(now_ms) < WEEK_CAP;
        }
        window.used_at(now_ms) < SHORT_WINDOW_CAP
            && projected(window, candidate.burn, now_ms) < 100.0
    });
    if !room || governing.is_empty() {
        return None;
    }
    let weeks: Vec<&UsageWindow> = governing.iter().copied().filter(|w| weekly(w)).collect();
    let limiting = if weeks.is_empty() { governing } else { weeks };
    let left = (limiting.iter())
        .map(|window| 100.0 - window.used_at(now_ms))
        .fold(100.0, f64::min);
    let deadline = (limiting.iter())
        .filter(|window| !window.is_past_reset(now_ms))
        .filter_map(|window| window.resets_at.as_ref()?.unix_ms())
        .min();
    Some(Standing {
        id: candidate.id,
        left,
        headroom: left * weight(&usage.provider, usage.plan.as_deref()),
        typical: candidate.typical.unwrap_or(ENOUGH_FOR_A_CHAT),
        deadline,
    })
}

/// How full the window will be at its reset if the chats running under it keep going at `burn`.
fn projected(window: &UsageWindow, burn: f64, now_ms: u64) -> f64 {
    let used = window.used_at(now_ms);
    let reset = (window.resets_at.as_ref())
        .and_then(|reset| reset.unix_ms())
        .filter(|_| !window.is_past_reset(now_ms));
    let minutes = reset.map_or(0.0, |reset| (reset - now_ms) as f64 / 60_000.0);
    used + burn * minutes
}

/// The account a chat of `model` goes to, of `candidates`, or none when no account has a
/// reading that leaves it room.
pub fn place(
    candidates: &[Candidate],
    model: Option<&str>,
    aim: Aim,
    now_ms: u64,
) -> Option<Placement> {
    let standings: Vec<Standing> = (candidates.iter())
        .filter_map(|candidate| standing(candidate, model, now_ms))
        .collect();
    let most_left = |among: &[&Standing]| -> Option<Placement> {
        let best = among
            .iter()
            .max_by(|a, b| a.headroom.total_cmp(&b.headroom))?;
        Some(Placement {
            id: best.id.to_owned(),
            why: format!("It has the most of its week left, {:.0}%", best.left),
        })
    };
    let all: Vec<&Standing> = standings.iter().collect();
    if aim == Aim::Headroom {
        return most_left(&all);
    }
    let mut enough: Vec<&Standing> = (all.iter().copied())
        .filter(|standing| standing.left >= standing.typical)
        .collect();
    enough.sort_by_key(|standing| standing.deadline.unwrap_or(u64::MAX));
    let Some(soonest) = enough.first() else {
        return most_left(&all);
    };
    let together = |standing: &&Standing| match (standing.deadline, soonest.deadline) {
        (Some(one), Some(other)) => one.abs_diff(other) <= SAME_DEADLINE_MS,
        (one, other) => one == other,
    };
    let ending: Vec<&Standing> = enough.iter().copied().filter(together).collect();
    let best = (ending.iter())
        .max_by(|a, b| a.headroom.total_cmp(&b.headroom))
        .unwrap_or(soonest);
    let why = match (ending.len(), best.deadline) {
        (_, None) => format!("It has {:.0}% of its week left", best.left),
        (1, Some(_)) => format!("Its week ends soonest, with {:.0}% still unused", best.left),
        (_, Some(_)) => format!(
            "Its week ends soonest, with the most still unused, {:.0}%",
            best.left
        ),
    };
    Some(Placement {
        id: best.id.to_owned(),
        why,
    })
}
