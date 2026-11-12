//! Removing the Cargo output of agent worktrees nothing builds in any more. A worktree's
//! `target` starts as a copy-on-write clone of the main checkout's and drifts from it with each
//! build, and that disk comes back only when the whole folder goes. So it goes whole: once the
//! worktree's agent has closed, once its agent has landed everything and gone to sleep, and,
//! least recently built first, while free space is low. The main checkout's is never touched,
//! nor a worktree's while an agent is at work in it or any process runs from it. A worktree that
//! settles, as its agent closes or has landed everything and gone to sleep, also runs the
//! project's `worktree.onSettle`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use sikemux_process::clock::epoch_ms;
use sikemux_process::machine::{self, Disk};
use sikemux_process::procs::{self, Place};
use sikemux_process::sync::Locked;
use sikemux_view::changed::Topic;
use sikemux_view::host_stats::{ClearedFor, HostEventKind};

use super::removal::busy;
use super::warm::{hold_builds, ignored_folders, is_cargo_output};
use super::{worktrees_folder, AgentWorktree};
use crate::git::Git;
use crate::server::changed::What;
use crate::server::host_stats::lowest_priority;
use crate::server::Core;

/// How long after one pass at making room the next may start, while free space stays low.
const MAKE_ROOM_EVERY: Duration = Duration::from_secs(60);
const GB: u64 = 1_000_000_000;
/// A pass at making room starts under the lesser of this and a tenth of the disk free.
const FREE_FLOOR: u64 = 15 * GB;
/// A pass stops at the lesser of this and 15% of the disk free, far enough above where it
/// starts that the next builds do not start another at once.
const FREE_ENOUGH: u64 = 25 * GB;
/// How long to wait before each look at the worktree of an agent that closed or slept, while
/// its own processes are still ending: the looks fall at 0, 3, 13 and 43 s. The last that finds
/// something running from it keeps it.
const LOOKS_AFTER_STOPPING: [Duration; 4] = [
    Duration::ZERO,
    Duration::from_secs(3),
    Duration::from_secs(10),
    Duration::from_secs(30),
];
/// What a build output is renamed to, in the worktrees folder, while it is deleted, so a build
/// that starts meanwhile makes a fresh one. One left by a core that stopped mid-delete goes at
/// the next start.
const SET_ASIDE: &str = ".build-output-removing-";

#[derive(Default)]
pub(crate) struct BuildOutputs {
    /// Agents that landed since they last slept or closed.
    landed: Mutex<HashSet<String>>,
    /// When the next pass at making room may start, in Unix milliseconds; `u64::MAX` while one runs.
    next_room_at: AtomicU64,
    /// Worktrees whose build output was kept because something ran from them, said once each.
    spared: Mutex<HashSet<PathBuf>>,
    /// Free space is under the floor and the last pass could remove nothing, which is said once
    /// until free space is back above the floor or a pass removes something.
    stuck: AtomicBool,
}

impl BuildOutputs {
    pub(crate) fn landed(&self, agent_id: &str) {
        self.landed.locked().insert(agent_id.to_owned());
    }
}

enum Outcome {
    Removed,
    Kept(Keeper),
    Nothing,
}

/// What keeps a worktree's build output.
enum Keeper {
    Agent(String),
    Process(String),
    Unsure(String),
}

impl Keeper {
    fn words(&self) -> &str {
        match self {
            Keeper::Agent(words) | Keeper::Process(words) | Keeper::Unsure(words) => words,
        }
    }
}

/// The agent went to sleep: when it landed while awake and its worktree holds nothing left to
/// land, the worktree's build output goes once its processes have ended. Waking it clones the
/// main checkout's back, warm.
pub(crate) fn slept(core: &Core, agent_id: &str) {
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
    let runtime = tokio::runtime::Handle::try_current().ok();
    in_background(core, move |core| {
        if all_landed(&tree) {
            clear_once_stopped(core, vec![tree.path.clone()], ClearedFor::Landed);
            settle(core, runtime, vec![tree]);
        }
    });
}

/// Agents closed: the build output of the worktrees they leave that no open agent works in goes
/// once their processes have ended.
pub(crate) fn closed(core: &Core, agent_ids: &[String], trees: Vec<AgentWorktree>) {
    {
        let mut landed = core.build_outputs.landed.locked();
        for agent_id in agent_ids {
            landed.remove(agent_id);
        }
    }
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
        let paths = left.iter().map(|tree| tree.path.clone()).collect();
        clear_once_stopped(core, paths, ClearedFor::Closed);
        settle(core, runtime, left);
    });
}

