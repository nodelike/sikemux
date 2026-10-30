//! Agents that share one checkout and change the same files. For each file
//! an agent read or changed this session the core keeps the copy that agent
//! last saw and the file's last few versions, and from them it warns two
//! agents when one changes lines the other has uncommitted, mends a write made
//! from an old copy so it keeps what others wrote since, holds short claims on
//! lockfiles, migrations and generated files, and runs the project's check
//! after each commit. Lines no agent wrote are the person's, and are kept like
//! any agent's. All of it lives in memory, within a fixed budget.

mod checks;
mod fragile;
mod merge;
mod watch;

use sikemux_fold::UpdateKind;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::edits::{self, authors, before, changed, line_count, replaced, Change, Changes, Edit};
use super::wake::{sooner, Wake};
use super::Core;
pub(crate) use checks::Failing;

/// Files bigger than this are not kept.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// What all the kept copies may add up to.
const MAX_KEPT_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILES: usize = 256;
const MAX_VERSIONS: usize = 4;
/// Tool calls kept open across all agents before the oldest is let go of.
const MAX_CALLS: usize = 256;
/// Shell commands kept per agent, to tell whether it was running one when a
/// file changed.
const MAX_SHELL_RUNS: usize = 8;
/// How much of a shell command's text is kept to tell which files it names.
const MAX_COMMAND_BYTES: usize = 16 * 1024;
/// How long a change on disk waits for an agent's edit tool to say it made it.
pub(crate) const GRACE_MS: u64 = 1500;
/// A burst of edits to the same lines makes one note, once it has been quiet
/// this long, or at the latest this long after it began.
const QUIET_MS: u64 = 4000;
const MAX_BURST_MS: u64 = 20_000;
/// Two agents colliding this often within the window are the person's to sort.
const REPEAT_WINDOW_MS: u64 = 10 * 60_000;
const REPEAT_LIMIT: usize = 3;
/// How long a claim on a fragile file outlasts its holder's last edit.
const CLAIM_MS: u64 = 5 * 60_000;
const MAX_HITS: usize = 512;
const MAX_FRAGILE: usize = 4096;

/// When a file changed: its own modification time when the file system keeps
/// it finely enough to tell back-to-back commands apart, else when the change
/// was seen.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Written {
    pub at: u64,
    pub exact: bool,
}

impl Written {
    /// A modification time far from when the change was seen was set by
    /// whatever wrote the file, such as an unpacked archive, so it is not used.
    pub(crate) fn of(path: &Path, seen: u64) -> Self {
        let written = std::fs::metadata(path)
            .ok()
            .and_then(|meta| Self::exact(&meta, seen));
        match written.filter(|written| written.at + GRACE_MS >= seen) {
            Some(written) => written,
            None => Self {
                at: seen,
                exact: false,
            },
        }
    }

    /// When the file `meta` describes last changed, if its file system keeps
    /// it finer than a second and it is not after `seen`, give or take the grace.
    pub(crate) fn exact(meta: &std::fs::Metadata, seen: u64) -> Option<Self> {
        let since = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        let at = since.as_millis() as u64;
        (since.subsec_nanos() != 0 && at <= seen + GRACE_MS).then_some(Self { at, exact: true })
    }
}

/// A copy of a file as one agent last saw it.
#[derive(Clone)]
struct Copy {
    text: Arc<str>,
    at: u64,
}

/// A state a file was in, and the agent whose edit made it, when known.
struct Version {
    text: Arc<str>,
    at: u64,
    /// Whether `at` is the file's own modification time.
    exact: bool,
    by: Option<String>,
    /// Looked at for a change no edit tool reported.
    settled: bool,
}

impl Version {
    fn written(&self) -> Written {
        Written {
            at: self.at,
            exact: self.exact,
        }
    }
}

#[derive(Default)]
struct File {
    versions: VecDeque<Version>,
    used: u64,
}

impl File {
    /// What the file held before it became `text`.
    fn before(&self, text: &str) -> Option<Copy> {
        let mut newest_first = self.versions.iter().rev().peekable();
        while newest_first
            .next_if(|version| &*version.text == text)
            .is_some()
        {}
        newest_first.next().map(|version| Copy {
            text: version.text.clone(),
            at: version.at,
        })
    }
}

#[derive(Clone, Default)]
struct Call {
    kind: String,
    paths: Vec<PathBuf>,
    at: u64,
    command: Option<Arc<str>>,
}

/// A shell command an agent ran, from when it started to when it ended.
struct ShellRun {
    started: u64,
    ended: u64,
    command: Option<Arc<str>>,
}

/// Lines one agent changed that another had uncommitted, gathered over a
/// burst of edits before the note goes out.
struct Overlap {
    project: String,
    lines: Vec<(u64, u64)>,
    first: u64,
    last: u64,
}

struct Hit {
    agents: (String, String),
    path: String,
    at: u64,
}

/// Two agents that keep colliding.
pub(crate) struct Pair {
    pub agents: (String, String),
    pub path: String,
    pub times: usize,
    pub since: u64,
}

struct Claim {
    agent: String,
    at: u64,
}

#[derive(Default)]
pub(super) struct State {
    files: HashMap<PathBuf, File>,
    seen: HashMap<(String, PathBuf), Copy>,
    /// What the file held just before each agent's latest recorded write.
    priors: HashMap<(String, PathBuf), Arc<str>>,
    calls: HashMap<(String, String), Call>,
    /// When each agent's latest shell commands started and finished.
    shell_runs: HashMap<String, VecDeque<ShellRun>>,
    overlaps: HashMap<(PathBuf, String, String), Overlap>,
    hits: VecDeque<Hit>,
    claims: HashMap<PathBuf, Claim>,
    claim_notes: HashMap<(PathBuf, String), u64>,
    commits: HashMap<(String, PathBuf), u64>,
    notes: Vec<(String, String)>,
    /// Whether each file is fragile, and why, once looked at.
    fragile: HashMap<PathBuf, Option<&'static str>>,
    checks: checks::Checks,
    /// The commit each project's branch was at when last looked at.
    heads: HashMap<String, String>,
    /// The inbox changed, so devices are sent the view again.
    publish: bool,
    bytes: usize,
    /// Files to start or stop watching once the lock is let go of.
    follow: Vec<PathBuf>,
    unfollow: Vec<PathBuf>,
}

