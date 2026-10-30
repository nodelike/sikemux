//! Changes no edit tool reported: a command an agent ran, an agent in a
//! terminal, or the person. While agents work in a checkout its files are
//! watched, and each change goes in the edit record as its writer's when one
//! agent was at work, shared among them when several were, and as the
//! person's when none was. A file's lines are told against what it held
//! after its last change, kept in [`Priors`].

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sikemux_process::clock::{epoch_ms, epoch_ms_of};
use sikemux_process::sync::Locked;

use super::prior::{text_of, Before, Priors, MAX_FILE_BYTES};
use super::{changed, line_count, owners::run, Edit, Store};
use crate::protocol::AgentStatus;
use crate::server::collisions::{Written, GRACE_MS};
use crate::server::messages::{listed, Listed};
use crate::server::wake::sooner;
use crate::server::Core;

/// Changes this close behind one whose grace has passed are looked at with it.
const TOGETHER_MS: u64 = 500;
/// A checkout no agent worked in for this long is no longer watched.
const IDLE_MS: u64 = 10 * 60_000;
const MAX_CHECKOUTS: usize = 16;
/// Changed paths that wait to be looked at in one checkout; more are let go.
const MAX_WAITING: usize = 4096;
const MAX_IGNORED: usize = 1024;
/// Changes at once past which folders git ignores are looked for first, as a
/// build fills one.
const MANY: usize = 64;
/// Changes looked at together, whose files' last commit copies are held at
/// once.
const AT_ONCE: usize = 256;
const MAX_AGENTS: usize = 256;

#[derive(Default)]
pub(super) struct Outside {
    checkouts: Mutex<HashMap<PathBuf, Checkout>>,
    /// The checkout each agent works in, as last found.
    tops: Mutex<HashMap<String, Option<PathBuf>>>,
}

struct Checkout {
    _watcher: RecommendedWatcher,
    project: String,
    seen: Arc<Mutex<Seen>>,
    worked_at: u64,
}

#[derive(Default)]
struct Seen {
    /// Each changed path, with when it first changed.
    changed: HashMap<PathBuf, Written>,
    /// When the checkout's commit or branch first moved.
    committed: Option<u64>,
    /// When more files changed than could wait to be looked at.
    overflowed: Option<u64>,
    /// When a shell command began, so the changes seen up to then are looked
    /// at without waiting out their grace, before the command's own join them.
    command_began: Option<u64>,
    /// What git ignores: whole folders, or single files.
    ignored: Vec<PathBuf>,
}

impl Seen {
    fn idle(&self) -> bool {
        self.changed.is_empty() && self.committed.is_none() && self.overflowed.is_none()
    }

    fn note(&mut self, top: &Path, path: PathBuf, now: u64) {
        let Ok(inside) = path.strip_prefix(top) else {
            return;
        };
        let mut parts = inside.components().map(|part| part.as_os_str());
        if parts.next().is_some_and(|first| first == ".git") {
            if parts
                .next()
                .is_some_and(|part| part == "HEAD" || part == "refs" || part == "packed-refs")
            {
                self.committed.get_or_insert(now);
            }
            return;
        }
        if self.ignored.iter().any(|ignored| path.starts_with(ignored)) {
            return;
        }
        if self.changed.len() < MAX_WAITING || self.changed.contains_key(&path) {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.changed.entry(path) {
                let changed = Written::of(entry.key(), now);
                entry.insert(changed);
            }
        } else {
            self.overflowed.get_or_insert(now);
        }
    }

    fn ignore(&mut self, path: PathBuf) {
        if self.ignored.len() < MAX_IGNORED && !self.ignored.contains(&path) {
            self.ignored.push(path);
        }
    }
}

/// Who made a change no edit tool reported.
enum Writer {
    Agent(String),
    Shared(Vec<String>),
    Person,
}

