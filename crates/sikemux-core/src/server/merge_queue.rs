//! What the two merge queues share: Auto's, for approved pull requests, and the
//! one agents land through with `git_commit`. Each base checks its waiting
//! changes stacked together, finds whose change broke a failed check, and, when
//! the base fails its own check, tries each change alone for one that mends it.

use std::path::Path;
use std::sync::Arc;

use sikemux_process::clock::epoch_ms;

use super::checks;
use super::Core;

/// How many changes one check takes in.
pub(crate) const MAX_TOGETHER: usize = 8;
/// How long a base that fails its own check goes before the queue looks whether it moved.
const RED_BASE_LOOK_MS: u64 = 5 * 60_000;

/// What a queue keeps about one base while the core runs.
#[derive(Default)]
pub(crate) struct Lane {
    pub running: bool,
    /// A check of several failed and could not say whose fault it was.
    pub one_at_a_time: bool,
    red: Option<RedBase>,
}

struct RedBase {
    tip: String,
    look_at: u64,
}

/// One change waiting in a lane, oldest first.
pub(crate) struct Waiter<'a> {
    /// The base commit it was tried alone on, while that base failed its own check.
    pub failed_over: Option<&'a str>,
    /// It has to go by itself, as a change its code host checks does.
    pub alone: bool,
}

pub(crate) enum Next {
    /// Check this many of the waiting changes, from the front, together.
    Take(usize),
    /// Every change was tried on the red base; look again at this time.
    LookAt(u64),
}

impl Lane {
    pub(crate) fn red_tip(&self) -> Option<&str> {
        self.red.as_ref().map(|red| red.tip.as_str())
    }

    /// The base at `tip` fails its own check, so nothing can be shown good on top
    /// of it: each change is tried alone, for one that mends it.
    pub(crate) fn mark_red(&mut self, tip: &str) {
        self.red = Some(RedBase {
            tip: tip.to_owned(),
            look_at: epoch_ms() + RED_BASE_LOOK_MS,
        });
    }

    pub(crate) fn clear_red(&mut self) {
        self.red = None;
    }

    /// Which of `waiting` the next check takes, by where it starts and how many.
    pub(crate) fn next(&self, waiting: &[Waiter<'_>]) -> Option<(usize, Next)> {
        let front = waiting.first()?;
        let Some(red) = &self.red else {
            let together = match front.alone || self.one_at_a_time {
                true => 1,
                false => (waiting.iter().take(MAX_TOGETHER))
                    .take_while(|w| !w.alone)
                    .count(),
            };
            return Some((0, Next::Take(together)));
        };
        match (waiting.iter()).position(|w| w.failed_over != Some(red.tip.as_str())) {
            Some(at) => Some((at, Next::Take(1))),
            None if epoch_ms() < red.look_at => Some((0, Next::LookAt(red.look_at))),
            None => Some((0, Next::Take(1))),
        }
    }
}

/// Who broke a check of changes stacked on a base.
pub(crate) enum Blame {
    /// The base fails the check by itself.
    BaseRed,
    /// The change at this place in the stack.
    Broke(usize),
    Unknown,
}

/// A check of `stacked` on `tip` failed, after `failing_took`: whether the base
/// fails it by itself, else which change holds the first commit that fails it.
/// Each change is its commits, oldest first.
pub(crate) async fn blame(
    core: &Arc<Core>,
    dir: &Path,
    tip: &str,
    stacked: &[Vec<String>],
    command: &str,
    agent: &str,
    failing_took: u64,
) -> Result<Blame, String> {
    if !checks::run_in_queue(core, dir, tip, command, agent)
        .await?
        .passed
    {
        return Ok(Blame::BaseRed);
    }
    if let [_] = stacked {
        return Ok(Blame::Broke(0));
    }
    let commits = stacked.iter().flatten().cloned().collect();
    let found =
        checks::first_failing_in_queue(core, dir, commits, tip, command, agent, failing_took).await;
    Ok(found
        .ok()
        .and_then(|sha| stacked.iter().position(|change| change.contains(&sha)))
        .map_or(Blame::Unknown, Blame::Broke))
}