fn used_by_others(core: &Core, tree: &Path, agent_id: &str) -> bool {
    (core.workspaces.agent_ids().into_iter())
        .filter(|id| id != agent_id)
        .filter_map(|id| core.workspaces.agent(&id))
        .any(|other| other.cwd.starts_with(tree))
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

fn clear_once_stopped(core: &Core, mut trees: Vec<PathBuf>, reason: ClearedFor) {
    for (look, wait) in LOOKS_AFTER_STOPPING.iter().enumerate() {
        std::thread::sleep(*wait);
        let last = look + 1 == LOOKS_AFTER_STOPPING.len();
        trees.retain(|tree| matches!(clear(core, tree, reason, last), Outcome::Kept(_)));
        if trees.is_empty() {
            return;
        }
    }
}

/// With `free` of `total` bytes left under the floor, removes the build output of agent
/// worktrees on the home disk that no awake agent works in, least recently built first, until
/// enough is free again.
pub(crate) fn make_room(core: &Core, free: u64, total: u64) {
    if free >= FREE_FLOOR.min(total / 10) {
        core.build_outputs.stuck.store(false, Ordering::Release);
        return;
    }
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
    let started = in_background(core, |core| {
        let _next = NextPass(core);
        pass_at_making_room(core);
    });
    if !started {
        next.store(now + MAKE_ROOM_EVERY.as_millis() as u64, Ordering::Release);
    }
}

/// Sets when the next pass may start as the one running ends, however it ends.
struct NextPass<'a>(&'a Core);

impl Drop for NextPass<'_> {
    fn drop(&mut self) {
        let after = epoch_ms() + MAKE_ROOM_EVERY.as_millis() as u64;
        self.0
            .build_outputs
            .next_room_at
            .store(after, Ordering::Release);
    }
}

fn pass_at_making_room(core: &Core) {
    let Some(home) = sikemux_process::paths::home() else {
        return;
    };
    let projects = core.workspaces.project_paths();
    let awake: Vec<PathBuf> = (core.workspaces.agent_ids().into_iter())
        .filter_map(|id| core.workspaces.agent(&id))
        .filter(|agent| !agent.asleep)
        .map(|agent| agent.cwd)
        .collect();
    let (held, free): (Vec<PathBuf>, Vec<PathBuf>) = worktrees_of(&projects)
        .into_iter()
        .filter(|tree| !projects.iter().any(|project| project.starts_with(tree)))
        .filter(|tree| same_disk(tree, &home))
        .partition(|tree| awake.iter().any(|cwd| cwd.starts_with(tree)));
    let mut trees: Vec<(SystemTime, PathBuf)> = (free.into_iter())
        .filter_map(|tree| Some((last_built(&tree)?, tree)))
        .collect();
    trees.sort();
    let (mut by_agents, mut by_processes, mut unsure) = (held.len(), 0, 0);
    let mut removed_any = false;
    for (_, tree) in trees {
        let Some(disk) = machine::disk(&home) else {
            return;
        };
        if disk.free() >= FREE_ENOUGH.min(disk.total * 15 / 100) {
            break;
        }
        match clear(core, &tree, ClearedFor::LowSpace, true) {
            Outcome::Removed => removed_any = true,
            Outcome::Kept(Keeper::Agent(_)) => by_agents += 1,
            Outcome::Kept(Keeper::Process(_)) => by_processes += 1,
            Outcome::Kept(Keeper::Unsure(_)) => unsure += 1,
            Outcome::Nothing => {}
        }
    }
    if removed_any {
        core.build_outputs.stuck.store(false, Ordering::Release);
    } else if !core.build_outputs.stuck.swap(true, Ordering::AcqRel) {
        eprintln!(
            "sikemux core: free space is low and no worktree's build output could go: {by_agents} kept for an agent awake or at work in it, {by_processes} for a process running from it, {unsure} as what runs there could not be told"
        );
    }
}

/// Deletes the build outputs a core that stopped mid-delete left set aside.
pub(crate) fn remove_set_aside(core: &Core) {
    let mut folders: Vec<PathBuf> = (core.workspaces.project_paths().iter())
        .map(|path| worktrees_folder(path))
        .collect();
    folders.sort();
    folders.dedup();
    in_background(core, move |_| {
        let left = folders
            .iter()
            .flat_map(|folder| std::fs::read_dir(folder).into_iter().flatten().flatten())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(SET_ASIDE));
        for entry in left {
            delete(&entry.path());
        }
    });
}