pub(crate) struct Collisions {
    state: Mutex<State>,
    pub(crate) wake: Wake,
    watch: watch::Watch,
    core: Weak<Core>,
}

/// Queues `text` for an agent, sent from Sikemux once the lock is let go of.
pub(super) fn tell(state: &mut State, to: &str, text: String) {
    state.notes.push((to.to_owned(), text));
}

fn read(path: &Path) -> Option<Arc<str>> {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file() && meta.len() <= MAX_FILE_BYTES)
        .and_then(|_| std::fs::read_to_string(path).ok())
        .map(Arc::from)
}

/// `path` with its folder's links resolved, whether or not it exists yet.
fn real(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(folder), Some(name)) => std::fs::canonicalize(folder)
            .map(|folder| folder.join(name))
            .unwrap_or_else(|_| path.to_owned()),
        _ => path.to_owned(),
    }
}

/// `path` as the project shows it.
fn shown(path: &Path, project: &str) -> String {
    path.strip_prefix(project)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Line runs as a note says them: "lines 40–60", "line 7 and lines 9–12".
fn spoken(runs: &[(u64, u64)]) -> String {
    let mut runs = runs.to_vec();
    runs.sort_unstable();
    let mut joined: Vec<(u64, u64)> = Vec::new();
    for (start, end) in runs {
        match joined.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => joined.push((start, end)),
        }
    }
    let said: Vec<String> = joined
        .iter()
        .map(|(start, end)| {
            if start == end {
                format!("line {start}")
            } else {
                format!("lines {start}–{end}")
            }
        })
        .collect();
    match said.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => "lines".to_owned(),
    }
}

