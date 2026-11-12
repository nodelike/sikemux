//! What happened on a goal and what was decided, written by a model from what the goal's agents
//! really did. The core numbers a bounded list of moments and the model answers with sentences
//! naming the moments behind them; a sentence naming none, or one that does not exist, is
//! dropped. A summary is written only when a goal's page opens and its moments changed, never on
//! a timer, and the page shows the last one until a newer one is ready. Each is kept in a file
//! of its own, so saving the board stays the same size.

mod answer;
mod moments;
mod naming;
mod pulls;

pub(crate) use naming::{chat_titled, name_new, name_unnamed};

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;
use sikemux_view::slice::{GoalSummary, GoalSummaryParams, GoalSummaryView};
use tokio::sync::Semaphore;

use self::moments::Gathered;
use super::board;
use super::changed::Topic;
use super::connection::blocking;
use super::Core;
use crate::agent_catalog::AgentKind;

const FOLDER: &str = "goal-summaries";
/// The short names goals were given, read by every view of a board.
const TITLES: &str = "titles.json";
/// Summaries kept across every project; past this the longest unwritten goes.
const MAX_FILES: usize = 128;
/// Goals read and written at once.
const MAX_AT_ONCE: usize = 2;

type Goal = (String, u64);

pub(crate) struct Summaries {
    folder: Option<PathBuf>,
    /// The goals being looked at now, and whether a model is writing for each.
    busy: Mutex<HashMap<Goal, bool>>,
    turns: Arc<Semaphore>,
    /// The short name each goal was first given, the oldest first.
    titles: Mutex<Vec<(Goal, String)>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Written {
    /// What the moments behind `summary` came to.
    stamp: Option<String>,
    summary: Option<GoalSummary>,
    failed: Option<Failed>,
}

/// An attempt that wrote nothing, not tried again until its moments change or the person asks.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Failed {
    stamp: String,
    why: String,
}

impl Summaries {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let folder = data_dir.map(|dir| dir.join(FOLDER));
        let titles = (folder.as_ref())
            .and_then(|folder| std::fs::read(folder.join(TITLES)).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            folder,
            busy: Mutex::default(),
            turns: Arc::new(Semaphore::new(MAX_AT_ONCE)),
            titles: Mutex::new(titles),
        }
    }

    pub(crate) fn title(&self, project: &str, card: u64) -> Option<String> {
        (self.titles.locked().iter())
            .find(|((known, id), _)| *id == card && known == project)
            .map(|(_, title)| title.clone())
    }

    /// Keeps `title` as `goal`'s short name, unless it has one; whether it took it.
    fn name(&self, goal: &Goal, title: String) -> bool {
        let mut titles = self.titles.locked();
        if titles.iter().any(|(named, _)| named == goal) {
            return false;
        }
        titles.push((goal.clone(), title));
        if titles.len() > MAX_FILES {
            titles.remove(0);
        }
        if let Some(folder) = &self.folder {
            write_whole(&folder.join(TITLES), &*titles);
        }
        true
    }

    fn file(&self, (project, card): &Goal) -> Option<PathBuf> {
        let project = hex(project.as_bytes());
        Some(self.folder.as_ref()?.join(format!("{project}-{card}.json")))
    }

    fn written(&self, goal: &Goal) -> Written {
        (self.file(goal))
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Keeps what was written for `goal`, in a file that appears whole, and lets go of the
    /// oldest once there are too many.
    fn keep(&self, goal: &Goal, written: &Written) {
        let (Some(folder), Some(file)) = (self.folder.as_ref(), self.file(goal)) else {
            return;
        };
        write_whole(&file, written);
        forget_oldest(folder);
    }
}

/// Writes `value` to the file at `path`, which appears whole.
fn write_whole(path: &Path, value: &impl Serialize) {
    let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .map_err(|error| error.to_string())
        .and_then(|()| serde_json::to_vec(value).map_err(|error| error.to_string()))
        .and_then(|bytes| {
            let writing = path.with_extension("writing");
            std::fs::write(&writing, bytes).map_err(|error| error.to_string())?;
            std::fs::rename(writing, path).map_err(|error| error.to_string())
        });
    if let Err(error) = saved {
        eprintln!("sikemux core: a goal's summary was not kept: {error}");
    }
}

fn forget_oldest(folder: &Path) {
    let Ok(files) = std::fs::read_dir(folder) else {
        return;
    };
    let mut files: Vec<_> = files
        .flatten()
        .filter(|file| file.file_name() != TITLES)
        .filter_map(|file| Some((file.metadata().ok()?.modified().ok()?, file.path())))
        .collect();
    if files.len() <= MAX_FILES {
        return;
    }
    files.sort();
    for (_, file) in &files[..files.len() - MAX_FILES] {
        let _ = std::fs::remove_file(file);
    }
}