#[cfg(unix)]
fn same_disk(one: &Path, other: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    matches!((one.metadata(), other.metadata()), (Ok(one), Ok(other)) if one.dev() == other.dev())
}

#[cfg(not(unix))]
fn same_disk(_one: &Path, _other: &Path) -> bool {
    false
}

/// Runs `work` on a thread of its own at the lowest priority. Answers whether it started.
fn in_background(core: &Core, work: impl FnOnce(&Core) + Send + 'static) -> bool {
    let Some(core) = core.arc() else {
        return false;
    };
    std::thread::Builder::new()
        .name("sikemux-build-output".into())
        .spawn(move || {
            lowest_priority();
            work(&core);
        })
        .is_ok()
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

/// What keeps the worktree at `tree` as it is now: an agent awake or at work in it, or a process
/// that runs from it. Not being able to tell keeps it too.
fn kept_by(core: &Core, tree: &Path) -> Option<Keeper> {
    let agent = (core.workspaces.agent_ids().into_iter())
        .filter_map(|id| core.workspaces.agent(&id))
        .find(|agent| agent.cwd.starts_with(tree) && (!agent.asleep || busy(core, &agent.id)));
    if let Some(agent) = agent {
        let name = core.names.name(&agent.id).unwrap_or(agent.id);
        return Some(Keeper::Agent(format!("@{name} is awake in it")));
    }
    let Ok(real) = tree.canonicalize() else {
        return Some(Keeper::Unsure("its folder could not be read".to_owned()));
    };
    let Some(places) = procs::places() else {
        return Some(Keeper::Unsure(
            "this system cannot say what runs from it".to_owned(),
        ));
    };
    running_in(&places, &[tree, &real]).map(|place| {
        let name = procs::name(place.pid).unwrap_or_else(|| "a process".to_owned());
        Keeper::Process(format!(
            "{name} ({}) runs from {}",
            place.pid,
            place.path.display()
        ))
    })
}

/// The first of `places` in the worktree, by either spelling of its path: the system names
/// programs and working folders by their real paths, while arguments keep the one typed.
fn running_in<'a>(places: &'a [Place], spellings: &[&Path]) -> Option<&'a Place> {
    (places.iter()).find(|place| spellings.iter().any(|tree| place.path.starts_with(tree)))
}

/// Removes the build output in the worktree at `tree` unless something keeps it, and says so;
/// a kept one only when `say_kept`, once for each worktree.
/// A main checkout has a `.git` folder, not a file, so it is left.
fn clear(core: &Core, tree: &Path, reason: ClearedFor, say_kept: bool) -> Outcome {
    if !tree.join(".git").is_file() {
        return Outcome::Nothing;
    }
    let outputs = outputs(tree);
    if outputs.is_empty() {
        return Outcome::Nothing;
    }
    if let Some(keeper) = kept_by(core, tree) {
        let mut spared = core.build_outputs.spared.locked();
        spared.retain(|path| path.exists());
        if say_kept && spared.insert(tree.to_path_buf()) {
            eprintln!(
                "sikemux core: kept the build output of the worktree {}, as {}",
                tree.display(),
                keeper.words()
            );
        }
        return Outcome::Kept(keeper);
    }
    core.build_outputs.spared.locked().remove(tree);
    let Some(freed) = remove_outputs(tree, outputs) else {
        return Outcome::Nothing;
    };
    let worktree = tree.file_name().map_or_else(
        || tree.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let why = match reason {
        ClearedFor::Closed => ", as its agent closed",
        ClearedFor::Landed => ", as its agent landed everything",
        ClearedFor::LowSpace => ", as free space ran low",
        ClearedFor::Unknown => "",
    };
    eprintln!(
        "sikemux core: removed the build output of the worktree {}, which freed {} MB{why}",
        tree.display(),
        freed / 1_000_000
    );
    core.host_watch.note(HostEventKind::BuildOutputCleared {
        worktree,
        bytes: freed,
        reason,
    });
    core.changed(What::Read(Topic::HostStats));
    Outcome::Removed
}

/// Removes the `outputs` of the worktree at `tree` that Cargo is not building in, and answers how
/// much disk came back, or `None` when none went. A copy-on-write clone gives back only what it
/// no longer shares, so the volume's free space is measured rather than the folder's size.
fn remove_outputs(tree: &Path, outputs: Vec<PathBuf>) -> Option<u64> {
    let free = || machine::disk(tree).as_ref().map_or(0, Disk::free);
    let before = free();
    let mut removed = false;
    for (index, output) in outputs.into_iter().enumerate() {
        let set_aside = {
            let Some(_held) = hold_builds(&output) else {
                continue;
            };
            if !is_cargo_output(&output) {
                continue;
            }
            match set_aside(tree, &output, index) {
                Ok(to) => to,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    delete(&output);
                    removed = true;
                    continue;
                }
            }
        };
        delete(&set_aside);
        removed = true;
    }
    removed.then(|| free().saturating_sub(before))
}

