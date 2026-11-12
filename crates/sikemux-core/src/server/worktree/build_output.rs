//! Removing the Cargo output of agent worktrees nothing builds in any more. A worktree's
//! `target` starts as a copy-on-write clone of the main checkout's and drifts from it with each
//! build, and that disk comes back only when the whole folder goes. So it goes whole: when the
//! worktree settles, as its agent closes or has landed everything and ends its turn, and, least
//! recently built first, while the disk is nearly full. The main checkout's is never touched,
//! nor one Cargo is building in. A worktree that settles also runs the project's
//! `worktree.onSettle`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::changed::Topic;
use sikemux_view::host_stats::{ClearedFor, HostEventKind};

use super::removal::busy;
use super::warm::{hold_builds, ignored_folders, is_cargo_output, seed_in_background};
use super::{worktrees_folder, AgentWorktree};
use crate::git::Git;
use crate::server::changed::What;
use crate::server::host_stats::{disk_has_room, lowest_priority};
use crate::server::Core;

/// How long after one pass at making room the next may start, while the disk stays full.
const MAKE_ROOM_EVERY: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct BuildOutputs {
    /// Agents that landed during their turn.
    landed: Mutex<HashSet<String>>,
    /// Agents whose worktree lost its build output when they landed, which get it back warm
    /// if they work again.
    cleared: Mutex<HashSet<String>>,
    /// When the next pass at making room may start, in Unix milliseconds; `u64::MAX` while one runs.
    next_room_at: AtomicU64,
}

impl BuildOutputs {
    pub(crate) fn landed(&self, agent_id: &str) {
        self.landed.locked().insert(agent_id.to_owned());
    }
}

/// A turn starts: an agent whose build output went when it landed gets the main checkout's
/// cloned back, which takes no disk until it builds.
pub(crate) fn turn_started(core: &Core, agent_id: &str) {
    if !core.build_outputs.cleared.locked().remove(agent_id) {
        return;
    }
    if let Some(tree) = core
        .workspaces
        .agent(agent_id)
        .and_then(|agent| agent.worktree)
    {
        seed_in_background(&tree.repo, &tree.path);
    }
}

/// A turn ends: when the agent landed in it and its worktree holds nothing left to land, the
/// worktree's build output goes.
pub(crate) fn turn_ended(core: &Core, agent_id: &str) {
    if !core.build_outputs.landed.locked().remove(agent_id) {
        return;
    }
    let Some(tree) = core
        .workspaces
        .agent(agent_id)
        .and_then(|agent| agent.worktree)
    else {
        return;
    };
    if used_by_others(core, &tree.path, agent_id) {
        return;
    }
    let agent_id = agent_id.to_owned();
    let runtime = tokio::runtime::Handle::try_current().ok();
    in_background(core, move |core| {
        if !all_landed(&tree) {
            return;
        }
        if clear(core, &tree.path, ClearedFor::Landed) {
            core.build_outputs.cleared.locked().insert(agent_id);
        }
        settle(core, runtime, vec![tree]);
    });
}

/// Agents closed: the build output of the worktrees they leave that no open agent works in goes.
pub(crate) fn closed(core: &Core, trees: Vec<AgentWorktree>) {
    let open: Vec<PathBuf> = (core.workspaces.agent_ids().into_iter())
        .filter_map(|id| core.workspaces.agent(&id))
        .map(|agent| agent.cwd)
        .collect();
    let mut left: Vec<AgentWorktree> = trees
        .into_iter()
        .filter(|tree| !open.iter().any(|cwd| cwd.starts_with(&tree.path)))
        .collect();
    left.sort_by(|a, b| a.path.cmp(&b.path));
    left.dedup_by(|a, b| a.path == b.path);
    if left.is_empty() {
        return;
    }
    let runtime = tokio::runtime::Handle::try_current().ok();
    in_background(core, move |core| {
        for tree in &left {
            clear(core, &tree.path, ClearedFor::Closed);
        }
        settle(core, runtime, left);
    });
}

/// Runs the project's `worktree.onSettle` in each of `trees`, one tree after another.
fn settle(core: &Core, runtime: Option<tokio::runtime::Handle>, trees: Vec<AgentWorktree>) {
    let (Some(runtime), Some(core)) = (runtime, core.arc()) else {
        return;
    };
    runtime.spawn(async move {
        for tree in trees {
            super::agent::settle(core.clone(), tree.repo, tree.path).await;
        }
    });
}

