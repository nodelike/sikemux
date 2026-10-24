//! Which agent changed which lines of which file, turn by turn: every edit a
//! chat agent's tools made, as its driver reported it, and every change made
//! outside them while agents worked, by the command or terminal agent most
//! likely behind it. Kept in `edits/` in the data folder, one file of JSON
//! lines per project, newest last. Paths and lines only; what changed files
//! held goes beside it, in the project's folder, until they are committed.

pub(crate) mod commit;
pub(crate) mod footer;
pub(crate) mod generate;
pub(crate) mod guard;
mod land;
mod lines;
mod named;
pub(crate) mod note;
mod outside;
mod owners;
mod prior;
mod push;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::chat::Chat;
use super::names::project_of;
use super::Core;
pub(crate) use lines::{changed, hunk, replaced, Lines, Replaced};
pub(crate) use owners::{authors, before, commit_lines, git, owners_at, CommitLines, Owners};

const FOLDER: &str = "edits";
const MAX_PER_PROJECT: usize = 5000;
/// Tool calls a chat may have open at once before the oldest is let go of.
const MAX_OPEN_CALLS: usize = 32;
/// Files bigger than this are not read to find where an edit landed.
const MAX_READ_BYTES: u64 = 16 * 1024 * 1024;

/// One file one tool call, or one change outside the edit tools, changed.
/// With no agent, the change is the person's, unless it is `shared`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Edit {
    pub at: u64,
    pub agent_id: String,
    /// The agents at work when no one of them could have made the change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub cli: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The run of the chat whose history holds the turn.
    pub feed: String,
    /// Where the turn starts in that history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    pub tool_call_id: String,
    pub path: String,
    /// Empty when the whole file changed or where is not known.
    pub lines: Lines,
    /// How many lines the file had just after, when it could be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// What a tool call says it changed in one file: the lines, or the text
/// before and after, whole or only the part that changed.
#[derive(Debug)]
pub(crate) enum Change {
    Lines(Lines),
    Text { old: Option<String>, new: String },
}

/// Each file a tool call changed, by path.
pub(crate) type Changes = Vec<(String, Change)>;

/// The edits of a chat's tool calls that have not finished yet.
#[derive(Default)]
pub(crate) struct Open(Mutex<VecDeque<(String, Changes)>>);

impl Open {
    /// The files a tool call changed once `update` says it completed. An
    /// update's diffs stand for everything the call changes so far.
    pub(crate) fn note(&self, update: &Value) -> Option<(String, Changes)> {
        if !matches!(
            update.get("sessionUpdate").and_then(Value::as_str),
            Some("tool_call" | "tool_call_update")
        ) {
            return None;
        }
        let id = update.get("toolCallId")?.as_str()?;
        let mut changes = reported(update);
        let status = update.get("status").and_then(Value::as_str);
        if changes.is_empty() && !matches!(status, Some("completed" | "failed")) {
            return None;
        }
        let mut open = self.0.locked();
        let earlier = open
            .iter()
            .position(|(open_id, _)| open_id == id)
            .and_then(|at| open.remove(at))
            .map(|(_, earlier)| earlier);
        match status {
            Some("failed") => None,
            Some("completed") => {
                if changes.is_empty() {
                    changes = earlier?;
                }
                Some((id.to_owned(), changes))
            }
            _ => {
                if open.len() >= MAX_OPEN_CALLS {
                    open.pop_front();
                }
                open.push_back((id.to_owned(), changes));
                None
            }
        }
    }
}

/// The changes `update` reports: lines the driver worked out itself, else
/// its diffs.
fn reported(update: &Value) -> Changes {
    if let Some(edited) = update
        .pointer("/_meta/sikemux/edited")
        .and_then(Value::as_array)
    {
        return edited
            .iter()
            .filter_map(|file| {
                let path = file.get("path")?.as_str()?.to_owned();
                let lines = serde_json::from_value(file.get("lines")?.clone()).ok()?;
                Some((path, Change::Lines(lines)))
            })
            .collect();
    }
    update
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("diff"))
        .filter_map(|block| {
            let path = block.get("path")?.as_str()?.to_owned();
            let new = block.get("newText")?.as_str()?.to_owned();
            let old = block
                .get("oldText")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some((path, Change::Text { old, new }))
        })
        .collect()
}

