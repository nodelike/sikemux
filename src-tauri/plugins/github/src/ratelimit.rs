// GitHub's rate limits. Once a limit is spent, every request until it resets
// is refused here without reaching GitHub, since GitHub treats requests made
// into a spent limit as abuse and holds the account back for longer.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde::Serialize;

use crate::error::{GithubError, GithubResult};

/// GitHub's own advice for a secondary limit that names no time: wait at least a minute.
const SECONDARY_WAIT_SECS: u64 = 60;
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
    /// Fewer than a tenth of the hour's requests are left.
    pub near: bool,
}

/// Each account's token has a limit of its own.
static STATES: Mutex<BTreeMap<String, Budget>> = Mutex::new(BTreeMap::new());

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

fn number(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// How long to hold off after this answer, if it says a limit is spent. GitHub
/// reports a spent limit as a 403 or 429 with a wait, with none left, or with
/// a message naming a secondary limit, which is worth telling apart from a
/// token that simply may not.
pub fn wait_for(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Option<u64> {
    if !matches!(status.as_u16(), 403 | 429) {
        return None;
    }
    if let Some(retry_after) = number(headers, "retry-after") {
        return Some(retry_after);
    }
    if number(headers, "x-ratelimit-remaining") == Some(0) {
        let reset = number(headers, "x-ratelimit-reset").unwrap_or(0);
        return Some(reset.saturating_sub(now_secs()));
    }
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    (status.as_u16() == 429 || text.contains("secondary rate limit")).then_some(SECONDARY_WAIT_SECS)
}

/// Refuses a request while a spent limit has not yet reset.
pub fn check(account: &str) -> GithubResult<()> {
    let now = now_secs();
    let mut states = STATES
        .lock()
        .map_err(|_| GithubError::Transport("rate limit state".into()))?;
    let Some(state) = states.get_mut(account) else {
        return Ok(());
    };
    match state.resets_at {
        Some(at) if state.limited && at > now => Err(GithubError::RateLimited {
            resets_in_secs: at - now,
        }),
        _ => {
            state.limited = false;
            Ok(())
        }
    }
}

/// Keeps what an answer says about the limit, and holds requests back if it is spent.
pub fn observe(account: &str, status: StatusCode, headers: &HeaderMap, body: &[u8]) {
    let Ok(mut states) = STATES.lock() else {
        return;
    };
    let state = states.entry(account.to_string()).or_default();
    if let Some(remaining) = number(headers, "x-ratelimit-remaining") {
        state.remaining = Some(remaining);
        state.limit = number(headers, "x-ratelimit-limit").or(state.limit);
        state.resets_at = number(headers, "x-ratelimit-reset").or(state.resets_at);
    }
    if let Some(wait) = wait_for(status, headers, body) {
        state.limited = true;
        state.resets_at = Some(now_secs() + wait.max(1));
    }
}

pub fn budget(account: &str) -> Budget {
    let _ = check(account);
    let mut budget = STATES
        .lock()
        .ok()
        .and_then(|states| states.get(account).copied())
        .unwrap_or_default();
    budget.near =
        matches!((budget.remaining, budget.limit), (Some(left), Some(all)) if left < all / 10);
    budget
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
    fn a_spent_limit_waits_until_it_resets() {
        let reset = (now_secs() + 120).to_string();
        let spent = headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", &reset),
        ]);
        let wait = wait_for(StatusCode::FORBIDDEN, &spent, b"{}").expect("limited");
        assert!((118..=120).contains(&wait), "{wait}");
    }

    #[test]
    fn a_secondary_limit_without_a_time_waits_a_minute() {
        let body = br#"{"message":"You have exceeded a secondary rate limit. Please wait a few minutes."}"#;
        let some_left = headers(&[("x-ratelimit-remaining", "4000")]);
        assert_eq!(wait_for(StatusCode::FORBIDDEN, &some_left, body), Some(60));
        assert_eq!(
            wait_for(
                StatusCode::FORBIDDEN,
                &headers(&[("retry-after", "30")]),
                b""
            ),
            Some(30)
        );
    }

    #[test]
    fn once_spent_nothing_is_sent_until_it_resets() {
        let account = "limit.test:someone";
        observe(
            account,
            StatusCode::OK,
            &headers(&[
                ("x-ratelimit-remaining", "12"),
                ("x-ratelimit-limit", "5000"),
            ]),
            b"",
        );
        assert!(check(account).is_ok());
        assert_eq!(
            (budget(account).remaining, budget(account).limit),
            (Some(12), Some(5000))
        );
        assert!(budget(account).near);
        observe(
            account,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("retry-after", "90")]),
            b"",
        );
        match check(account) {
            Err(GithubError::RateLimited { resets_in_secs }) => {
                assert!((89..=90).contains(&resets_in_secs))
            }
            other => panic!("expected to be held back, got {other:?}"),
        }
        assert!(budget(account).limited);
        assert!(
            check("limit.test:another").is_ok(),
            "another account has its own limit"
        );
    }

    #[test]
    fn a_plain_refusal_is_not_a_limit() {
        let some_left = headers(&[("x-ratelimit-remaining", "4000")]);
        assert_eq!(
            wait_for(
                StatusCode::FORBIDDEN,
                &some_left,
                br#"{"message":"Resource not accessible by integration"}"#
            ),
            None
        );
        assert_eq!(
            wait_for(StatusCode::OK, &headers(&[("retry-after", "5")]), b""),
            None
        );
    }
}