fn listed_names(names: &[String]) -> String {
    match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

impl Collisions {
    pub(crate) fn new(core: Weak<Core>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            wake: Wake::default(),
            watch: watch::Watch::default(),
            core,
        }
    }

    /// Starts and stops watching what the last change asked for.
    fn watch_files(&self) {
        let (follow, unfollow) = {
            let mut state = self.state.locked();
            (
                std::mem::take(&mut state.follow),
                std::mem::take(&mut state.unfollow),
            )
        };
        for path in follow {
            self.watch.follow(&self.core, &path);
        }
        for path in unfollow {
            self.watch.unfollow(&path);
        }
    }

    /// Notes what a chat's tool call says it reads, runs or is about to
    /// change: a read keeps the copy the agent now has, and a file about to
    /// change is kept as it stands first. Whether a shell command began.
    pub(crate) fn saw(&self, agent_id: &str, cwd: &Path, update: &Value) -> bool {
        if !UpdateKind::of(update).is_some_and(UpdateKind::is_tool) {
            return false;
        }
        let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
            return false;
        };
        let named = update
            .get("locations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(
                update
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("diff")),
            )
            .filter_map(|place| place.get("path")?.as_str())
            .map(|path| real(&cwd.join(path)));
        let status = update.get("status").and_then(Value::as_str);
        let ended = matches!(status, Some("completed" | "failed"));
        let key = (agent_id.to_owned(), id.to_owned());
        let (call, began) = {
            let mut state = self.state.locked();
            if !state.calls.contains_key(&key) && state.calls.len() >= MAX_CALLS {
                let oldest = state
                    .calls
                    .iter()
                    .min_by_key(|(_, call)| call.at)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    state.calls.remove(&oldest);
                }
            }
            let call = state.calls.entry(key.clone()).or_insert_with(|| Call {
                at: epoch_ms(),
                ..Call::default()
            });
            let was_command = call.kind == "execute";
            if let Some(kind) = update.get("kind").and_then(Value::as_str) {
                kind.clone_into(&mut call.kind);
            }
            for path in named {
                if !call.paths.contains(&path) {
                    call.paths.push(path);
                }
            }
            if call.command.is_none() {
                call.command = command_text(update);
            }
            let call = call.clone();
            let began = !was_command && !ended && call.kind == "execute";
            if ended {
                state.calls.remove(&key);
                if call.kind == "execute" {
                    let runs = state.shell_runs.entry(agent_id.to_owned()).or_default();
                    if runs.len() >= MAX_SHELL_RUNS {
                        runs.pop_front();
                    }
                    runs.push_back(ShellRun {
                        started: call.at,
                        ended: epoch_ms(),
                        command: call.command.clone(),
                    });
                }
            }
            (call, began)
        };
        let reading = call.kind == "read" && status == Some("completed");
        let changing = matches!(call.kind.as_str(), "edit" | "delete" | "move") && !ended;
        if !reading && !changing {
            return began;
        }
        let agent_id = agent_id.to_owned();
        let core = self.core.clone();
        off_thread(move || {
            let Some(core) = core.upgrade() else { return };
            for path in &call.paths {
                let Some(text) = read(path) else { continue };
                if reading {
                    core.collisions.keep_seen(&agent_id, path, text);
                } else {
                    core.collisions.observe(path, text, None);
                }
            }
            core.collisions.watch_files();
        });
        began
    }

    /// Keeps `text` as the copy of `path` the agent last saw.
    fn keep_seen(&self, agent_id: &str, path: &Path, text: Arc<str>) {
        self.observe(path, text.clone(), None);
        let mut state = self.state.locked();
        let at = epoch_ms();
        let key = (agent_id.to_owned(), path.to_path_buf());
        let freed = state.seen.get(&key).map_or(0, |copy| copy.text.len());
        state.bytes = state.bytes.saturating_sub(freed) + text.len();
        state.seen.insert(key, Copy { text, at });
        fit(&mut state);
    }

    /// Notes that `path` holds `text`, made by `by` when known. A change no
    /// edit tool claims is looked at once the grace time has passed.
    fn observe(&self, path: &Path, text: Arc<str>, by: Option<&str>) {
        let now = epoch_ms();
        let written = match by {
            Some(_) => Written {
                at: now,
                exact: false,
            },
            None => Written::of(path, now),
        };
        let mut state = self.state.locked();
        let tracked = state.files.contains_key(path);
        if !tracked {
            state.follow.push(path.to_path_buf());
        }
        let file = state.files.entry(path.to_path_buf()).or_default();
        file.used = now;
        let added = match file.versions.back_mut() {
            Some(last) if *last.text == *text => {
                if by.is_some() && last.by.is_none() {
                    last.by = by.map(str::to_owned);
                    last.settled = true;
                }
                0
            }
            _ => {
                let mut freed = 0;
                if file.versions.len() >= MAX_VERSIONS {
                    freed = file.versions.pop_front().map_or(0, |old| old.text.len());
                }
                file.versions.push_back(Version {
                    text: text.clone(),
                    at: written.at,
                    exact: written.exact,
                    by: by.map(str::to_owned),
                    settled: by.is_some() || !tracked,
                });
                state.bytes = state.bytes.saturating_sub(freed);
                text.len()
            }
        };
        state.bytes += added;
        fit(&mut state);
        let unclaimed = state
            .files
            .get(path)
            .and_then(|file| file.versions.back())
            .is_some_and(|last| !last.settled);
        drop(state);
        if unclaimed {
            self.wake.at(now + GRACE_MS);
        }
    }

    /// A file in a watched folder changed on disk.
    pub(crate) fn disk_changed(&self, path: &Path) {
        if !self.state.locked().files.contains_key(path) {
            return;
        }
        if let Some(text) = read(path) {
            self.observe(path, text, None);
        }
    }

    /// An agent's tool call just changed `changes`, relative to `cwd`. A
    /// write made from a copy older than the file is mended on disk to keep
    /// what others changed since, and the lines it changed are worked out
    /// again for the record.
    pub(crate) fn wrote(&self, core: &Core, agent_id: &str, cwd: &Path, changes: &mut Changes) {
        for (path, change) in changes.iter_mut() {
            let path = real(&cwd.join(&*path));
            let Some(now) = read(&path) else { continue };
            let (prior, base, until) = {
                let state = self.state.locked();
                let file = state.files.get(&path);
                (
                    file.and_then(|file| file.before(&now)),
                    state
                        .seen
                        .get(&(agent_id.to_owned(), path.clone()))
                        .cloned(),
                    file.and_then(|file| {
                        file.versions
                            .iter()
                            .rev()
                            .take_while(|version| *version.text == *now)
                            .last()
                            .map(|version| version.at)
                    })
                    .unwrap_or(u64::MAX),
                )
            };
            self.observe(&path, now.clone(), Some(agent_id));
            let mut lines = None;
            let prior_text = prior.as_ref().map(|prior| prior.text.clone());
            if let (Some(base), Some(prior)) = (&base, &prior) {
                if base.text != prior.text
                    && prior.text != now
                    && self.changed_by_others(core, agent_id, &path, base, until)
                {
                    if let Some(result) = self.mend(core, agent_id, &path, base, &prior.text, &now)
                    {
                        lines = Some(changed(&prior.text, &result, 1));
                    }
                }
            }
            let mut state = self.state.locked();
            let key = (agent_id.to_owned(), path.clone());
            let freed = state.seen.get(&key).map_or(0, |copy| copy.text.len());
            state.bytes = state.bytes.saturating_sub(freed) + now.len();
            state.seen.insert(
                key.clone(),
                Copy {
                    text: now,
                    at: epoch_ms(),
                },
            );
            match prior_text {
                Some(prior) => state.priors.insert(key, prior),
                None => state.priors.remove(&key),
            };
            fit(&mut state);
            drop(state);
            if let Some(lines) = lines {
                *change = Change::Lines(lines);
            }
        }
        self.watch_files();
    }

    /// Writes `ours`, made from `base`, merged with `theirs`, which the file
    /// held before it, unless the file changed again meanwhile. The merged
    /// text, when it differs from `ours`.
    fn mend(
        &self,
        core: &Core,
        agent_id: &str,
        path: &Path,
        base: &Copy,
        theirs: &str,
        ours: &str,
    ) -> Option<Arc<str>> {
        let merged = merge::merge(&base.text, ours, theirs);
        if merged.text == ours {
            return None;
        }
        let result: Arc<str> = merged.text.as_str().into();
        if read(path).as_deref() != Some(ours) {
            return None;
        }
        self.observe(path, result.clone(), Some(agent_id));
        if std::fs::write(path, &*result).is_err() {
            return None;
        }
        let mut state = self.state.locked();
        let note = mended_note(core, &state, agent_id, path, base, &merged.kept);
        tell(&mut state, agent_id, note);
        drop(state);
        self.wake.now();
        Some(result)
    }

    /// The record took `edits` in `project`: warns of lines another agent
    /// had uncommitted that they wrote over, and of fragile files another
    /// agent holds.
    pub(crate) fn landed(&self, core: &Core, project: &str, edits: &[Edit]) {
        for edit in edits {
            let path = PathBuf::from(&edit.path);
            self.claim(core, project, edit, &path);
            let prior = self
                .state
                .locked()
                .priors
                .remove(&(edit.agent_id.clone(), path.clone()));
            let Some(prior) = prior else { continue };
            let earlier = core.edits.of_file(project, &edit.path, Some(edit));
            let (lines, total) = authors(&earlier);
            if total.is_some_and(|total| total != line_count(&prior)) {
                continue;
            }
            let others: HashMap<u64, &str> = lines
                .iter()
                .filter(|line| line.agent_id != edit.agent_id)
                .map(|line| (line.line, line.agent_id.as_str()))
                .collect();
            if others.is_empty() {
                continue;
            }
            let mut hit: HashMap<&str, Vec<(u64, u64, u64)>> = HashMap::new();
            for (old, [start, count, removed]) in before(&edit.lines) {
                let after = (start, start + count.max(1) - 1);
                let replaced_lines = old..old + removed;
                let mut owners: Vec<(&str, u64)> = replaced_lines
                    .filter_map(|line| others.get(&line).map(|agent| (*agent, line)))
                    .collect();
                if removed == 0 {
                    if let (Some(above), Some(below)) = (others.get(&(old - 1)), others.get(&old)) {
                        if above == below {
                            owners.push((above, old));
                        }
                    }
                }
                for (agent, line) in owners {
                    hit.entry(agent).or_default().push((line, after.0, after.1));
                }
            }
            if hit.is_empty() {
                continue;
            }
            let uncommitted = uncommitted(&path, &prior);
            for (owner, lines) in hit {
                let touched: Vec<(u64, u64)> = lines
                    .iter()
                    .filter(|(line, _, _)| {
                        uncommitted.as_ref().is_none_or(|open| open.contains(line))
                    })
                    .map(|(_, start, end)| (*start, *end))
                    .collect();
                if touched.is_empty() || core.names.get(owner).is_none() {
                    continue;
                }
                self.overlapped(project, &path, &edit.agent_id, owner, &touched);
            }
        }
    }

    /// Gathers lines `writer` changed that `owner` had uncommitted, for the
    /// note that goes out once the burst is over. An unknown writer is "".
    fn overlapped(
        &self,
        project: &str,
        path: &Path,
        writer: &str,
        owner: &str,
        lines: &[(u64, u64)],
    ) {
        let now = epoch_ms();
        let mut state = self.state.locked();
        let overlap = state
            .overlaps
            .entry((path.to_path_buf(), writer.to_owned(), owner.to_owned()))
            .or_insert_with(|| Overlap {
                project: project.to_owned(),
                lines: Vec::new(),
                first: now,
                last: now,
            });
        overlap.lines.extend_from_slice(lines);
        overlap.last = now;
        drop(state);
        self.wake.at(now + QUIET_MS);
    }

    /// Holds or refreshes `edit`'s agent's claim on a fragile file, or tells
    /// both agents when another holds it.
    fn claim(&self, core: &Core, project: &str, edit: &Edit, path: &Path) {
        let known = self.state.locked().fragile.get(path).copied();
        let why = known.unwrap_or_else(|| {
            let why = fragile::why(path);
            let mut state = self.state.locked();
            if state.fragile.len() >= MAX_FRAGILE {
                state.fragile.clear();
            }
            state.fragile.insert(path.to_path_buf(), why);
            why
        });
        let Some(why) = why else {
            return;
        };
        let now = epoch_ms();
        let agent = &edit.agent_id;
        let mut state = self.state.locked();
        let holder = state
            .claims
            .get(path)
            .filter(|claim| {
                claim.agent != *agent
                    && now.saturating_sub(claim.at) < CLAIM_MS
                    && core.names.get(&claim.agent).is_some()
            })
            .map(|claim| (claim.agent.clone(), claim.at));
        let Some((holder, held_at)) = holder else {
            state.claims.insert(
                path.to_path_buf(),
                Claim {
                    agent: agent.clone(),
                    at: now,
                },
            );
            drop(state);
            self.wake.at(now + CLAIM_MS);
            return;
        };
        let key = (path.to_path_buf(), agent.clone());
        if state
            .claim_notes
            .get(&key)
            .is_some_and(|at| now.saturating_sub(*at) < CLAIM_MS)
        {
            return;
        }
        state.claim_notes.insert(key, now);
        let name = |id: &str| core.names.name(id).unwrap_or_else(|| id.to_owned());
        let (agent_name, holder_name) = (name(agent), name(&holder));
        let file = shown(path, project);
        let minutes = now.saturating_sub(held_at) / 60_000;
        let ago = if minutes == 0 {
            "less than a minute ago".to_owned()
        } else {
            format!("{minutes} min ago")
        };
        tell(
            &mut state,
            agent,
            format!("{holder_name} holds a short claim on {file}, {why}, which it last edited {ago}. Your edit went in, but agree with {holder_name} with agent_send before changing it further, so you do not regenerate or rewrite it over each other."),
        );
        tell(
            &mut state,
            &holder,
            format!("{agent_name} just edited {file}, {why} you hold a short claim on. Agree with {agent_name} with agent_send on which of you finishes it."),
        );
        hit(&mut state, agent, &holder, file, now);
        state.publish = true;
        drop(state);
        self.wake.now();
    }

    /// `agent_id` committed `paths`, from the top of `top`, as `commit`: the
    /// copies it saw of them and its claims on them go, and the project's
    /// check runs.
    pub(crate) fn committed(
        &self,
        core: &Core,
        agent_id: &str,
        top: &Path,
        paths: &[String],
        commit: &str,
        message: &str,
    ) {
        let now = epoch_ms();
        let project = core.workspaces.project_of(top);
        let mut state = self.state.locked();
        for path in paths {
            let path = real(&top.join(path));
            let key = (agent_id.to_owned(), path.clone());
            if let Some(copy) = state.seen.remove(&key) {
                state.bytes = state.bytes.saturating_sub(copy.text.len());
            }
            state.priors.remove(&key);
            state.commits.insert(key, now);
            if state
                .claims
                .get(&path)
                .is_some_and(|claim| claim.agent == agent_id)
            {
                state.claims.remove(&path);
            }
        }
        if Path::new(&project).starts_with(top) {
            let mut agents: Vec<String> = crate::trailers::agents(message)
                .into_iter()
                .map(|agent| agent.chat)
                .collect();
            if agents.is_empty() {
                agents.push(agent_id.to_owned());
            }
            state.checks.wait(
                project.clone(),
                PathBuf::from(&project),
                checks::Commit {
                    sha: commit.to_owned(),
                    subject: message.lines().next().unwrap_or_default().to_owned(),
                    agents,
                },
            );
        }
        state.publish = true;
        drop(state);
        self.wake.now();
    }

    /// The pairs of agents that keep colliding.
    pub(crate) fn pairs(&self) -> Vec<Pair> {
        let state = self.state.locked();
        let mut pairs: Vec<Pair> = Vec::new();
        for hit in &state.hits {
            let same = |pair: &&mut Pair| {
                let (a, b) = &pair.agents;
                (*a == hit.agents.0 && *b == hit.agents.1)
                    || (*a == hit.agents.1 && *b == hit.agents.0)
            };
            match pairs.iter_mut().find(same) {
                Some(pair) => {
                    pair.times += 1;
                    hit.path.clone_into(&mut pair.path);
                }
                None => pairs.push(Pair {
                    agents: hit.agents.clone(),
                    path: hit.path.clone(),
                    times: 1,
                    since: hit.at,
                }),
            }
        }
        pairs.retain(|pair| pair.times >= REPEAT_LIMIT);
        pairs
    }

    /// The projects whose check has failed long enough for the person to hear.
    pub(crate) fn failing(&self) -> Vec<Failing> {
        self.state.locked().checks.failing().cloned().collect()
    }

    /// `project`'s git state may have moved. When its branch is at another
    /// commit than when last looked at, by whatever commit, an agent's own
    /// `git commit` or the person's among them, agents waiting on its events
    /// hear it; its newest commit waits for the project's check, with the
    /// agents its footer names.
    pub(crate) fn head_moved(&self, core: &Arc<Core>, project: &str) {
        let (core, project) = (core.clone(), project.to_owned());
        tokio::task::spawn_blocking(move || {
            let dir = PathBuf::from(&project);
            let Ok(repo) = git2::Repository::discover(&dir) else {
                return;
            };
            let at = repo
                .head()
                .ok()
                .and_then(|head| head.target())
                .map(|id| id.to_string())
                .unwrap_or_default();
            let before = core
                .collisions
                .state
                .locked()
                .heads
                .insert(project.clone(), at.clone());
            if before.is_some_and(|before| before != at) {
                core.harness.note_repository(&project, "git.moved");
            }
            if crate::server::board::check_command(&core, &project).is_none() {
                return;
            }
            let Ok(head) = edits::git(&dir, &["log", "-1", "--format=%H%x00%s"]) else {
                return;
            };
            let Some((sha, subject)) = head.trim_end().split_once('\0') else {
                return;
            };
            let agents = edits::footer::verified(&core, &dir, sha)
                .unwrap_or_default()
                .into_iter()
                .map(|agent| agent.chat)
                .collect();
            let commit = checks::Commit {
                sha: sha.to_owned(),
                subject: subject.to_owned(),
                agents,
            };
            core.collisions
                .state
                .locked()
                .checks
                .wait(project.clone(), dir, commit);
            core.collisions.wake.now();
        });
    }
}

