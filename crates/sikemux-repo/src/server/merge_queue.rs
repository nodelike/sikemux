//! What the land queue keeps per base, and the per-repo checkout its checks take turns in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// How many changes one check takes in.
pub const MAX_TOGETHER: usize = 8;

struct RepoGate {
    checkout: Arc<tokio::sync::Mutex<()>>,
    pushing: AtomicUsize,
    pushed: tokio::sync::Notify,
}

impl Default for RepoGate {
    fn default() -> Self {
        Self {
            checkout: Arc::new(tokio::sync::Mutex::new(())),
            pushing: AtomicUsize::new(0),
            pushed: tokio::sync::Notify::new(),
        }
    }
}

struct PushingGuard {
    gate: Arc<RepoGate>,
}

impl Drop for PushingGuard {
    fn drop(&mut self) {
        self.gate.pushing.fetch_sub(1, Ordering::SeqCst);
        self.gate.pushed.notify_waiters();
    }
}

fn gate_for(repo: &Path) -> Arc<RepoGate> {
    static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<RepoGate>>>> = OnceLock::new();
    let root = sikemux_process::files::real_path(repo).unwrap_or_else(|_| repo.to_path_buf());
    let mut gates = GATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    gates.entry(root).or_default().clone()
}

/// Waits for `repo`'s merge-queue checkout, which one check uses at a time. A check of
/// commits bound for the remote goes before any check that only lands on a
/// local branch, such as a teammate's land into its lead's checkout.
pub async fn checkout_turn(repo: &Path, to_remote: bool) -> tokio::sync::OwnedMutexGuard<()> {
    let gate = gate_for(repo);
    if to_remote {
        gate.pushing.fetch_add(1, Ordering::SeqCst);
        let _pushing = PushingGuard { gate: gate.clone() };
        return gate.checkout.clone().lock_owned().await;
    }
    loop {
        let pushed = gate.pushed.notified();
        tokio::pin!(pushed);
        pushed.as_mut().enable();
        if gate.pushing.load(Ordering::SeqCst) > 0 {
            pushed.await;
            continue;
        }
        let turn = gate.checkout.clone().lock_owned().await;
        if gate.pushing.load(Ordering::SeqCst) == 0 {
            return turn;
        }
    }
}

/// What the land queue keeps about one base while the core runs.
#[derive(Default)]
pub struct Lane {
    pub running: bool,
    /// A check of several failed, so each goes by itself until one lands.
    pub one_at_a_time: bool,
}

impl Lane {
    /// How many of `waiting` lands, from the front, the next check takes together.
    pub fn take(&self, waiting: usize) -> usize {
        match self.one_at_a_time {
            true => waiting.min(1),
            false => waiting.min(MAX_TOGETHER),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn different_repos_do_not_block_each_other_while_same_repo_serializes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_a = dir.path().join("repo-a");
        let repo_b = dir.path().join("repo-b");
        std::fs::create_dir_all(&repo_a).expect("repo a");
        std::fs::create_dir_all(&repo_b).expect("repo b");

        let hold_a = checkout_turn(&repo_a, false).await;

        let hold_b =
            tokio::time::timeout(Duration::from_millis(200), checkout_turn(&repo_b, false))
                .await
                .expect("repo B should not be blocked by repo A");
        drop(hold_b);

        let same_via_dot = repo_a.join(".");
        let blocked = tokio::time::timeout(
            Duration::from_millis(50),
            checkout_turn(&same_via_dot, false),
        )
        .await;
        assert!(
            blocked.is_err(),
            "canonicalized same repo must still serialize"
        );

        drop(hold_a);
        let _acquired = tokio::time::timeout(
            Duration::from_millis(200),
            checkout_turn(&same_via_dot, false),
        )
        .await
        .expect("repo A acquires once released");
    }

    #[tokio::test]
    async fn cancelling_remote_waiter_does_not_deadlock_local_lands() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo = dir.path().join("repo-cancel");
        std::fs::create_dir_all(&repo).expect("repo");

        let hold = checkout_turn(&repo, false).await;

        let remote_repo = repo.clone();
        let remote_waiter = tokio::spawn(async move {
            let _turn = checkout_turn(&remote_repo, true).await;
        });

        let gate = gate_for(&repo);
        tokio::time::timeout(Duration::from_millis(200), async {
            while gate.pushing.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("remote waiter registered");

        remote_waiter.abort();
        let _ = remote_waiter.await;
        assert_eq!(gate.pushing.load(Ordering::SeqCst), 0);

        drop(hold);
        let _local = tokio::time::timeout(Duration::from_millis(200), checkout_turn(&repo, false))
            .await
            .expect("local land should proceed after cancelled remote waiter");
    }
}
