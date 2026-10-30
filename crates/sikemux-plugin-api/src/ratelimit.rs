//! A service's rate limit, as its answers describe it. Once a limit is spent,
//! requests are refused here until it resets without reaching the service,
//! since services treat requests made into a spent limit as abuse and hold
//! the account back for longer.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde::Serialize;
use sikemux_process::clock::epoch_secs;
use sikemux_process::sync::Locked;

use crate::service::ServiceError;

/// The wait after a refusal that names none, doubled for each refusal in a row.
const FIRST_WAIT_SECS: u64 = 60;
const LONGEST_WAIT_SECS: u64 = 15 * 60;
/// A wait this short is sat out and the request sent again, rather than failing it.
const SHORT_WAIT_SECS: u64 = 3;

/// The headers a service reports its limit in.
pub struct LimitHeaders {
    pub limit: &'static str,
    pub remaining: Option<&'static str>,
    /// When the count starts again, in seconds since the epoch.
    pub reset: Option<&'static str>,
    /// Says outright that few requests are left.
    pub near: Option<&'static str>,
}

#[derive(Serialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    /// Whether requests are being held back until `resets_at`.
    pub limited: bool,
    /// Seconds since the epoch.
    pub resets_at: Option<u64>,
    pub remaining: Option<u64>,
    pub limit: Option<u64>,
    /// Few requests are left: under a tenth, or as few as the service says is near.
    pub near: bool,
}

#[derive(Clone, Copy, Default)]
struct State {
    budget: Budget,
    /// Refusals in a row, which lengthen the wait.
    strikes: u32,
}

/// Each account has a limit of its own.
pub struct RateLimits {
    headers: LimitHeaders,
    states: Mutex<BTreeMap<String, State>>,
    /// The wait set by the latest refusal, for the error that reports it.
    latest_wait: AtomicU64,
}