fn in_project(core: &Core, project: &str, agent: &str) -> bool {
    core.names
        .get(agent)
        .is_some_and(|named| named.project == project)
}

/// Whether `agent` works in the checkout that holds `place`.
fn holds(core: &Core, place: &Path, agent: &str) -> bool {
    core.edits
        .checkout_of(core, agent)
        .is_none_or(|top| place.starts_with(&top))
}

/// Lets go of the least recently used files until what is kept fits.
fn fit(state: &mut State) {
    while state.bytes > MAX_KEPT_BYTES || state.files.len() > MAX_FILES {
        let Some(oldest) = state
            .files
            .iter()
            .min_by_key(|(_, file)| file.used)
            .map(|(path, _)| path.clone())
        else {
            return;
        };
        forget(state, &oldest);
    }
}

fn forget(state: &mut State, path: &Path) {
    if let Some(file) = state.files.remove(path) {
        let freed: usize = file.versions.iter().map(|version| version.text.len()).sum();
        state.bytes = state.bytes.saturating_sub(freed);
        state.unfollow.push(path.to_path_buf());
    }
    let mut freed = 0;
    state.seen.retain(|(_, seen), copy| {
        let keep = seen != path;
        if !keep {
            freed += copy.text.len();
        }
        keep
    });
    state.bytes = state.bytes.saturating_sub(freed);
    state.priors.retain(|(_, prior), _| prior != path);
}