/// The disk is nearly full: removes the build output of agent worktrees no agent is at work
/// in, least recently built first, until there is room again.
pub(crate) fn make_room(core: &Core) {
    let now = epoch_ms();
    let next = &core.build_outputs.next_room_at;
    let due = next.load(Ordering::Acquire);
    if now < due
        || next
            .compare_exchange(due, u64::MAX, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    let projects = core.workspaces.project_paths();
    let working: Vec<PathBuf> = (core.workspaces.agent_ids().into_iter())
        .filter(|id| busy(core, id))
        .filter_map(|id| core.workspaces.agent(&id))
        .map(|agent| agent.cwd)
        .collect();
    in_background(core, move |core| {
        let mut trees: Vec<(SystemTime, PathBuf)> = worktrees_of(&projects)
            .into_iter()
            .filter(|tree| !working.iter().any(|cwd| cwd.starts_with(tree)))
            .filter(|tree| !projects.iter().any(|project| project.starts_with(tree)))
            .filter_map(|tree| Some((last_built(&tree)?, tree)))
            .collect();
        trees.sort();
        for (_, tree) in trees {
            if disk_has_room() {
                break;
            }
            clear(core, &tree, ClearedFor::DiskFull);
        }
        let after = epoch_ms() + MAKE_ROOM_EVERY.as_millis() as u64;
        core.build_outputs
            .next_room_at
            .store(after, Ordering::Release);
    });
}

fn in_background(core: &Core, work: impl FnOnce(&Core) + Send + 'static) {
    let Some(core) = core.arc() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("sikemux-build-output".into())
        .spawn(move || {
            lowest_priority();
            work(&core);
        });
}

fn used_by_others(core: &Core, tree: &Path, agent_id: &str) -> bool {
    (core.workspaces.agent_ids().into_iter())
        .filter(|id| id != agent_id)
        .filter_map(|id| core.workspaces.agent(&id))
        .any(|other| other.cwd.starts_with(tree))
}

/// Whether the worktree has no changes left uncommitted and its base branch holds every commit.
fn all_landed(tree: &AgentWorktree) -> bool {
    let Some(base) = tree.base.as_deref() else {
        return false;
    };
    let clean = Git::quick(&tree.path)
        .trimmed(&["status", "--porcelain"])
        .is_ok_and(|status| status.is_empty());
    clean
        && Git::quick(&tree.path)
            .trimmed(&["rev-parse", "HEAD"])
            .is_ok_and(|head| {
                Git::quick(&tree.repo)
                    .trimmed(&["merge-base", "--is-ancestor", &head, base])
                    .is_ok()
            })
}

/// Every agent worktree in the worktrees folders of `projects`, leaving out the core's own
/// checkouts there, such as `.integration`, whose builds the next check would only redo.
fn worktrees_of(projects: &[PathBuf]) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = projects.iter().map(|path| worktrees_folder(path)).collect();
    folders.sort();
    folders.dedup();
    folders
        .iter()
        .flat_map(|folder| std::fs::read_dir(folder).into_iter().flatten().flatten())
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .filter(|tree| tree.join(".git").is_file())
        .collect()
}

/// The folders in `tree` that Cargo builds in.
fn outputs(tree: &Path) -> Vec<PathBuf> {
    (ignored_folders(tree).unwrap_or_default().into_iter())
        .map(|folder| tree.join(folder))
        .filter(|folder| is_cargo_output(folder))
        .collect()
}

/// When Cargo last wrote to the worktree's build output, which every build does to the folders
/// its results are in.
fn last_built(tree: &Path) -> Option<SystemTime> {
    let modified = |folder: &Path| folder.metadata().and_then(|meta| meta.modified()).ok();
    let subfolders = |folder: &Path| -> Vec<PathBuf> {
        (std::fs::read_dir(folder).into_iter().flatten().flatten())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect()
    };
    outputs(tree)
        .into_iter()
        .flat_map(|output| {
            let profiles = subfolders(&output);
            let inner: Vec<PathBuf> = profiles.iter().flat_map(|dir| subfolders(dir)).collect();
            std::iter::once(output).chain(profiles).chain(inner)
        })
        .filter_map(|folder| modified(&folder))
        .max()
}

/// Removes the build output in the worktree at `tree` and says so. Answers whether any went.
fn clear(core: &Core, tree: &Path, reason: ClearedFor) -> bool {
    let Some(freed) = remove_outputs(tree) else {
        return false;
    };
    let worktree = tree.file_name().map_or_else(
        || tree.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let why = match reason {
        ClearedFor::Closed => "its agent closed",
        ClearedFor::Landed => "its agent landed everything",
        ClearedFor::DiskFull => "the disk was nearly full",
    };
    eprintln!(
        "sikemux core: removed the build output of the worktree {}, which freed {} MB, as {why}",
        tree.display(),
        freed / (1024 * 1024)
    );
    core.host_watch.note(HostEventKind::BuildOutputCleared {
        worktree,
        bytes: freed,
        reason,
    });
    core.changed(What::Read(Topic::HostStats));
    true
}

/// Removes the build output in the worktree at `tree` that Cargo is not building in, and answers
/// how much disk came back, or `None` when none went. A copy-on-write clone gives back only what
/// it no longer shares, so the volume's free space is measured rather than the folder's size. A
/// main checkout has a `.git` folder, not a file, so it is left.
fn remove_outputs(tree: &Path) -> Option<u64> {
    if !tree.join(".git").is_file() {
        return None;
    }
    let before = free_space(tree);
    let mut removed = false;
    for output in outputs(tree) {
        let Some(_held) = hold_builds(&output) else {
            continue;
        };
        if let Err(error) = std::fs::remove_dir_all(&output) {
            eprintln!(
                "sikemux core: {} was not all removed: {error}",
                output.display()
            );
        }
        removed = true;
    }
    removed.then(|| free_space(tree).saturating_sub(before))
}

fn free_space(path: &Path) -> u64 {
    sikemux_process::machine::disk(path).map_or(0, |disk| disk.total.saturating_sub(disk.used))
}
