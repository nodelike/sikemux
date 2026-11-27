//! Which of the person's accounts with one agent a chat goes to, and when a saved limit reset is
//! spent. A chat goes to the account whose limit for its model refills soonest and still has
//! room, as what a limit leaves unused is lost at its refill. A reset lasts months, so it is spent
//! only when no account has room and the wait for the next refill is long; in the reset's own last
//! week its account goes first, so it is used rather than lost.

use super::usage::{ResetKind, Usage, UsageWindow};

/// An account a chat could go to, with its last reading of its plan, if any.
pub struct Candidate<'a> {
    pub id: &'a str,
    pub usage: Option<&'a Usage>,
    /// How fast the chats running on it fill its windows shorter than a week, in percent a
    /// minute.
    pub burn: f64,
}

/// The chat being placed: its model, and whether it answers a message the person sent, which
/// may use the last tenth of a limit that agents leave for the person.
#[derive(Clone, Copy, Default)]
pub struct Chat<'a> {
    pub model: Option<&'a str>,
    pub person: bool,
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

/// The account chosen, and why, as a person reads it.
#[derive(Debug, PartialEq)]
pub struct Placement {
    pub id: String,
    pub why: String,
}

const HOUR_MS: u64 = 60 * 60 * 1000;
const WEEK_MS: u64 = 7 * 24 * HOUR_MS;
const WEEK_MINUTES: u64 = 10_080;
/// How full a window shorter than a week may be for a new chat to start under it.
const SHORT_WINDOW_CAP: f64 = 80.0;
/// How full a week may be for chats agents started; the rest is the person's.
const AGENTS_CAP: f64 = 90.0;
/// How long before a week refills its last tenth opens to agents too, so it isn't lost.
const RESERVE_LIFTS_MS: u64 = 6 * HOUR_MS;

/// Whether `window` limits a chat on `model`: every window for the whole plan, and a model's
/// own week only for that model.
fn governs(window: &UsageWindow, model: Option<&str>) -> bool {
    let Some(scope) = &window.scope else {
        return true;
    };
    let (scope, model) = (
        scope.to_lowercase(),
        model.unwrap_or_default().to_lowercase(),
    );
    !model.is_empty() && (model.contains(&scope) || scope.contains(&model))
}

fn weekly(window: &UsageWindow) -> bool {
    window.window_minutes.unwrap_or(0) >= WEEK_MINUTES
}

fn refill_at(window: &UsageWindow, now_ms: u64) -> Option<u64> {
    (window.resets_at.as_ref())
        .and_then(|reset| reset.unix_ms())
        .filter(|_| !window.is_past_reset(now_ms))
}

/// How full a week may get for `chat`: all of it for the person's message, nine tenths for
/// agents' until the week is about to refill.
fn cap(window: &UsageWindow, chat: Chat, now_ms: u64) -> f64 {
    let refills_soon =
        refill_at(window, now_ms).is_some_and(|at| at.saturating_sub(now_ms) <= RESERVE_LIFTS_MS);
    match chat.person || refills_soon {
        true => 100.0,
        false => AGENTS_CAP,
    }
}

/// Whether the account can take no more of `chat` before a window over its model refills.
pub fn full(usage: &Usage, chat: Chat, now_ms: u64) -> bool {
    (usage.deciding()).any(|window| {
        governs(window, chat.model) && window.used_at(now_ms) >= cap(window, chat, now_ms)
    })
}

/// The account's resets that have not run out, each with the week over `model` it refills.
fn resets_over<'a>(
    usage: &'a Usage,
    model: Option<&'a str>,
    now_ms: u64,
) -> impl Iterator<Item = (&'a ResetKind, &'a UsageWindow)> {
    (usage.resets.iter().flat_map(|resets| &resets.kinds))
        .filter(move |kind| kind.left > 0 && kind.use_by.is_none_or(|by| by > now_ms))
        .filter_map(move |kind| {
            let window = (usage.deciding()).find(|window| {
                Some(&window.label) == kind.window.as_ref()
                    && weekly(window)
                    && governs(window, model)
            })?;
            Some((kind, window))
        })
}

