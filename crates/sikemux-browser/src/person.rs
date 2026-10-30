//! When the person last typed in each tab, so an agent never moves a page out
//! from under them, such as one they are paying or signing in on.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use sikemux_process::sync::Locked;

const RECENT: Duration = Duration::from_secs(60);

static TYPED: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

#[cfg(target_os = "macos")]
pub(crate) fn typed_in(tab_id: &str) {
    let now = Instant::now();
    let mut typed = TYPED.locked();
    typed.retain(|(id, at)| id != tab_id && now.duration_since(*at) < RECENT);
    typed.push((tab_id.to_owned(), now));
}

/// How long ago the person typed in the tab, if they did lately.
pub(crate) fn typed_lately(tab_id: &str) -> Option<Duration> {
    TYPED
        .locked()
        .iter()
        .find(|(id, _)| id == tab_id)
        .map(|(_, at)| at.elapsed())
        .filter(|ago| *ago < RECENT)
}

/// Refuses to move a tab the person typed in lately.
pub(crate) fn leave_be(tab_id: &str) -> Result<(), String> {
    match typed_lately(tab_id) {
        Some(ago) => Err(format!(
            "the person typed in this tab {}s ago, so it was left where it is; use browser_navigate with newTab: true to look elsewhere",
            ago.as_secs()
        )),
        None => Ok(()),
    }
}