fn hit(state: &mut State, a: &str, b: &str, path: String, at: u64) {
    if state.hits.len() >= MAX_HITS {
        state.hits.pop_front();
    }
    state.hits.push_back(Hit {
        agents: (a.to_owned(), b.to_owned()),
        path,
        at,
    });
}

/// Who changed `path` since `since`, other than `agent_id`, by name.
fn changed_by(core: &Core, state: &State, agent_id: &str, path: &Path, since: u64) -> Vec<String> {
    let mut names = Vec::new();
    for version in state
        .files
        .get(path)
        .into_iter()
        .flat_map(|file| &file.versions)
        .filter(|version| version.at > since)
    {
        let name = match &version.by {
            Some(by) if by == agent_id => continue,
            Some(by) => core.names.name(by).unwrap_or_else(|| by.clone()),
            None => "someone outside the agents' edit tools".to_owned(),
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// What an agent is told when its write, made from an older copy, was mended.
fn mended_note(
    core: &Core,
    state: &State,
    agent_id: &str,
    path: &Path,
    base: &Copy,
    kept: &[std::ops::Range<usize>],
) -> String {
    let project = core.workspaces.project_of(path);
    let file = shown(path, &project);
    let mut by = changed_by(core, state, agent_id, path, base.at);
    if by.is_empty() {
        by.push("someone else".to_owned());
    }
    let mut note = format!(
        "Your write to {file} was made from an older copy: {} changed it after you last read it. Sikemux kept their changes along with yours",
        listed_names(&by)
    );
    if kept.is_empty() {
        note.push('.');
    } else {
        let runs: Vec<(u64, u64)> = kept
            .iter()
            .map(|range| {
                let start = range.start as u64 + 1;
                (start, (range.end as u64).max(start))
            })
            .collect();
        note.push_str(&format!(
            ", except where you both changed the same lines: there it kept their version, now {} of the file. Redo your change to those lines on the file as it is now.",
            spoken(&runs)
        ));
    }
    note.push_str(" Read the file again before you edit it.");
    note
}

/// The lines of `text`, `path` as it stands in the checkout, counted from 1,
/// that its last commit does not have. None outside a git checkout.
fn uncommitted(path: &Path, text: &str) -> Option<HashSet<u64>> {
    let folder = path.parent()?;
    let top = edits::git(folder, &["rev-parse", "--show-toplevel"]).ok()?;
    let top = PathBuf::from(top.trim());
    let inside = path.strip_prefix(&top).ok()?.to_string_lossy().into_owned();
    let committed = edits::git(&top, &["show", &format!("HEAD:{inside}")]).unwrap_or_default();
    let now = merge::lines(text);
    Some(
        replaced(&merge::lines(&committed), &now)
            .into_iter()
            .flat_map(|change| change.new)
            .map(|line| line as u64 + 1)
            .collect(),
    )
}

fn off_thread(work: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn_blocking(work);
        }
        Err(_) => work(),
    }
}

/// A change on disk that no agent's edit tool claimed within the grace time.
struct Unclaimed {
    path: PathBuf,
    before: Arc<str>,
    after: Arc<str>,
    written: Written,
}

/// Sends the notes that wait, lets go of what has aged, looks at changes no
/// edit tool claimed, and starts the checks that wait. Runs whenever the
/// collisions' wake is woken.
pub(crate) fn settle(core: &Arc<Core>) {
    let collisions = &core.collisions;
    let now = epoch_ms();
    let mut next = None;
    let mut state = collisions.state.locked();
    while state
        .hits
        .front()
        .is_some_and(|hit| now.saturating_sub(hit.at) >= REPEAT_WINDOW_MS)
    {
        state.hits.pop_front();
    }
    next = sooner(
        next,
        state.hits.front().map(|hit| hit.at + REPEAT_WINDOW_MS),
    );
    state.claims.retain(|_, claim| {
        now.saturating_sub(claim.at) < CLAIM_MS && core.names.get(&claim.agent).is_some()
    });
    next = sooner(
        next,
        state.claims.values().map(|claim| claim.at + CLAIM_MS).min(),
    );
    state
        .claim_notes
        .retain(|_, at| now.saturating_sub(*at) < CLAIM_MS);
    let gone: Vec<String> = state
        .seen
        .keys()
        .map(|(agent, _)| agent.clone())
        .filter(|agent| core.names.get(agent).is_none())
        .collect();
    for agent in gone {
        let mut freed = 0;
        state.seen.retain(|(seen_by, _), copy| {
            let keep = *seen_by != agent;
            if !keep {
                freed += copy.text.len();
            }
            keep
        });
        state.bytes = state.bytes.saturating_sub(freed);
        state.calls.retain(|(caller, _), _| *caller != agent);
        state.shell_runs.remove(&agent);
    }
    let due: Vec<(PathBuf, String, String)> = state
        .overlaps
        .iter()
        .filter(|(_, overlap)| {
            now >= overlap.last + QUIET_MS || now >= overlap.first + MAX_BURST_MS
        })
        .map(|(key, _)| key.clone())
        .collect();
    next = sooner(
        next,
        state
            .overlaps
            .values()
            .map(|overlap| (overlap.last + QUIET_MS).min(overlap.first + MAX_BURST_MS))
            .min(),
    );
    let mut flushed = false;
    for key in due {
        if let Some(overlap) = state.overlaps.remove(&key) {
            warn(core, &mut state, &key, overlap, now);
            flushed = true;
        }
    }
    let mut unclaimed = Vec::new();
    for (path, file) in &mut state.files {
        let mut previous: Option<Arc<str>> = None;
        for version in &mut file.versions {
            if !version.settled && now >= version.at + GRACE_MS {
                version.settled = true;
                if let Some(before) = &previous {
                    unclaimed.push(Unclaimed {
                        path: path.clone(),
                        before: before.clone(),
                        after: version.text.clone(),
                        written: version.written(),
                    });
                }
            } else if !version.settled {
                next = sooner(next, Some(version.at + GRACE_MS));
            }
            previous = Some(version.text.clone());
        }
    }
    checks::start(core, &mut state);
    let notes = std::mem::take(&mut state.notes);
    let publish = std::mem::take(&mut state.publish) || flushed;
    drop(state);
    collisions.watch_files();
    if let Some(next) = next {
        collisions.wake.at(next);
    }
    for (to, text) in notes {
        let core = core.clone();
        tokio::spawn(async move {
            if let Err(error) = super::messages::from_sikemux(&core, to, text).await {
                eprintln!(
                    "sikemux core: a note about shared files did not reach its agent: {error}"
                );
            }
        });
    }
    if !unclaimed.is_empty() {
        let core = core.clone();
        tokio::task::spawn_blocking(move || {
            for change in unclaimed {
                core.collisions.unclaimed(&core, change);
            }
        });
    }
    if publish {
        core.view_changed();
    }
}

/// Sends the note for a burst of lines `writer` changed that `owner` had
/// uncommitted, to both and to their leads.
fn warn(
    core: &Core,
    state: &mut State,
    key: &(PathBuf, String, String),
    overlap: Overlap,
    now: u64,
) {
    let (path, writer, owner) = key;
    let name = |id: &str| core.names.name(id).unwrap_or_else(|| id.to_owned());
    let file = shown(path, &overlap.project);
    let lines = spoken(&overlap.lines);
    let owner_name = name(owner);
    if writer.is_empty() {
        tell(
            state,
            owner,
            format!("{lines} of {file}, which you have uncommitted, were just changed outside the agents' edit tools, by a command or a person Sikemux could not tell. Read the file again before you edit it."),
        );
        return;
    }
    let writer_name = name(writer);
    tell(
        state,
        owner,
        format!("{writer_name} changed {lines} of {file}, which you have uncommitted. Read the file again before you edit it."),
    );
    tell(
        state,
        writer,
        format!("You changed {lines} of {file}, where {owner_name} has uncommitted lines. {owner_name} has been told. Read the file again, and agree with {owner_name} with agent_send before you change those lines again."),
    );
    let leads: Vec<String> = [writer, owner]
        .iter()
        .filter_map(|agent| core.names.get(agent).and_then(|named| named.parent))
        .filter(|lead| lead != writer && lead != owner)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for lead in leads {
        tell(
            state,
            &lead,
            format!("{writer_name} changed {lines} of {file}, which {owner_name} has uncommitted. Both have been told; step in if they keep working on the same lines."),
        );
    }
    hit(state, writer, owner, file, now);
}

impl Collisions {
    /// Looks at a change on disk no agent's edit tool claimed: who most
    /// likely made it, whose uncommitted lines it changed, and, when an
    /// agent's command wrote over the file from an older copy, mends it.
    fn unclaimed(&self, core: &Arc<Core>, change: Unclaimed) {
        let Unclaimed {
            path,
            before: prior,
            after: now,
            written,
        } = change;
        let at = written.at;
        let project = core.workspaces.project_of(&path);
        let listed = super::messages::listed(core);
        let (shells, terminals) = self.writers(core, &listed, &project, &path, written);
        let writer = match (shells.len(), terminals.len()) {
            (1, 0) => shells.first().cloned().map(|agent| (agent, true)),
            (0, 1) => terminals.first().cloned().map(|agent| (agent, false)),
            _ => None,
        };
        if let Some((agent, true)) = &writer {
            let key = (agent.clone(), path.clone());
            let base = self.state.locked().seen.get(&key).cloned();
            if let Some(base) = base.filter(|base| base.at < at && base.text != prior) {
                if self.changed_by_others(core, agent, &path, &base, at) {
                    self.mend(core, agent, &path, &base, &prior, &now);
                }
            }
        }
        {
            let mut state = self.state.locked();
            for agent in &shells {
                let key = (agent.clone(), path.clone());
                let older = state.seen.get(&key).map(|copy| (copy.at, copy.text.len()));
                if older.is_some_and(|(seen_at, _)| seen_at < at) {
                    let freed = older.map_or(0, |(_, len)| len);
                    state.bytes = state.bytes.saturating_sub(freed) + now.len();
                    state.seen.insert(
                        key,
                        Copy {
                            text: now.clone(),
                            at,
                        },
                    );
                }
            }
            if let Some((agent, true)) = &writer {
                let key = (agent.clone(), path.clone());
                if !state.seen.contains_key(&key) {
                    state.bytes += now.len();
                    state.seen.insert(
                        key,
                        Copy {
                            text: now.clone(),
                            at,
                        },
                    );
                }
            }
            fit(&mut state);
        }
        let mut all = core.edits.of_file(&project, &path.to_string_lossy(), None);
        all.retain(|edit| edit.at <= at);
        let (lines, total) = authors(&all);
        if total.is_some_and(|total| total != line_count(&prior)) {
            return;
        }
        let writer_id = writer.map(|(agent, _)| agent).unwrap_or_default();
        let maybe_own = |owner: &str| shells.contains(owner) || terminals.contains(owner);
        let owned: HashMap<u64, &str> = lines
            .iter()
            .filter(|line| line.agent_id != writer_id)
            .map(|line| (line.line, line.agent_id.as_str()))
            .collect();
        if owned.is_empty() {
            return;
        }
        let before_lines = merge::lines(&prior);
        let after_lines = merge::lines(&now);
        let mut hit: HashMap<&str, Vec<(u64, u64, u64)>> = HashMap::new();
        for change in replaced(&before_lines, &after_lines) {
            let start = change.new.start as u64 + 1;
            let end = (change.new.end as u64).max(start);
            let old = change.old.start as u64 + 1;
            let mut owners: Vec<(&str, u64)> = (old..change.old.end as u64 + 1)
                .filter_map(|line| owned.get(&line).map(|agent| (*agent, line)))
                .collect();
            if change.old.is_empty() {
                if let (Some(above), Some(below)) = (owned.get(&(old - 1)), owned.get(&old)) {
                    if above == below {
                        owners.push((above, old));
                    }
                }
            }
            for (agent, line) in owners {
                hit.entry(agent).or_default().push((line, start, end));
            }
        }
        let open = uncommitted(&path, &prior);
        for (owner, lines) in hit {
            let touched: Vec<(u64, u64)> = lines
                .iter()
                .filter(|(line, _, _)| open.as_ref().is_none_or(|open| open.contains(line)))
                .map(|(_, start, end)| (*start, *end))
                .collect();
            if !touched.is_empty() && core.names.get(owner).is_some() && !maybe_own(owner) {
                self.overlapped(&project, &path, &writer_id, owner, &touched);
            }
        }
    }

    /// Whether anyone but `agent_id` changed `path` after it saw `seen` and
    /// before `until`: another agent's edit tool, or a change no edit tool
    /// claimed that `agent_id` was not at work to have made.
    fn changed_by_others(
        &self,
        core: &Core,
        agent_id: &str,
        path: &Path,
        seen: &Copy,
        until: u64,
    ) -> bool {
        let unclaimed: Vec<Written> = {
            let state = self.state.locked();
            let mut unclaimed = Vec::new();
            for version in state
                .files
                .get(path)
                .into_iter()
                .flat_map(|file| &file.versions)
                .filter(|version| {
                    version.at > seen.at && version.at < until && version.text != seen.text
                })
            {
                match &version.by {
                    Some(by) if by == agent_id => {}
                    Some(_) => return true,
                    None => unclaimed.push(version.written()),
                }
            }
            unclaimed
        };
        if unclaimed.is_empty() {
            return false;
        }
        let project = core.workspaces.project_of(path);
        let listed = super::messages::listed(core);
        unclaimed.into_iter().any(|written| {
            let (shells, terminals) = self.writers(core, &listed, &project, path, written);
            !shells.contains(agent_id) && !terminals.contains(agent_id)
        })
    }

    /// The agents that could have written `place` when it changed: when the
    /// time is the file's own and a command ran at that very time, those at
    /// work then, else those at work then or up to the grace before, as
    /// [`Self::at_work`].
    pub(crate) fn writers(
        &self,
        core: &Core,
        listed: &[super::messages::Listed],
        project: &str,
        place: &Path,
        written: Written,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let exact = written.exact
            && !self
                .commands(core, project, place, written.at, 0)
                .is_empty();
        let grace = if exact { 0 } else { GRACE_MS };
        let running = self.commands(core, project, place, written.at, grace);
        let naming: BTreeSet<String> = running
            .iter()
            .filter(|(_, commands)| commands.iter().any(|command| names_file(command, place)))
            .map(|(agent, _)| agent.clone())
            .collect();
        if !naming.is_empty() {
            return (naming, BTreeSet::new());
        }
        let terminals = terminals_at_work(core, listed, project, place);
        if running.is_empty() {
            return (in_background(core, project, place), terminals);
        }
        (running.into_keys().collect(), terminals)
    }

    /// Whether an agent's edit tool is changing `path` now.
    pub(crate) fn editing(&self, path: &Path) -> bool {
        self.state.locked().calls.values().any(|call| {
            matches!(call.kind.as_str(), "edit" | "delete" | "move")
                && call.paths.iter().any(|called| called == path)
        })
    }

    /// The agents that could have changed `place`, a file or folder in
    /// `project`, at `at` with no edit tool: the chat agents then running a
    /// command, or one that ended up to `grace` before, a subagent or a
    /// background task, and the terminal agents then working, each in the
    /// checkout that holds `place`.
    pub(crate) fn at_work(
        &self,
        core: &Core,
        listed: &[super::messages::Listed],
        project: &str,
        place: &Path,
        at: u64,
        grace: u64,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut shells: BTreeSet<String> = self
            .commands(core, project, place, at, grace)
            .into_keys()
            .collect();
        shells.extend(in_background(core, project, place));
        (shells, terminals_at_work(core, listed, project, place))
    }

    /// The chat agents in `project` that were running a shell command at
    /// `at`, or ended one up to `grace` before, in the checkout holding
    /// `place`, each with the text of those commands.
    fn commands(
        &self,
        core: &Core,
        project: &str,
        place: &Path,
        at: u64,
        grace: u64,
    ) -> BTreeMap<String, Vec<Arc<str>>> {
        let mut found: BTreeMap<String, Vec<Arc<str>>> = BTreeMap::new();
        {
            let state = self.state.locked();
            let open = state
                .calls
                .iter()
                .filter(|(_, call)| call.kind == "execute" && call.at <= at)
                .map(|((agent, _), call)| (agent, call.command.clone()));
            let ended = state.shell_runs.iter().flat_map(|(agent, runs)| {
                runs.iter()
                    .filter(|run| run.started <= at && at <= run.ended + grace)
                    .map(move |run| (agent, run.command.clone()))
            });
            for (agent, command) in open.chain(ended) {
                found.entry(agent.clone()).or_default().extend(command);
            }
        }
        found.retain(|agent, _| in_project(core, project, agent) && holds(core, place, agent));
        found
    }
}

/// The chat agents in `project` with subagents or background tasks going,
/// in the checkout holding `place`: whatever those run reports no command.
fn in_background(core: &Core, project: &str, place: &Path) -> BTreeSet<String> {
    core.chats
        .list()
        .into_iter()
        .filter(|chat| chat.subagents > 0 || chat.tasks > 0)
        .map(|chat| chat.agent_id)
        .filter(|agent| in_project(core, project, agent) && holds(core, place, agent))
        .collect()
}

/// The terminal agents in `project` working now, in the checkout holding `place`.
fn terminals_at_work(
    core: &Core,
    listed: &[super::messages::Listed],
    project: &str,
    place: &Path,
) -> BTreeSet<String> {
    listed
        .iter()
        .filter(|agent| {
            agent.reach == super::messages::Reach::Terminal
                && agent.project == project
                && agent.status == crate::protocol::AgentStatus::Working
        })
        .map(|agent| agent.agent_id.clone())
        .filter(|agent| holds(core, place, agent))
        .collect()
}

/// The text of the shell command a tool call runs, as its update gives it.
fn command_text(update: &Value) -> Option<Arc<str>> {
    let command = update.pointer("/rawInput/command")?;
    let text = match command {
        Value::String(text) => text.clone(),
        Value::Array(words) => words
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let mut end = text.len().min(MAX_COMMAND_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(Arc::from(&text[..end]))
}

/// Whether `command` names the file at `place`, as a script that writes it does.
fn names_file(command: &str, place: &Path) -> bool {
    place
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            command.match_indices(name).any(|(at, _)| {
                let before = command[..at].chars().next_back();
                let after = command[at + name.len()..].chars().next();
                let edge = |c: Option<char>| {
                    c.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.')))
                };
                edge(before) && edge(after)
            })
        })
}