/// A reset in its last week, which is spent at its account's first full limit, so it isn't lost.
fn expiring(kind: &ResetKind, now_ms: u64) -> bool {
    kind.use_by
        .is_some_and(|by| by.saturating_sub(now_ms) <= WEEK_MS)
}

/// Where an account stands for a chat it has room for.
struct Standing<'a> {
    id: &'a str,
    /// The model's own limit, such as Fable's week, else the plan's week.
    limit: &'a UsageWindow,
    /// The share of that limit left.
    left: f64,
    /// When that limit refills, in Unix milliseconds.
    refills: Option<u64>,
    /// That share in plans of the smallest tier.
    headroom: f64,
    /// It holds a reset in its last week for a limit over the chat.
    reset_expiring: bool,
}

/// Where the account stands for `chat`, or none when it has no room for it. A window a reset
/// in its last week can refill counts as having room. A chat whose model is not known counts
/// every model's own week as over it while `wary`.
fn standing<'a>(
    candidate: &Candidate<'a>,
    chat: Chat,
    wary: bool,
    now_ms: u64,
) -> Option<Standing<'a>> {
    let usage = candidate.usage?;
    let governing: Vec<&UsageWindow> = (usage.deciding())
        .filter(|window| governs(window, chat.model))
        .collect();
    let expiring_for: Vec<&UsageWindow> = resets_over(usage, chat.model, now_ms)
        .filter(|(kind, _)| expiring(kind, now_ms))
        .map(|(_, window)| window)
        .collect();
    let unknown_models =
        (usage.deciding()).filter(|window| wary && chat.model.is_none() && window.scope.is_some());
    let room = (governing.iter().copied().chain(unknown_models)).all(|window| {
        let used = window.used_at(now_ms);
        match weekly(window) {
            _ if expiring_for.contains(&window) => true,
            true => used < cap(window, chat, now_ms),
            false => used < SHORT_WINDOW_CAP && projected(window, candidate.burn, now_ms) < 100.0,
        }
    });
    if !room {
        return None;
    }
    let weeks = governing.iter().copied().filter(|window| weekly(window));
    let limit = (weeks.clone().find(|window| window.scope.is_some()))
        .or_else(|| weeks.clone().next())
        .or_else(|| governing.first().copied())?;
    let left = 100.0 - limit.used_at(now_ms);
    Some(Standing {
        id: candidate.id,
        limit,
        left,
        refills: refill_at(limit, now_ms),
        headroom: left * weight(&usage.provider, usage.plan.as_deref()),
        reset_expiring: !expiring_for.is_empty(),
    })
}

/// How full the window will be at its reset if the chats running under it keep going at `burn`.
fn projected(window: &UsageWindow, burn: f64, now_ms: u64) -> f64 {
    let minutes = refill_at(window, now_ms).map_or(0.0, |reset| (reset - now_ms) as f64 / 60_000.0);
    window.used_at(now_ms) + burn * minutes
}

/// The order accounts with room take a chat in. One holding a reset in its last week goes
/// first, so the reset is used rather than lost. Then the limit that refills soonest, since
/// its unused room is lost at the refill; limits refilling in the same hour go by the most
/// room left.
fn order(a: &Standing, b: &Standing) -> std::cmp::Ordering {
    let hour = |standing: &Standing| standing.refills.map_or(u64::MAX, |at| at / HOUR_MS);
    (b.reset_expiring.cmp(&a.reset_expiring))
        .then(hour(a).cmp(&hour(b)))
        .then(b.headroom.total_cmp(&a.headroom))
}

