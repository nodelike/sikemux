//! Testing knowledge again, on a slow schedule and soon after a brief carried
//! it on a test that was not fresh. Every test starts here, a few at a time,
//! and nothing waits for one: a brief carries what the last test found. The
//! same wake forgets goal rules whose goal card was removed.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::memory::{LastCheck, MemoryCheck, Stale};

use super::{tell_clients, Core};

const EVERY_MS: u64 = 6 * 60 * 60 * 1000;
/// How often goal rules whose goal card was removed are forgotten.
const SWEEP_MS: u64 = 60 * 60 * 1000;
/// How old a test may be for a brief to carry knowledge without asking for
/// another.
pub(super) const FRESH_MS: u64 = 10 * 60 * 1000;
const MAX_AT_ONCE: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

struct Due {
    folder: String,
    id: u64,
    check: MemoryCheck,
}

/// Starts the tests that are due, as many as the budget allows, and wakes
/// again when the next one is.
pub(crate) fn settle(core: &Arc<Core>) {
    super::forget_gone_goals(core);
    let now = epoch_ms();
    let mut next = now + SWEEP_MS;
    let mut due = Vec::new();
    {
        let mut state = core.memory.state.locked();
        let mut room = MAX_AT_ONCE.saturating_sub(state.checking.len());
        for held in state.held.values() {
            for known in &held.knowledge {
                let Some(check) = &known.check else {
                    continue;
                };
                if state.checking.contains(&known.id) {
                    continue;
                }
                let every = if state.wanted.contains(&known.id) {
                    FRESH_MS
                } else {
                    EVERY_MS
                };
                let at = known
                    .checked
                    .as_ref()
                    .map_or(0, |checked| checked.at + every);
                if at > now {
                    next = next.min(at);
                } else if room > 0 {
                    room -= 1;
                    due.push(Due {
                        folder: held.folder.clone(),
                        id: known.id,
                        check: check.clone(),
                    });
                }
            }
        }
        for due in &due {
            state.wanted.remove(&due.id);
            state.checking.insert(due.id);
        }
    }
    for due in due {
        let core = core.clone();
        tokio::spawn(async move {
            let result = run(Path::new(&due.folder), &due.check).await;
            record(&core, &due, result);
        });
    }
    core.memory.wake.at(next);
}

async fn run(folder: &Path, check: &MemoryCheck) -> LastCheck {
    let failed = match check {
        MemoryCheck::PathExists { path } => {
            let path = folder.join(path);
            match tokio::fs::try_exists(&path).await {
                Ok(true) => None,
                Ok(false) => Some(format!("{} does not exist", path.display())),
                Err(error) => Some(format!("{} cannot be read: {error}", path.display())),
            }
        }
        MemoryCheck::HostAnswers { host, port } => {
            let connect = tokio::net::TcpStream::connect((host.as_str(), *port));
            match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
                Ok(Ok(_)) => None,
                Ok(Err(error)) => Some(format!("{host}:{port} does not answer: {error}")),
                Err(_) => Some(format!(
                    "{host}:{port} did not answer within {} seconds",
                    CONNECT_TIMEOUT.as_secs()
                )),
            }
        }
    };
    LastCheck {
        at: epoch_ms(),
        passed: failed.is_none(),
        why: failed,
    }
}

fn record(core: &Core, due: &Due, result: LastCheck) {
    let mut state = core.memory.state.locked();
    state.checking.remove(&due.id);
    let Some(known) = state
        .held
        .get_mut(&due.folder)
        .and_then(|held| held.knowledge.iter_mut().find(|known| known.id == due.id))
    else {
        core.memory.wake.now();
        return;
    };
    if known.check.as_ref() != Some(&due.check) {
        core.memory.wake.now();
        return;
    }
    let was_stale = known.stale.is_some();
    match (&known.stale, result.passed) {
        (None, false) => known.stale = Some(Stale::Failed { since: result.at }),
        (Some(Stale::Failed { .. }), true) => known.stale = None,
        _ => {}
    }
    let stale_moved = was_stale != known.stale.is_some();
    known.checked = Some(result);
    core.memory.save(&mut state, &due.folder);
    drop(state);
    tell_clients(core, stale_moved);
}

/// Asks for knowledge a brief carries to be tested soon when its last test
/// is not fresh.
pub(super) fn want(core: &Core, ids: impl IntoIterator<Item = u64>) {
    let mut state = core.memory.state.locked();
    let before = state.wanted.len();
    state.wanted.extend(ids);
    if state.wanted.len() != before {
        drop(state);
        core.memory.wake.now();
    }
}