/// A tool call that finished, waiting to be written down.
struct Done {
    at: u64,
    agent_id: String,
    cli: String,
    model: Option<String>,
    cwd: PathBuf,
    feed: String,
    turn: Option<u64>,
    tool_call_id: String,
    changes: Changes,
}

#[derive(Default)]
struct Log {
    edits: VecDeque<Edit>,
    lines_on_disk: usize,
}

impl Log {
    fn trim(&mut self) {
        let excess = self.edits.len().saturating_sub(MAX_PER_PROJECT);
        self.edits.drain(..excess);
    }
}

fn json_lines<'a>(edits: impl IntoIterator<Item = &'a Edit>) -> String {
    edits
        .into_iter()
        .filter_map(|edit| serde_json::to_string(edit).ok())
        .map(|line| line + "\n")
        .collect()
}

struct Store {
    folder: Option<PathBuf>,
    logs: Mutex<HashMap<String, Log>>,
    priors: Mutex<HashMap<String, prior::Priors>>,
    outside: outside::Outside,
}

enum Job {
    Done(Done),
    /// An agent is at work since `at`, running a shell command from then
    /// when `command`.
    Working {
        agent_id: String,
        at: u64,
        command: bool,
    },
    /// The person saved a file in Sikemux's editor.
    Saved(PathBuf),
    /// A file changed in a watched checkout.
    Changed,
}

pub(crate) struct Edits {
    store: Arc<Store>,
    jobs: mpsc::Sender<Job>,
}

/// Which of a project's edits to read; each filter left out matches all.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Filter {
    pub project: PathBuf,
    agent_id: Option<String>,
    feed: Option<String>,
    turn: Option<u64>,
    path: Option<String>,
}

impl Edits {
    pub(crate) fn new(data_dir: Option<&Path>, core: Weak<Core>) -> Self {
        let store = Arc::new(Store {
            folder: data_dir.map(|dir| dir.join(FOLDER)),
            logs: Mutex::new(HashMap::new()),
            priors: Mutex::new(HashMap::new()),
            outside: outside::Outside::default(),
        });
        let (jobs, queue) = mpsc::channel();
        let writer = store.clone();
        let spawned = std::thread::Builder::new()
            .name("sikemux-edits".into())
            .spawn(move || writer.write(&queue, &core));
        if let Err(error) = spawned {
            eprintln!("sikemux core: edits will not be kept: {error}");
        }
        Self { store, jobs }
    }

    /// `agent_id` is at work, so changes in its checkout are watched.
    pub(crate) fn working(&self, agent_id: &str) {
        self.send_working(agent_id, false);
    }

    /// `agent_id` begins a shell command, so the changes in its checkout
    /// before it are told apart from the command's.
    pub(crate) fn command_began(&self, agent_id: &str) {
        self.send_working(agent_id, true);
    }

    fn send_working(&self, agent_id: &str, command: bool) {
        let _ = self.jobs.send(Job::Working {
            agent_id: agent_id.to_owned(),
            at: epoch_ms(),
            command,
        });
    }

    /// The top folder of the checkout `agent_id` works in, none outside one.
    pub(crate) fn checkout_of(&self, core: &Core, agent_id: &str) -> Option<PathBuf> {
        self.store.outside.top_of(core, agent_id)
    }

    /// The person saved `path` in Sikemux's editor.
    pub(crate) fn saved(&self, path: PathBuf) {
        let _ = self.jobs.send(Job::Saved(path));
    }

    fn wake(&self) {
        let _ = self.jobs.send(Job::Changed);
    }

    /// Notes what `chat`'s tool call `tool_call_id` changed.
    pub(crate) fn record(&self, chat: &Chat, tool_call_id: String, changes: Changes) {
        let done = Done {
            at: epoch_ms(),
            agent_id: chat.agent_id().to_owned(),
            cli: chat.provider().to_owned(),
            model: chat.current_launch().model,
            cwd: chat.launch.cwd.clone(),
            feed: chat.feed.id().to_owned(),
            turn: chat.feed.turn_start(),
            tool_call_id,
            changes,
        };
        let _ = self.jobs.send(Job::Done(done));
    }

