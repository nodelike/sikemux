// GitLab's rate limit. Every answer says how many requests are left and when
// the count starts again; a refused request says how long to wait, and with
// nothing said, requests are held back for a while, longer each time.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde::Serialize;

use crate::error::{GitlabError, GitlabResult};

const FIRST_WAIT_SECS: u64 = 60;
const LONGEST_WAIT_SECS: u64 = 15 * 60;
/// A wait this short is sat out and the read tried again, rather than failing it.
pub const SHORT_WAIT_SECS: u64 = 3;

#[derive(Serialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    /// Whether requests are being held back until `resets_at`.
    pub limited: bool,
    /// Seconds since the epoch.
    pub resets_at: Option<u64>,
    pub remaining: Option<u64>,
    pub limit: Option<u64>,
    /// Fewer than a tenth of the requests are left.
    pub near: bool,
}

#[derive(Clone, Copy, Default)]
struct State {
    budget: Budget,
    /// Refusals in a row, which lengthen the wait.
    strikes: u32,
}

/// Each account has a limit of its own.
static STATES: Mutex<BTreeMap<String, State>> = Mutex::new(BTreeMap::new());
/// The wait set by the latest refusal, for the error that reports it.
static LATEST_WAIT: AtomicU64 = AtomicU64::new(FIRST_WAIT_SECS);

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

fn number(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// How long a refusal says to wait, or how long to wait after this many refusals in a row.
fn wait_after(headers: &HeaderMap, strikes: u32) -> u64 {
    number(headers, "retry-after").unwrap_or_else(|| {
        FIRST_WAIT_SECS
            .saturating_mul(1 << strikes.saturating_sub(1).min(6))
            .min(LONGEST_WAIT_SECS)
    })
}

/// The wait a 429 names, for deciding whether to sit it out; `None` for any other answer.
pub fn named_wait(status: StatusCode, headers: &HeaderMap) -> Option<u64> {
    (status == StatusCode::TOO_MANY_REQUESTS)
        .then(|| number(headers, "retry-after"))
        .flatten()
}

pub fn check(account: &str) -> GitlabResult<()> {
    let now = now_secs();
    let mut states = STATES
        .lock()
        .map_err(|_| GitlabError::Transport("rate limit state".into()))?;
    let Some(state) = states.get_mut(account) else {
        return Ok(());
    };
    match state.budget.resets_at {
        Some(at) if state.budget.limited && at > now => Err(GitlabError::RateLimited {
            resets_in_secs: at - now,
        }),
        _ => {
            state.budget.limited = false;
            Ok(())
        }
    }
}

pub fn observe(account: &str, status: StatusCode, headers: &HeaderMap) {
    let Ok(mut states) = STATES.lock() else {
        return;
    };
    let state = states.entry(account.to_string()).or_default();
    if let Some(limit) = number(headers, "ratelimit-limit") {
        state.budget.limit = Some(limit);
    }
    if let Some(remaining) = number(headers, "ratelimit-remaining") {
        state.budget.remaining = Some(remaining);
        state.budget.near = state
            .budget
            .limit
            .is_some_and(|limit| remaining * 10 < limit);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        state.strikes = state.strikes.saturating_add(1);
        let named = number(headers, "ratelimit-reset").and_then(|at| at.checked_sub(now_secs()));
        let wait = named
            .unwrap_or_else(|| wait_after(headers, state.strikes))
            .max(1);
        state.budget.limited = true;
        state.budget.resets_at = Some(now_secs() + wait);
        LATEST_WAIT.store(wait, Ordering::Relaxed);
    } else if status.is_success() {
        state.strikes = 0;
        if let Some(at) = number(headers, "ratelimit-reset") {
            state.budget.resets_at = Some(at);
        }
    }
}

pub fn latest_wait() -> u64 {
    LATEST_WAIT.load(Ordering::Relaxed)
}

pub fn budget(account: &str) -> Budget {
    let _ = check(account);
    STATES
        .lock()
        .ok()
        .and_then(|states| states.get(account).map(|state| state.budget))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).expect("header"));
        }
        map
    }

    #[test]
    fn each_refusal_in_a_row_waits_longer_up_to_a_limit() {
        let none = HeaderMap::new();
        assert_eq!(wait_after(&none, 1), 60);
        assert_eq!(wait_after(&none, 2), 120);
        assert_eq!(wait_after(&none, 3), 240);
        assert_eq!(wait_after(&none, 20), LONGEST_WAIT_SECS);
        assert_eq!(wait_after(&headers(&[("retry-after", "7")]), 5), 7);
    }

    #[test]
    fn once_refused_nothing_is_sent_until_the_wait_is_over() {
        let account = "limit-test";
        observe(
            account,
            StatusCode::OK,
            &headers(&[("ratelimit-limit", "2000"), ("ratelimit-remaining", "150")]),
        );
        assert!(check(account).is_ok());
        assert!(budget(account).near);
        assert_eq!(budget(account).limit, Some(2000));
        assert_eq!(budget(account).remaining, Some(150));
        observe(account, StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new());
        match check(account) {
            Err(GitlabError::RateLimited { resets_in_secs }) => {
                assert!((59..=60).contains(&resets_in_secs))
            }
            Ok(()) => panic!("expected to be held back"),
            Err(other) => panic!("expected to be held back, got {other}"),
        }
        assert_eq!(latest_wait(), 60);
        assert!(check("another-account").is_ok());
    }

    #[test]
    fn only_a_429_names_a_wait() {
        let after = headers(&[("retry-after", "2")]);
        assert_eq!(named_wait(StatusCode::TOO_MANY_REQUESTS, &after), Some(2));
        assert_eq!(named_wait(StatusCode::OK, &after), None);
        assert_eq!(
            named_wait(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new()),
            None
        );
    }
}