fn hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest[..12].iter().map(|b| format!("{b:02x}")).collect()
}

fn tell_clients(core: &Core, (project, _): &Goal) {
    core.changed(What::ReadIn(Topic::GoalSummary, Some(project), None));
}

/// `goalSummary`: what is kept for the goal, at once. When asked to refresh it, the goal is read
/// and written about afterwards, and clients hear when that changes what this answers.
pub(crate) async fn read(
    core: &Arc<Core>,
    params: GoalSummaryParams,
) -> Result<GoalSummaryView, String> {
    let goal = (board::project_at(core, &params.project)?, params.card);
    if params.refresh {
        start(core, goal.clone(), params.again);
    }
    let core = core.clone();
    blocking(move || {
        let summaries = core.goal_summaries();
        let Written {
            summary, failed, ..
        } = summaries.written(&goal);
        Ok(GoalSummaryView {
            summary,
            writing: summaries.busy.locked().get(&goal).copied().unwrap_or(false),
            failed: failed.map(|failed| failed.why),
        })
    })
    .await
    .map_err(|error| error.to_string())
}

/// Refreshes `goal` off to the side, unless it is being refreshed already.
fn start(core: &Arc<Core>, goal: Goal, again: bool) {
    if core.goal_summaries().busy.locked().contains_key(&goal) {
        return;
    }
    core.goal_summaries()
        .busy
        .locked()
        .insert(goal.clone(), false);
    let core = core.clone();
    tokio::spawn(async move {
        let turn = core.goal_summaries().turns.clone().acquire_owned().await;
        let (reading, asked) = (core.clone(), goal.clone());
        let gathered = blocking(move || Ok(moments::gather(&reading, &asked.0, asked.1))).await;
        let done = match gathered {
            Ok(Some(mut gathered)) => {
                pulls::add(&core, &goal.0, &mut gathered).await;
                let (working, reading) = (core.clone(), goal.clone());
                blocking(move || {
                    refresh(&working, &reading, &gathered, again);
                    Ok(())
                })
                .await
            }
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        drop(turn);
        if let Err(error) = done {
            eprintln!("sikemux core: a goal's summary stopped: {error}");
        }
        let wrote = core.goal_summaries().busy.locked().remove(&goal) == Some(true);
        if wrote {
            tell_clients(&core, &goal);
        }
    });
}

/// When the goal's moments changed, or `again`, has a model write about them.
fn refresh(core: &Core, goal: &Goal, gathered: &Gathered, again: bool) {
    if gathered.moments.is_empty() && gathered.left_out.is_empty() {
        return;
    }
    let stamp = hex(answer::fingerprint(gathered).as_bytes());
    let mut written = core.goal_summaries().written(goal);
    let tried = written.stamp.as_deref() == Some(stamp.as_str())
        || written
            .failed
            .as_ref()
            .is_some_and(|failed| failed.stamp == stamp);
    if tried && !again {
        return;
    }
    core.goal_summaries()
        .busy
        .locked()
        .insert(goal.clone(), true);
    tell_clients(core, goal);
    match write(core, gathered) {
        Ok((summary, title)) => {
            if title.is_some_and(|title| core.goal_summaries().name(goal, title)) {
                core.changed(What::ReadIn(Topic::Boards, Some(&goal.0), None));
            }
            written.summary = Some(summary);
            written.stamp = Some(stamp);
            written.failed = None;
        }
        Err(why) => {
            eprintln!("sikemux core: a goal's summary was not written: {why}");
            written.failed = Some(Failed { stamp, why });
        }
    }
    core.goal_summaries().keep(goal, &written);
}

/// What a model wrote about `gathered` and the short name it gave the goal, or why there is
/// nothing to show.
fn write(core: &Core, gathered: &Gathered) -> Result<(GoalSummary, Option<String>), String> {
    if gathered.moments.is_empty() {
        return Err("Sikemux couldn't read this goal's agents' sessions".into());
    }
    let (agent, account) = gathered.asked.as_ref().ok_or_else(|| {
        format!(
            "{} can't be asked for a summary, and its chats are not sent to another vendor",
            gathered.lead_label,
        )
    })?;
    let said = ask(core, *agent, account.as_deref(), &answer::prompt(gathered))?;
    answer::read(&said, gathered)
}

/// What `agent`'s CLI on `account` answers `prompt` with, asked once.
fn ask(
    core: &Core,
    agent: AgentKind,
    account: Option<&str>,
    prompt: &str,
) -> Result<String, String> {
    let ask = agent
        .ops()
        .ask_once()
        .ok_or("This agent can't be asked once")?;
    let model = (agent.entry().commit_models.first()).ok_or("This agent names no model to ask")?;
    let asked_in = (core.goal_summaries().folder.clone()).unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&asked_in).map_err(|error| error.to_string())?;
    ask(&asked_in.to_string_lossy(), model, prompt, account)
}