/// The accounts with room for `chat`, in the order they take it. A chat whose model is not
/// known keeps off an account with any model's week full, unless every account has one.
fn ranked<'a>(candidates: &[Candidate<'a>], chat: Chat, now_ms: u64) -> Vec<Standing<'a>> {
    let standings = |wary| -> Vec<Standing> {
        (candidates.iter())
            .filter_map(|candidate| standing(candidate, chat, wary, now_ms))
            .collect()
    };
    let mut ranked = match standings(true) {
        none if none.is_empty() && chat.model.is_none() => standings(false),
        wary => wary,
    };
    ranked.sort_by(order);
    ranked
}

/// The account `chat` goes to, of `candidates`, or none when no account has a reading that
/// leaves it room.
pub fn place(candidates: &[Candidate], chat: Chat, now_ms: u64) -> Option<Placement> {
    let ranked = ranked(candidates, chat, now_ms);
    let best = ranked.first()?;
    let limit = match &best.limit.scope {
        Some(scope) => scope.as_str(),
        None => "week",
    };
    let why = match (best.reset_expiring, ranked.len(), best.refills) {
        (true, ..) => "It holds a reset that runs out within a week, so it is used first".into(),
        (_, 1, _) | (_, _, None) => format!("It has {:.0}% of its {limit} left", best.left),
        _ => format!(
            "Its {limit} refills soonest, with {:.0}% still unused",
            best.left
        ),
    };
    Some(Placement {
        id: best.id.to_owned(),
        why,
    })
}

/// A reset to spend now, on the account it belongs to.
#[derive(Debug, PartialEq)]
pub struct Spend {
    pub account: String,
    /// The grant to spend, by its kind's id; none lets the agent choose.
    pub kind: Option<String>,
}

/// A reset held for a limit over a chat's model, at a full limit Claude would honour it on.
struct Held<'a> {
    account: &'a str,
    kind: &'a ResetKind,
    /// When the account's limit it refills would refill by itself.
    refills: Option<u64>,
}

/// How long chats on `model` would wait at their limits for a refill before a reset is worth
/// spending: a week shared out among the accounts under that limit, since they take turns.
fn worth_spending_after(candidates: &[Candidate]) -> u64 {
    let accounts = candidates
        .iter()
        .filter(|candidate| candidate.usage.is_some());
    WEEK_MS / accounts.count().max(1) as u64
}

/// When an account full for `chat` takes chats again: once every full window over it refills.
fn back_at(usage: &Usage, chat: Chat, now_ms: u64) -> Option<u64> {
    (usage.deciding())
        .filter(|window| governs(window, chat.model) && window.used_at(now_ms) >= 100.0)
        .map(|window| refill_at(window, now_ms))
        .try_fold(0, |latest, at| Some(latest.max(at?)))
        .filter(|latest| *latest > 0)
}

/// When the first account full for `chat` takes chats again, of those with a reading.
fn next_refill(candidates: &[Candidate], chat: Chat, now_ms: u64) -> Option<u64> {
    (candidates.iter())
        .filter_map(|candidate| back_at(candidate.usage?, chat, now_ms))
        .min()
}

fn held<'a>(candidates: &[Candidate<'a>], model: Option<&'a str>, now_ms: u64) -> Vec<Held<'a>> {
    (candidates.iter())
        .filter_map(|candidate| Some((candidate.id, candidate.usage?)))
        .flat_map(|(account, usage)| {
            resets_over(usage, model, now_ms)
                .filter(move |(kind, window)| kind.usable && window.used_at(now_ms) >= 100.0)
                .map(move |(kind, window)| Held {
                    account,
                    kind,
                    refills: refill_at(window, now_ms),
                })
        })
        .collect()
}