fn delete(folder: &Path) {
    if let Err(error) = std::fs::remove_dir_all(folder) {
        eprintln!(
            "sikemux core: {} was not all removed: {error}",
            folder.display()
        );
    }
}

/// Moves `output` out of the worktree at `tree`, beside it in the worktrees folder, answering
/// where it went.
fn set_aside(tree: &Path, output: &Path, index: usize) -> std::io::Result<PathBuf> {
    let (Some(name), Some(folder)) = (tree.file_name(), tree.parent()) else {
        return Err(std::io::ErrorKind::InvalidInput.into());
    };
    let to = folder.join(format!(
        "{SET_ASIDE}{}-{}-{index}",
        name.to_string_lossy(),
        epoch_ms()
    ));
    std::fs::rename(output, &to).map(|()| to)
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use sikemux_process::user_environment::command;
    use sikemux_wire::protocol::BuildIdentity;
    use std::process::Child;

    const STAND_IN: &str = "SIKEMUX_TEST_STAND_IN";

    fn git(dir: &Path, args: &[&str]) {
        Git::quick(dir).trimmed(args).unwrap();
    }

    fn removed_while(core: &Core, tree: &Path, mut child: Child) -> bool {
        let removed = matches!(
            clear(core, tree, ClearedFor::LowSpace, false),
            Outcome::Removed
        );
        child.kill().unwrap();
        child.wait().unwrap();
        removed
    }

    /// What runs from `target` in the test below: this test binary, copied there.
    #[test]
    #[ignore]
    fn stands_in_for_a_program_in_target() {
        if std::env::var_os(STAND_IN).is_some() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    /// A tools eval ran `target/debug/sikemux-host` from a worktree whose agent had ended its
    /// turn, and making room took the whole `target` from under it.
    #[test]
    fn a_process_running_from_an_idle_worktree_keeps_its_build_output() {
        let parent = tempfile::tempdir().unwrap();
        let main = parent.path().join("app");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]);
        std::fs::write(main.join(".gitignore"), "target/\n").unwrap();
        git(&main, &["add", "."]);
        git(
            &main,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "base",
            ],
        );
        let tree = parent.path().join("app.worktrees/feat-eval");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/eval",
                tree.to_str().unwrap(),
            ],
        );
        let debug = tree.join("target/debug");
        std::fs::create_dir_all(&debug).unwrap();
        std::fs::write(tree.join("target/.rustc_info.json"), "{}").unwrap();
        let host = debug.join("sikemux-host");
        std::fs::copy(std::env::current_exe().unwrap(), &host).unwrap();
        let script = tree.join("run.sh");
        std::fs::write(&script, "read line\n").unwrap();
        let core = Core::new(BuildIdentity::default(), None).unwrap();

        let from_target = command(&host)
            .args(["--ignored", "--exact", "--quiet"])
            .arg("server::worktree::build_output::tests::stands_in_for_a_program_in_target")
            .env(STAND_IN, "1")
            .current_dir(parent.path())
            .spawn()
            .unwrap();
        let by_program = removed_while(&core, &tree, from_target);
        let in_folder = command("sleep")
            .arg("30")
            .current_dir(&tree)
            .spawn()
            .unwrap();
        let by_folder = removed_while(&core, &tree, in_folder);
        let from_script = command("sh")
            .arg(&script)
            .stdin(std::process::Stdio::piped())
            .current_dir(parent.path())
            .spawn()
            .unwrap();
        let by_argument = removed_while(&core, &tree, from_script);
        let with_nothing = matches!(
            clear(&core, &tree, ClearedFor::LowSpace, false),
            Outcome::Removed
        );

        assert!(!by_program, "removed while its program ran from target");
        assert!(!by_folder, "removed while a process worked in it");
        assert!(!by_argument, "removed while a process ran a script in it");
        assert!(with_nothing);
        assert!(!tree.join("target").exists());
        assert!(std::fs::read_dir(tree.parent().unwrap())
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(SET_ASIDE)));
    }
}