fn number(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

fn backoff(strikes: u32) -> u64 {
    FIRST_WAIT_SECS
        .saturating_mul(1 << strikes.saturating_sub(1).min(6))
        .min(LONGEST_WAIT_SECS)
}

fn release(budget: &mut Budget, now: u64) -> Option<u64> {
    match budget.resets_at {
        Some(at) if budget.limited && at > now => Some(at - now),
        _ => {
            budget.limited = false;
            None
        }
    }
}

impl RateLimits {
    pub const fn new(headers: LimitHeaders) -> Self {
        Self {
            headers,
            states: Mutex::new(BTreeMap::new()),
            latest_wait: AtomicU64::new(FIRST_WAIT_SECS),
        }
    }

    /// How long to hold off after this answer, if it says a limit is spent. A
    /// 403 counts only when it says so, since it may just mean the account may not.
    fn spent(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
        strikes: u32,
    ) -> Option<u64> {
        if !matches!(status.as_u16(), 403 | 429) {
            return None;
        }
        if let Some(after) = number(headers, "retry-after") {
            return Some(after);
        }
        let named = |name: Option<&str>| name.and_then(|name| number(headers, name));
        let remaining = named(self.headers.remaining);
        let none_left = remaining == Some(0);
        // A 429 with requests still left is a burst limit; the reset names the hourly one.
        let hourly_spent = none_left || (status == 429 && remaining.is_none());
        if let Some(at) = named(self.headers.reset).filter(|_| hourly_spent) {
            return Some(at.saturating_sub(epoch_secs()));
        }
        let secondary = String::from_utf8_lossy(body)
            .to_ascii_lowercase()
            .contains("secondary rate limit");
        (status == StatusCode::TOO_MANY_REQUESTS || secondary || none_left)
            .then(|| backoff(strikes.saturating_add(1)))
    }

    /// The wait this answer means, when it says a limit is spent.
    pub fn wait_for(
        &self,
        account: &str,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Option<u64> {
        let strikes = self
            .states
            .locked()
            .get(account)
            .map_or(0, |state| state.strikes);
        self.spent(status, headers, body, strikes)
    }

    /// A wait short enough to sit out before sending the request again.
    pub fn short_wait(
        &self,
        account: &str,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Option<Duration> {
        self.wait_for(account, status, headers, body)
            .filter(|secs| *secs <= SHORT_WAIT_SECS)
            .map(|secs| Duration::from_secs(secs.max(1)))
    }

    /// Refuses a request while a spent limit has not yet reset.
    pub fn check<S>(&self, account: &str) -> Result<(), ServiceError<S>> {
        let mut states = self.states.locked();
        let Some(state) = states.get_mut(account) else {
            return Ok(());
        };
        match release(&mut state.budget, epoch_secs()) {
            Some(secs) => Err(ServiceError::RateLimited {
                resets_in_secs: Some(secs),
            }),
            None => Ok(()),
        }
    }

    /// Keeps what an answer says about the limit, and holds requests back if it is spent.
    pub fn observe(&self, account: &str, status: StatusCode, headers: &HeaderMap, body: &[u8]) {
        let mut states = self.states.locked();
        let state = states.entry(account.to_owned()).or_default();
        let named = |name: Option<&str>| name.and_then(|name| number(headers, name));
        if let Some(limit) = number(headers, self.headers.limit) {
            state.budget.limit = Some(limit);
        }
        if let Some(remaining) = named(self.headers.remaining) {
            state.budget.remaining = Some(remaining);
            state.budget.near = state
                .budget
                .limit
                .is_some_and(|limit| remaining * 10 < limit);
            state.budget.resets_at = named(self.headers.reset).or(state.budget.resets_at);
        }
        if let Some(near) = self
            .headers
            .near
            .and_then(|name| headers.get(name))
            .and_then(|value| value.to_str().ok())
        {
            state.budget.near = near.trim().eq_ignore_ascii_case("true");
        }
        match self.spent(status, headers, body, state.strikes) {
            Some(wait) => {
                let wait = wait.max(1);
                state.strikes = state.strikes.saturating_add(1);
                state.budget.limited = true;
                state.budget.resets_at = Some(epoch_secs() + wait);
                self.latest_wait.store(wait, Ordering::Relaxed);
            }
            None if status.is_success() => state.strikes = 0,
            None => {}
        }
    }

    pub fn latest_wait(&self) -> u64 {
        self.latest_wait.load(Ordering::Relaxed)
    }

    pub fn budget(&self, account: &str) -> Budget {
        let mut states = self.states.locked();
        let Some(state) = states.get_mut(account) else {
            return Budget::default();
        };
        release(&mut state.budget, epoch_secs());
        state.budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    struct Test;

    impl crate::Service for Test {
        const ID: &'static str = "test";
        const NAME: &'static str = "Test";
    }

    const GITHUB: LimitHeaders = LimitHeaders {
        limit: "x-ratelimit-limit",
        remaining: Some("x-ratelimit-remaining"),
        reset: Some("x-ratelimit-reset"),
        near: None,
    };

    fn headers(pairs: &[(&'static str, &str)]) -> Result<HeaderMap, String> {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                *name,
                HeaderValue::from_str(value).map_err(|error| error.to_string())?,
            );
        }
        Ok(map)
    }

    #[test]
    fn a_spent_limit_waits_until_it_resets() -> Result<(), String> {
        let limits = RateLimits::new(GITHUB);
        let reset = (epoch_secs() + 120).to_string();
        let spent = headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", &reset),
        ])?;
        let wait = limits.wait_for("a", StatusCode::FORBIDDEN, &spent, b"{}");
        assert!(
            wait.is_some_and(|wait| (118..=120).contains(&wait)),
            "{wait:?}"
        );
        Ok(())
    }

    #[test]
    fn a_plain_refusal_is_not_a_limit() -> Result<(), String> {
        let limits = RateLimits::new(GITHUB);
        let some_left = headers(&[("x-ratelimit-remaining", "4000")])?;
        let body = br#"{"message":"Resource not accessible by integration"}"#;
        assert_eq!(
            limits.wait_for("a", StatusCode::FORBIDDEN, &some_left, body),
            None
        );
        let after = headers(&[("retry-after", "5")])?;
        assert_eq!(limits.wait_for("a", StatusCode::OK, &after, b""), None);
        let secondary = br#"{"message":"You have exceeded a secondary rate limit."}"#;
        assert_eq!(
            limits.wait_for("a", StatusCode::FORBIDDEN, &some_left, secondary),
            Some(60)
        );
        let reset = (epoch_secs() + 1800).to_string();
        let burst = headers(&[
            ("x-ratelimit-remaining", "4580"),
            ("x-ratelimit-reset", &reset),
        ])?;
        assert_eq!(
            limits.wait_for("a", StatusCode::TOO_MANY_REQUESTS, &burst, secondary),
            Some(60)
        );
        Ok(())
    }

    #[test]
    fn once_refused_nothing_is_sent_until_the_wait_is_over_and_each_refusal_waits_longer(
    ) -> Result<(), String> {
        let limits = RateLimits::new(GITHUB);
        let some_left = headers(&[
            ("x-ratelimit-remaining", "12"),
            ("x-ratelimit-limit", "5000"),
        ])?;
        limits.observe("a", StatusCode::OK, &some_left, b"");
        assert!(limits.check::<Test>("a").is_ok());
        assert!(limits.budget("a").near);
        limits.observe("a", StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), b"");
        assert!(matches!(
            limits.check::<Test>("a"),
            Err(ServiceError::RateLimited { resets_in_secs: Some(secs) }) if (59..=60).contains(&secs)
        ));
        assert!(limits.check::<Test>("another").is_ok());
        limits.observe("a", StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), b"");
        assert_eq!(limits.latest_wait(), 120);
        assert_eq!(backoff(20), LONGEST_WAIT_SECS);
        Ok(())
    }
}