impl Outside {
    /// `agent_id` is at work since `began`, running a shell command from then
    /// when `command`. Its checkout is watched, if it was not, and what
    /// changed in it since `began` is the work's, as the agent may have
    /// changed files before the watch could start.
    pub(super) fn working(
        &self,
        core: &Arc<Core>,
        store: &Store,
        agent_id: &str,
        began: u64,
        command: bool,
    ) {
        let Some(top) = self.top_of(core, agent_id) else {
            return;
        };
        let now = epoch_ms();
        if let Some(checkout) = self.checkouts.locked().get_mut(&top) {
            checkout.worked_at = now;
            if command {
                let mut seen = checkout.seen.locked();
                seen.command_began = seen.command_began.max(Some(began));
            }
            return;
        }
        let seen = Arc::new(Mutex::new(Seen::default()));
        let since = UNIX_EPOCH + Duration::from_millis(began.min(now));
        let Some(watcher) = watch(&top, seen.clone(), Arc::downgrade(core)) else {
            return;
        };
        let project = core.workspaces.project_of(&top);
        reconcile(store, &top, &project, &seen, Look::Whole { since });
        let mut checkouts = self.checkouts.locked();
        if checkouts.len() >= MAX_CHECKOUTS {
            let oldest = checkouts
                .iter()
                .min_by_key(|(_, checkout)| checkout.worked_at)
                .map(|(top, _)| top.clone());
            if let Some(oldest) = oldest {
                checkouts.remove(&oldest);
            }
        }
        checkouts.insert(
            top,
            Checkout {
                _watcher: watcher,
                project,
                seen,
                worked_at: now,
            },
        );
    }

    /// The top folder of the checkout `agent_id` works in, none outside one.
    pub(super) fn top_of(&self, core: &Core, agent_id: &str) -> Option<PathBuf> {
        if let Some(top) = self.tops.locked().get(agent_id) {
            return top.clone();
        }
        let cwd = match core.chats.get(agent_id) {
            Some(chat) => Some(chat.launch.cwd.clone()),
            None => core
                .all_sessions()
                .iter()
                .find(|session| session.owner().agent_id.as_deref() == Some(agent_id))
                .and_then(|session| session.owner().project.clone())
                .map(PathBuf::from),
        };
        let top = cwd
            .and_then(|cwd| super::git(&cwd, &["rev-parse", "--show-toplevel"]).ok())
            .map(|top| PathBuf::from(top.trim()))
            .filter(|top| top.is_absolute())
            .map(|top| std::fs::canonicalize(&top).unwrap_or(top));
        let mut tops = self.tops.locked();
        if tops.len() >= MAX_AGENTS {
            tops.clear();
        }
        tops.insert(agent_id.to_owned(), top.clone());
        top
    }

    /// The watched checkout `path` is in, by its top folder, with its project.
    pub(super) fn checkout_of(&self, path: &Path) -> Option<(PathBuf, String)> {
        self.checkouts
            .locked()
            .iter()
            .filter(|(top, _)| path.starts_with(top))
            .max_by_key(|(top, _)| top.as_os_str().len())
            .map(|(top, checkout)| (top.clone(), checkout.project.clone()))
    }