    /// The edits in a project that `filter` matches, oldest first.
    pub(crate) fn read(&self, project: &str, filter: &Filter) -> Vec<Edit> {
        self.matching(project, |edit| {
            filter
                .agent_id
                .as_ref()
                .is_none_or(|id| *id == edit.agent_id)
                && filter.feed.as_ref().is_none_or(|feed| *feed == edit.feed)
                && filter.turn.is_none_or(|turn| Some(turn) == edit.turn)
                && filter.path.as_ref().is_none_or(|path| *path == edit.path)
        })
    }

    /// The edits in a project to `path`, oldest first, up to but not
    /// including `until` when given.
    pub(crate) fn of_file(&self, project: &str, path: &str, until: Option<&Edit>) -> Vec<Edit> {
        let mut edits = self.matching(project, |edit| edit.path == path);
        if let Some(until) = until {
            let at = edits.iter().position(|edit| {
                edit.at == until.at
                    && edit.agent_id == until.agent_id
                    && edit.tool_call_id == until.tool_call_id
            });
            edits.truncate(at.unwrap_or(edits.len()));
        }
        edits
    }

    /// The edits in a project to any of `paths`, oldest first.
    fn touching(&self, project: &str, paths: &HashSet<String>) -> Vec<Edit> {
        self.matching(project, |edit| paths.contains(&edit.path))
    }

    fn matching(&self, project: &str, keep: impl Fn(&Edit) -> bool) -> Vec<Edit> {
        self.store.matching(project, keep)
    }
}

impl Store {
    /// The name a project's record goes by, and the folder beside it.
    fn stem(&self, project: &str) -> Option<PathBuf> {
        let digest = hex::encode(Sha256::digest(project.as_bytes()));
        let stem = digest.get(..16).unwrap_or(&digest);
        self.folder.as_ref().map(|folder| folder.join(stem))
    }

    fn file(&self, project: &str) -> Option<PathBuf> {
        self.stem(project).map(|stem| stem.with_extension("jsonl"))
    }

    /// Runs `work` on what is kept of a project's changed files, saving it after.
    fn priors<T>(&self, project: &str, work: impl FnOnce(&mut prior::Priors) -> T) -> T {
        let mut priors = self.priors.locked();
        let priors = priors
            .entry(project.to_owned())
            .or_insert_with(|| prior::Priors::load(self.stem(project)));
        let done = work(priors);
        priors.save();
        done
    }

    fn matching(&self, project: &str, keep: impl Fn(&Edit) -> bool) -> Vec<Edit> {
        let mut logs = self.logs.locked();
        let kept =
            logs.contains_key(project) || self.file(project).is_some_and(|file| file.exists());
        if !kept {
            return Vec::new();
        }
        self.log(&mut logs, project)
            .edits
            .iter()
            .filter(|edit| keep(edit))
            .cloned()
            .collect()
    }