/// The reset to spend for a chat of `model` that reached its limit, if one should go now. One
/// in its last week goes at once. Otherwise only when no account has room even for the
/// person, and the next refill is further off than [`worth_spending_after`]; it then goes
/// where it buys the longest wait.
pub fn reset_to_spend(candidates: &[Candidate], model: Option<&str>, now_ms: u64) -> Option<Spend> {
    let held = held(candidates, model, now_ms);
    let spend = |held: &Held| Spend {
        account: held.account.to_owned(),
        kind: held.kind.id.clone(),
    };
    if let Some(lapsing) = held.iter().find(|held| expiring(held.kind, now_ms)) {
        return Some(spend(lapsing));
    }
    let chat = Chat {
        model,
        person: true,
    };
    if !ranked(candidates, chat, now_ms).is_empty() {
        return None;
    }
    let wait = next_refill(candidates, chat, now_ms)?.saturating_sub(now_ms);
    if wait <= worth_spending_after(candidates) {
        return None;
    }
    (held.iter())
        .max_by_key(|held| held.refills.unwrap_or(u64::MAX))
        .map(spend)
}

/// Chats on a model waiting for a refill while a reset for that limit is kept.
#[derive(Debug, PartialEq)]
pub struct Kept {
    /// When the first account takes chats again, in Unix milliseconds.
    pub refills: u64,
}

/// Whether chats on `model` wait for a refill while an account holds a reset for its limit:
/// no account has room even for the person, and no reset is due to be spent.
pub fn kept(candidates: &[Candidate], model: Option<&str>, now_ms: u64) -> Option<Kept> {
    let chat = Chat {
        model,
        person: true,
    };
    let waiting = ranked(candidates, chat, now_ms).is_empty()
        && reset_to_spend(candidates, model, now_ms).is_none()
        && !held(candidates, model, now_ms).is_empty();
    waiting.then_some(())?;
    Some(Kept {
        refills: next_refill(candidates, chat, now_ms)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::usage::{Resets, UsageResetAt};
    use super::*;

    const DAY: u64 = 24 * HOUR_MS;
    const NOW: u64 = 1_800_000_000_000;

    fn week(scope: Option<&str>, used: f64, refills_in: u64) -> UsageWindow {
        UsageWindow {
            label: scope.map_or("7d".to_owned(), |scope| format!("{scope} · 7d")),
            scope: scope.map(str::to_owned),
            used_percent: used,
            resets_at: Some(UsageResetAt::Unix(NOW + refills_in)),
            window_minutes: Some(WEEK_MINUTES),
        }
    }

    fn ultra(week_used: f64, fable_used: f64, refills_in: u64, fable_resets: u32) -> Usage {
        let resets = (fable_resets > 0).then(|| Resets {
            left: fable_resets,
            kinds: vec![ResetKind {
                id: Some("fable".into()),
                label: "Fable reset".into(),
                left: fable_resets,
                use_by: Some(NOW + 112 * DAY),
                window: Some("Fable · 7d".into()),
                usable: fable_used >= 100.0,
            }],
        });
        Usage {
            provider: "claude".into(),
            plan: None,
            tier: None,
            windows: vec![
                week(None, week_used, refills_in),
                week(Some("Fable"), fable_used, refills_in),
            ],
            unavailable_reason: None,
            resets,
            light: true,
        }
    }

    // The readings of 17 Nov 2026 the person checked the rule against.
    #[test]
    fn fable_goes_to_the_account_that_refills_first() {
        let kishore = ultra(65.0, 74.0, 5 * DAY + 3 * HOUR_MS, 0);
        let pers = ultra(45.0, 31.0, 2 * DAY + 7 * HOUR_MS, 1);
        let candidates = [
            Candidate {
                id: "kishore",
                usage: Some(&kishore),
                burn: 0.0,
            },
            Candidate {
                id: "pers",
                usage: Some(&pers),
                burn: 0.0,
            },
        ];
        let fable = Chat {
            model: Some("claude-fable-5-2"),
            person: false,
        };
        assert_eq!(
            place(&candidates, fable, NOW).map(|p| p.id).as_deref(),
            Some("pers")
        );
        assert_eq!(reset_to_spend(&candidates, fable.model, NOW), None);
    }
}