    /// Looks at the changes whose grace has passed, lets go of what was
    /// committed, and stops watching checkouts no agent works in, including
    /// one between commands of a long turn. A checkout no longer watched lets
    /// go of what it kept of files since committed, which a linked worktree's
    /// watch cannot see happen, and one taken away lets go of all of it. When
    /// it wants to run next.
    pub(super) fn settle(&self, core: &Arc<Core>, store: &Store) -> Option<u64> {
        let now = epoch_ms();
        let checkouts: Vec<(PathBuf, String, Arc<Mutex<Seen>>, u64)> = self
            .checkouts
            .locked()
            .iter()
            .map(|(top, checkout)| {
                let Checkout {
                    project,
                    seen,
                    worked_at,
                    ..
                } = checkout;
                (top.clone(), project.clone(), seen.clone(), *worked_at)
            })
            .collect();
        let mut next = None;
        let mut agents = None;
        for (top, project, seen, worked_at) in checkouts {
            if !top.exists() {
                self.checkouts.locked().remove(&top);
                store.priors(&project, |priors| {
                    for path in priors.under(&top) {
                        priors.forget(&path);
                    }
                });
                continue;
            }
            let (due, committed, overflowed) = {
                let mut seen = seen.locked();
                let mut due = Vec::new();
                let before_command = seen.command_began.take();
                let before_command = |at: u64| before_command.is_some_and(|began| at <= began);
                let ripe = seen
                    .changed
                    .values()
                    .any(|changed| changed.at + GRACE_MS <= now || before_command(changed.at));
                seen.changed.retain(|path, changed| {
                    let at = changed.at;
                    let ready = before_command(at) || ripe && at + GRACE_MS <= now + TOGETHER_MS;
                    if ready {
                        due.push((path.clone(), *changed));
                    } else {
                        next = sooner(next, Some(at + GRACE_MS));
                    }
                    !ready
                });
                let mut moved = |at: &mut Option<u64>| {
                    let ready = at.take_if(|at| *at + GRACE_MS <= now);
                    next = sooner(next, at.map(|at| at + GRACE_MS));
                    ready
                };
                let committed = moved(&mut seen.committed).is_some();
                let overflowed = moved(&mut seen.overflowed);
                (due, committed, overflowed)
            };
            if committed {
                let kept = store.priors(&project, |priors| priors.under(&top));
                if !kept.is_empty() {
                    reconcile(store, &top, &project, &seen, Look::Kept(&kept));
                }
            }
            if !due.is_empty() {
                look(core, store, &top, &project, &seen, due, &mut agents);
            }
            if let Some(at) = overflowed {
                let since = UNIX_EPOCH + Duration::from_millis(at.saturating_sub(GRACE_MS));
                reconcile(store, &top, &project, &seen, Look::Whole { since });
                let found = seen
                    .locked()
                    .changed
                    .values()
                    .map(|changed| changed.at)
                    .min();
                next = sooner(next, found.map(|at| at + GRACE_MS));
            }
            if now < worked_at + IDLE_MS || !seen.locked().idle() {
                next = sooner(next, Some(worked_at + IDLE_MS));
                continue;
            }
            let agents = agents.get_or_insert_with(|| listed(core));
            let mid_turn = {
                let tops = self.tops.locked();
                agents.iter().any(|agent| {
                    matches!(agent.status, AgentStatus::Working | AgentStatus::Blocked)
                        && tops
                            .get(&agent.agent_id)
                            .is_some_and(|at| at.as_ref() == Some(&top))
                })
            };
            let (shells, terminals) = core
                .collisions
                .at_work(core, agents, &project, &top, now, GRACE_MS);
            if !mid_turn && shells.is_empty() && terminals.is_empty() {
                self.checkouts.locked().remove(&top);
                let kept = store.priors(&project, |priors| priors.under(&top));
                if !kept.is_empty() {
                    reconcile(store, &top, &project, &seen, Look::Kept(&kept));
                }
            } else if let Some(checkout) = self.checkouts.locked().get_mut(&top) {
                checkout.worked_at = now;
                next = sooner(next, Some(now + IDLE_MS));
            }
        }
        next
    }

    /// The person saved `path` in Sikemux's editor: the change is theirs.
    pub(super) fn saved(&self, core: &Core, store: &Store, path: &Path) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let Some((top, project)) = self.checkout_of(&path) else {
            return;
        };
        let Some(inside) = inside(&top, &path) else {
            return;
        };
        let committed = committed_copies(&top, &[inside.as_str()]);
        let edit = store.priors(&project, |priors| {
            told(priors, &path, committed.get(&inside).map(Option::as_deref)).map(|(edit, _)| edit)
        });
        if let Some(edit) = edit {
            record(
                core,
                store,
                &project,
                vec![(edit, Writer::Person)],
                &mut None,
            );
        }
    }
}