    /// A project's log, read from its file the first time it is asked for.
    fn log<'a>(&self, logs: &'a mut HashMap<String, Log>, project: &str) -> &'a mut Log {
        logs.entry(project.to_owned()).or_insert_with(|| {
            let Some(text) = self
                .file(project)
                .and_then(|file| std::fs::read_to_string(file).ok())
            else {
                return Log::default();
            };
            let mut log = Log::default();
            for line in text.lines() {
                log.lines_on_disk += 1;
                if let Ok(edit) = serde_json::from_str(line) {
                    log.edits.push_back(edit);
                }
            }
            log.trim();
            log
        })
    }

    /// Writes down tool calls as they finish, all that wait at once in one go,
    /// and the changes outside the edit tools once their grace has passed.
    fn write(&self, queue: &mpsc::Receiver<Job>, core: &Weak<Core>) {
        let mut due: Option<u64> = None;
        loop {
            let first = match due {
                Some(due) => {
                    let wait = Duration::from_millis(due.saturating_sub(epoch_ms()));
                    match queue.recv_timeout(wait) {
                        Ok(job) => Some(job),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match queue.recv() {
                    Ok(job) => Some(job),
                    Err(_) => return,
                },
            };
            let Some(core) = core.upgrade() else { return };
            let mut done = Vec::new();
            for job in first.into_iter().chain(queue.try_iter()) {
                match job {
                    Job::Done(finished) => done.push(finished),
                    Job::Working {
                        agent_id,
                        at,
                        command,
                    } => self.outside.working(&core, self, &agent_id, at, command),
                    Job::Saved(path) => self.outside.saved(&core, self, &path),
                    Job::Changed => {}
                }
            }
            if !done.is_empty() {
                self.finished(&core, done);
            }
            due = self.outside.settle(&core, self);
        }
    }

    fn finished(&self, core: &Arc<Core>, finished: Vec<Done>) {
        let projects = core.workspaces.project_paths();
        let mut batch: HashMap<String, Vec<Edit>> = HashMap::new();
        for mut done in finished {
            let named = core.names.get(&done.agent_id);
            let project = match &named {
                Some(named) => named.project.clone(),
                None => project_of(&done.cwd, &projects),
            };
            core.collisions
                .wrote(core, &done.agent_id, &done.cwd, &mut done.changes);
            batch
                .entry(project)
                .or_default()
                .extend(edits(done, named.map(|named| named.name)));
        }
        for (project, edits) in batch {
            self.append(&project, &edits);
            for edit in &edits {
                let path = Path::new(&edit.path);
                if let Some((_, project)) = self.outside.checkout_of(path) {
                    let text = prior::text_of(path);
                    self.priors(&project, |priors| {
                        priors.keep(
                            &edit.path,
                            text.as_ref().map(|(text, meta)| (text.as_str(), meta)),
                        );
                    });
                }
            }
            core.collisions.landed(core, &project, &edits);
        }
    }

    fn append(&self, project: &str, edits: &[Edit]) {
        let mut logs = self.logs.locked();
        let log = self.log(&mut logs, project);
        let lines = json_lines(edits);
        log.lines_on_disk += edits.len();
        log.edits.extend(edits.iter().cloned());
        log.trim();
        let Some(file) = self.file(project) else {
            return;
        };
        let written = if log.lines_on_disk > 2 * MAX_PER_PROJECT {
            log.lines_on_disk = log.edits.len();
            let all = json_lines(&log.edits);
            sikemux_process::files::write_atomic(&file, all.as_bytes(), true)
        } else {
            file.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .mode(0o600)
                        .open(&file)
                })
                .and_then(|mut opened| opened.write_all(lines.as_bytes()))
        };
        if let Err(error) = written {
            eprintln!("sikemux core: edits were not saved: {error}");
        }
    }
}

/// A finished tool call's edits, one per file, with the lines worked out.
fn edits(done: Done, name: Option<String>) -> impl Iterator<Item = Edit> {
    let Done {
        at,
        agent_id,
        cli,
        model,
        cwd,
        feed,
        turn,
        tool_call_id,
        changes,
    } = done;
    changes.into_iter().map(move |(path, change)| {
        let path = cwd.join(path);
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let text = read_text(&path);
        let lines = match change {
            Change::Lines(lines) => lines,
            Change::Text { old, new } => text
                .as_deref()
                .map(|text| located(text, old.as_deref(), &new))
                .unwrap_or_default(),
        };
        Edit {
            at,
            agent_id: agent_id.clone(),
            shared: Vec::new(),
            name: name.clone(),
            cli: cli.clone(),
            model: model.clone(),
            feed: feed.clone(),
            turn,
            tool_call_id: tool_call_id.clone(),
            path: path.to_string_lossy().into_owned(),
            lines,
            total: text.as_deref().map(line_count),
        }
    })
}

fn read_text(path: &Path) -> Option<String> {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.len() <= MAX_READ_BYTES)
        .and_then(|_| std::fs::read_to_string(path).ok())
}

pub(crate) fn line_count(text: &str) -> u64 {
    let breaks = text.matches('\n').count() as u64;
    breaks + u64::from(!text.is_empty() && !text.ends_with('\n'))
}

/// The lines an edit from `old` to `new` changed, found in the file as it is
/// now, since an agent may report only the part of the file it changed.
fn located(file: &str, old: Option<&str>, new: &str) -> Lines {
    if new.is_empty() {
        return Vec::new();
    }
    let Some(before) = file.find(new).map(|at| &file[..at]) else {
        return Vec::new();
    };
    let first = before.matches('\n').count() as u64 + 1;
    lines::changed(old.unwrap_or_default(), new, first)
}