/// Starts watching every file under `top`.
fn watch(top: &Path, seen: Arc<Mutex<Seen>>, core: Weak<Core>) -> Option<RecommendedWatcher> {
    let at = top.to_path_buf();
    let started = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let now = epoch_ms();
        let woke = {
            let mut seen = seen.locked();
            let idle = seen.idle();
            for path in event.paths {
                seen.note(&at, path, now);
            }
            idle && !seen.idle()
        };
        if let (true, Some(core)) = (woke, core.upgrade()) {
            core.edits.wake();
        }
    });
    let mut watcher = match started {
        Ok(watcher) => watcher,
        Err(error) => {
            eprintln!("sikemux core: changes outside agents' edit tools go unrecorded: {error}");
            return None;
        }
    };
    watcher.watch(top, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

fn inside(top: &Path, path: &Path) -> Option<String> {
    let inside = path.strip_prefix(top).ok()?.to_str()?;
    (!inside.is_empty() && !inside.contains('\n')).then(|| inside.to_owned())
}

/// Which of a checkout's files [`reconcile`] looks at.
enum Look<'a> {
    /// All, keeping each changed since its last commit as it is now, unless
    /// it was kept so already. One changed `since` the watch began is looked
    /// at as the watch would have.
    Whole { since: SystemTime },
    /// Only these kept ones, after a commit.
    Kept(&'a [String]),
}

/// Brings what is kept of `top`'s files in line with git, letting go of each
/// that no longer differs from its last commit.
fn reconcile(store: &Store, top: &Path, project: &str, seen: &Mutex<Seen>, look: Look<'_>) {
    let mut args = vec![
        "--no-optional-locks",
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--no-renames",
    ];
    let specs: Vec<String>;
    match look {
        Look::Kept(paths) => {
            specs = paths
                .iter()
                .filter_map(|path| inside(top, Path::new(path)))
                .map(|path| format!(":(top,literal){path}"))
                .collect();
            args.push("--");
            args.extend(specs.iter().map(String::as_str));
        }
        Look::Whole { .. } => args.push("--ignored=matching"),
    }
    let Ok(status) = run(top, &args, None, None) else {
        return;
    };
    let status = String::from_utf8_lossy(&status);
    let mut changed = HashSet::new();
    {
        let mut seen = seen.locked();
        for entry in status.split('\0').filter(|entry| entry.len() > 3) {
            let (code, path) = entry.split_at(3);
            let path = top.join(path.trim_end_matches('/'));
            match code {
                "!! " => seen.ignore(path),
                _ => {
                    changed.insert(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    let recent = store.priors(project, |priors| {
        let kept = match look {
            Look::Kept(paths) => paths.to_vec(),
            Look::Whole { .. } => priors.under(top),
        };
        for path in kept {
            if !changed.contains(&path) {
                priors.forget(&path);
            }
        }
        let mut recent = Vec::new();
        let Look::Whole { since } = look else {
            return recent;
        };
        for path in changed {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if let Some(at) = meta.modified().ok().filter(|at| *at >= since) {
                let changed = Written::exact(&meta, epoch_ms()).unwrap_or(Written {
                    at: epoch_ms_of(at),
                    exact: false,
                });
                recent.push((PathBuf::from(path), changed));
                continue;
            }
            if priors.fresh(&path, &meta) {
                continue;
            }
            let text = (priors.room() && meta.len() <= MAX_FILE_BYTES)
                .then(|| text_of(Path::new(&path)))
                .flatten();
            priors.keep(
                &path,
                text.as_ref().map(|(text, meta)| (text.as_str(), meta)),
            );
        }
        recent
    });
    let mut seen = seen.locked();
    for (path, changed) in recent {
        seen.changed.entry(path).or_insert(changed);
    }
}

/// Records the changes `due` in a watched checkout once their grace passed.
/// Git is asked only of files not kept already: for its copy, and whether it
/// ignores them, learning the folders it ignores.
fn look(
    core: &Core,
    store: &Store,
    top: &Path,
    project: &str,
    seen: &Mutex<Seen>,
    due: Vec<(PathBuf, Written)>,
    agents: &mut Option<Vec<Listed>>,
) {
    let mut due: Vec<(String, PathBuf, Written)> = due
        .into_iter()
        .filter(|(path, _)| !core.collisions.editing(path))
        .filter_map(|(path, at)| Some((inside(top, &path)?, path, at)))
        .collect();
    if due.len() > MANY {
        learn_ignored(
            top,
            seen,
            due.iter().map(|(inside, ..)| inside.as_str()),
            false,
        );
        let seen = seen.locked();
        due.retain(|(_, path, _)| !seen.ignored.iter().any(|ignored| path.starts_with(ignored)));
    }
    for due in due.chunks(AT_ONCE) {
        look_at(core, store, top, project, seen, due, agents);
    }
}

/// Records `due`, a share of [`look`]'s changes small enough to hold the
/// last commit's copies of at once.
fn look_at(
    core: &Core,
    store: &Store,
    top: &Path,
    project: &str,
    seen: &Mutex<Seen>,
    due: &[(String, PathBuf, Written)],
    agents: &mut Option<Vec<Listed>>,
) {
    let asked: Vec<&str> = store.priors(project, |priors| {
        due.iter()
            .filter(|(_, path, _)| {
                !priors.knows(&path.to_string_lossy())
                    && std::fs::metadata(path)
                        .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_FILE_BYTES)
            })
            .map(|(inside, ..)| inside.as_str())
            .collect()
    });
    let committed = if asked.is_empty() {
        HashMap::new()
    } else {
        committed_copies(top, &asked)
    };
    let new: HashSet<&str> = asked
        .iter()
        .copied()
        .filter(|inside| matches!(committed.get(*inside), Some(None)))
        .collect();
    let untracked = untracked(top, &new);
    let ignored = |inside: &str| new.contains(inside) && !untracked.contains(inside);
    let ignored_files = due
        .iter()
        .filter(|(inside, path, _)| ignored(inside) && path.is_file());
    learn_ignored(
        top,
        seen,
        ignored_files.map(|(inside, ..)| inside.as_str()),
        true,
    );
    let changes: Vec<(Edit, [Written; 2])> = store.priors(project, |priors| {
        due.iter()
            .filter(|(inside, ..)| !ignored(inside))
            .filter_map(|(inside, path, first)| {
                let committed = committed.get(inside).map(Option::as_deref);
                told(priors, path, committed).map(|(edit, last)| {
                    let last = last.filter(|last| last.at >= first.at).unwrap_or(*first);
                    (edit, [*first, last])
                })
            })
            .collect()
    });
    if changes.is_empty() {
        return;
    }
    let listed = agents.get_or_insert_with(|| listed(core));
    let changes = changes
        .into_iter()
        .map(|(edit, [first, last])| {
            let place = Path::new(&edit.path);
            let mut times = vec![first];
            if last.at != first.at {
                times.push(last);
            }
            let mut at_work: Vec<String> = times
                .into_iter()
                .flat_map(|written| {
                    let (shells, terminals) = core
                        .collisions
                        .writers(core, listed, project, place, written);
                    shells.into_iter().chain(terminals)
                })
                .collect();
            at_work.sort_unstable();
            at_work.dedup();
            let writer = match at_work.len() {
                0 => Writer::Person,
                1 => Writer::Agent(at_work.remove(0)),
                _ => Writer::Shared(at_work),
            };
            (edit, writer)
        })
        .collect();
    record(core, store, project, changes, agents);
}

/// Which of `new`, files the last commit lacks, git would add: those it does
/// not ignore, or that were staged anyway.
fn untracked(top: &Path, new: &HashSet<&str>) -> HashSet<String> {
    if new.is_empty() {
        return HashSet::new();
    }
    let specs: Vec<String> = new
        .iter()
        .map(|inside| format!(":(top,literal){inside}"))
        .collect();
    let mut args = vec![
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
        "--",
    ];
    args.extend(specs.iter().map(String::as_str));
    let listed = run(top, &args, None, None).unwrap_or_default();
    String::from_utf8_lossy(&listed)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The lines `path` changed since what was kept of it, as an edit no one is
/// named on yet, with when the file last changed if the file system says it
/// finely enough, keeping the file as it is now. None when there is nothing
/// to tell: it did not change, it is as its last commit has it, or what it
/// held before is not known. `committed` is its last commit's copy, none
/// when that commit lacks it, and missing when git was not asked, as it is
/// not of a file kept already.
fn told(
    priors: &mut Priors,
    path: &Path,
    committed: Option<Option<&str>>,
) -> Option<(Edit, Option<Written>)> {
    let key = path.to_string_lossy();
    let Some((text, meta)) = text_of(path) else {
        if path.exists() {
            priors.keep(&key, None);
        } else {
            priors.forget(&key);
        }
        return None;
    };
    if committed == Some(Some(text.as_str())) {
        priors.forget(&key);
        return None;
    }
    let before = match (priors.before(&key), committed) {
        (Before::Kept(before), _) => before,
        (Before::Committed, Some(committed)) => committed.unwrap_or_default().to_owned(),
        (Before::Committed, None) => return None,
        (Before::Unknown, _) => {
            priors.keep(&key, Some((&text, &meta)));
            return None;
        }
    };
    priors.keep(&key, Some((&text, &meta)));
    let last = Written::exact(&meta, epoch_ms());
    let edit = (before != text).then(|| Edit {
        at: epoch_ms(),
        agent_id: String::new(),
        shared: Vec::new(),
        name: None,
        cli: String::new(),
        model: None,
        feed: String::new(),
        turn: None,
        tool_call_id: String::new(),
        path: key.into_owned(),
        lines: changed(&before, &text, 1),
        total: Some(line_count(&text)),
    });
    edit.map(|edit| (edit, last))
}

/// Writes down changes no edit tool reported, each with who made it. The
/// person's go in only where an agent's edit is on record, to take back the
/// lines it rewrote.
fn record(
    core: &Core,
    store: &Store,
    project: &str,
    changes: Vec<(Edit, Writer)>,
    agents: &mut Option<Vec<Listed>>,
) {
    let mut edits = Vec::with_capacity(changes.len());
    for (mut edit, writer) in changes {
        match writer {
            Writer::Person
                if store
                    .matching(project, |on| on.path == edit.path)
                    .is_empty() =>
            {
                continue
            }
            Writer::Person => {}
            Writer::Shared(agents) => edit.shared = agents,
            Writer::Agent(agent_id) => {
                edit.name = core.names.name(&agent_id);
                match core.chats.get(&agent_id) {
                    Some(chat) => {
                        edit.cli = chat.provider().to_owned();
                        edit.model = chat.current_launch().model;
                        edit.feed = chat.feed.id().to_owned();
                        edit.turn = chat.feed.turn_start();
                    }
                    None => {
                        edit.cli = agents
                            .get_or_insert_with(|| listed(core))
                            .iter()
                            .find(|agent| agent.agent_id == agent_id)
                            .map_or_else(|| "terminal".to_owned(), |agent| agent.cli.clone());
                    }
                }
                edit.agent_id = agent_id;
            }
        }
        edits.push(edit);
    }
    if !edits.is_empty() {
        store.append(project, &edits);
    }
}

/// Each of `paths`' copy in `top`'s last commit, none where it has none.
fn committed_copies(top: &Path, paths: &[&str]) -> HashMap<String, Option<String>> {
    let input: String = paths.iter().map(|path| format!("HEAD:{path}\n")).collect();
    let Ok(out) = run(top, &["cat-file", "--batch"], None, Some(input.as_bytes())) else {
        return HashMap::new();
    };
    let mut copies = HashMap::new();
    let mut rest = out.as_slice();
    for path in paths {
        let Some(end) = rest.iter().position(|byte| *byte == b'\n') else {
            break;
        };
        let header = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 1..];
        let found = match header.split(' ').collect::<Vec<_>>()[..] {
            [_, kind, size] => size
                .parse::<usize>()
                .ok()
                .map(|size| (kind == "blob", size)),
            _ => None,
        };
        let copy = match found {
            Some((blob, size)) if rest.len() > size => {
                let copy = String::from_utf8(rest[..size].to_vec())
                    .ok()
                    .filter(|_| blob);
                rest = &rest[size + 1..];
                copy
            }
            Some(_) => break,
            None => None,
        };
        copies.insert((*path).to_owned(), copy);
    }
    copies
}

/// Learns which folders git ignores from changed `files`, so their next
/// changes are let go of as they happen: the topmost ignored folder above
/// each, else, when `all_ignored`, the file itself.
fn learn_ignored<'a>(
    top: &Path,
    seen: &Mutex<Seen>,
    files: impl Iterator<Item = &'a str>,
    all_ignored: bool,
) {
    let files: Vec<&str> = files.collect();
    if files.is_empty() {
        return;
    }
    let mut folders = BTreeSet::new();
    for file in &files {
        let mut folder = Path::new(file).parent();
        while let Some(at) = folder.filter(|at| !at.as_os_str().is_empty()) {
            folders.insert(format!("{}/", at.to_string_lossy()));
            folder = at.parent();
        }
    }
    let input: String = folders.iter().map(|folder| format!("{folder}\0")).collect();
    let answered = run(
        top,
        &["check-ignore", "-z", "--stdin", "--no-index"],
        None,
        Some(input.as_bytes()),
    )
    .unwrap_or_default();
    let answered = String::from_utf8_lossy(&answered);
    let ignored_folders: Vec<&str> = answered.split('\0').filter(|at| !at.is_empty()).collect();
    let mut seen = seen.locked();
    for file in files {
        let topmost = ignored_folders
            .iter()
            .filter(|folder| file.starts_with(*folder))
            .min_by_key(|folder| folder.len());
        match topmost {
            Some(folder) => seen.ignore(top.join(folder.trim_end_matches('/'))),
            None if all_ignored => seen.ignore(top.join(file)),
            None => {}
        }
    }
    let Seen {
        changed, ignored, ..
    } = &mut *seen;
    changed.retain(|path, _| !ignored.iter().any(|ignored| path.starts_with(ignored)));
}
