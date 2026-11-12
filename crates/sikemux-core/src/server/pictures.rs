//! What a turn's change shows: the page its checkout serves, which the core takes itself at a
//! phone's, a tablet's and a desktop's width in a view off screen, so the person's own tab never
//! moves. A set is kept beside the last pictures of the same page, so a client shows before and
//! after. The pictures are shown, never proven. The core serves an agent's own checkout, starting
//! the project's process there when nothing serves it, but only a process `sikemux.json` names.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::shown::{PageShown, Picture, ResultRow, TurnShown, WidthShown};
use sikemux_view::turn::{TurnPhase, TurnState, WorkAt};

use super::changed::What;
use super::harness::forward;
use super::kept::Kept;
use super::ports::{self, PortOwner};
use super::Core;
use crate::cli::protocol::{HarnessRequest, BROWSER_PICTURE_METHOD};
use crate::tasks::config::{self, ConfigLoad, Stated};
use crate::tasks::detect;

const FILE: &str = "pictures.json";
const FOLDER: &str = "pictures";
/// The widths a set is taken at, by the browser's names for them: 390, 820 and 1440.
const WIDTHS: [&str; 3] = ["mobile", "tablet", "desktop"];
/// Sets kept per project. Past it the oldest set of a page that has a newer one goes first, so
/// each page keeps its latest; then the oldest of all.
const KEPT_PER_PROJECT: usize = 60;
/// The turns of one chat whose pages its head carries.
const SHOWN_TURNS: usize = 10;
/// How long a process the core started may take to listen.
const SERVE_WAIT: Duration = Duration::from_secs(90);
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// How long the page may take to answer before it is taken.
const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// The endings of the files a browser draws a page from.
const PAGE_FILES: [&str; 12] = [
    "tsx", "jsx", "vue", "svelte", "astro", "html", "htm", "css", "scss", "sass", "less", "styl",
];

/// How the titles start of what a server shows in place of a page: a listing of its files, or
/// an error.
const NOT_PAGES: [&str; 5] = [
    "Directory listing for",
    "Index of /",
    "Error response",
    "404 Not Found",
    "502 Bad Gateway",
];

pub(crate) fn is_page_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ending| ending.to_str())
        .is_some_and(|ending| PAGE_FILES.contains(&ending.to_ascii_lowercase().as_str()))
}

/// One page taken at each width, after one turn of one agent.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Set {
    project: String,
    agent: String,
    /// When the turn began, which names it.
    turn: u64,
    page: String,
    at: u64,
    shots: Vec<Shot>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Shot {
    width: u32,
    path: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    sets: Vec<Set>,
}

pub(crate) struct Pictures {
    kept: Kept,
    folder: Option<PathBuf>,
    state: Mutex<Saved>,
    /// The turns being taken now, by agent and turn.
    taking: Mutex<HashSet<(String, u64)>>,
}

impl Pictures {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let state = Mutex::new(kept.load());
        Self {
            kept,
            folder: data_dir.map(|dir| dir.join(FOLDER)),
            state,
            taking: Mutex::default(),
        }
    }

    fn taken(&self, agent: &str, turn: u64) -> bool {
        (self.state.locked().sets.iter()).any(|set| set.agent == agent && set.turn == turn)
    }

    /// Keeps `set`, letting go of the project's oldest past the bound, and their files.
    fn keep(&self, set: Set) {
        let mut state = self.state.locked();
        let project = set.project.clone();
        state.sets.push(set);
        let mut gone = Vec::new();
        while state
            .sets
            .iter()
            .filter(|set| set.project == project)
            .count()
            > KEPT_PER_PROJECT
        {
            let ours = |set: &&Set| set.project == project;
            let superseded = (state.sets.iter().enumerate())
                .filter(|(_, set)| ours(set))
                .find(|(_, old)| {
                    (state.sets.iter()).any(|newer| {
                        newer.project == old.project && newer.page == old.page && newer.at > old.at
                    })
                })
                .map(|(at, _)| at);
            let oldest =
                superseded.or_else(|| state.sets.iter().position(|set| set.project == project));
            let Some(at) = oldest else { break };
            gone.push(state.sets.remove(at));
        }
        self.kept.save_soon(&*state);
        drop(state);
        for shot in gone.iter().flat_map(|set| &set.shots) {
            let _ = std::fs::remove_file(&shot.path);
        }
    }

    /// The pages each of the agent's latest turns changed, oldest first.
    pub(crate) fn turns_of(&self, agent: &str) -> Vec<TurnShown> {
        let state = self.state.locked();
        let mut turns: Vec<u64> = (state.sets.iter())
            .filter(|set| set.agent == agent)
            .map(|set| set.turn)
            .collect();
        turns.sort_unstable();
        turns.dedup();
        let from = turns.len().saturating_sub(SHOWN_TURNS);
        turns[from..]
            .iter()
            .map(|turn| TurnShown {
                turn: *turn,
                pages: pages(&state.sets, agent, *turn),
            })
            .collect()
    }

    /// The pages one turn of the agent's changed.
    pub(crate) fn pages_of(&self, agent: &str, turn: u64) -> Vec<PageShown> {
        pages(&self.state.locked().sets, agent, turn)
    }
}

fn pages(sets: &[Set], agent: &str, turn: u64) -> Vec<PageShown> {
    (sets.iter())
        .filter(|set| set.agent == agent && set.turn == turn)
        .map(|set| {
            let before = (sets.iter())
                .filter(|old| old.project == set.project && old.page == set.page && old.at < set.at)
                .max_by_key(|old| old.at);
            let picture = |set: &Set, width: u32| {
                (set.shots.iter())
                    .find(|shot| shot.width == width)
                    .map(|shot| Picture {
                        path: shot.path.clone(),
                        at: set.at,
                    })
            };
            PageShown {
                page: set.page.clone(),
                widths: (set.shots.iter())
                    .filter_map(|shot| {
                        Some(WidthShown {
                            width: shot.width,
                            after: picture(set, shot.width)?,
                            before: before.and_then(|before| picture(before, shot.width)),
                        })
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The checkout the agent works in: its own, else the project's.
pub(crate) fn checkout(core: &Core, agent_id: &str) -> Option<PathBuf> {
    match core.chats.get(agent_id) {
        Some(chat) => Some(chat.launch.cwd.clone()),
        None => core.workspaces.agent(agent_id).map(|agent| agent.cwd),
    }
}

/// The process `sikemux.json` in `dir` names to serve its page: the one its preview runs, else
/// one named for running.
fn serving_task(dir: &Path) -> Option<String> {
    let ConfigLoad::Valid { config, .. } = config::load(dir) else {
        return None;
    };
    let previewed = (config.preview.as_ref())
        .and_then(Stated::given)
        .and_then(|preview| preview.command.clone());
    let tasks = &config.tasks;
    (tasks.iter())
        .find(|task| previewed.as_deref() == Some(task.command.as_str()))
        .or_else(|| tasks.iter().find(|task| detect::is_run_task(&task.id)))
        .map(|task| task.id.clone())
}

/// Where the agent's checkout at `dir` is served now: a server the agent started, else one
/// running in that checkout, its preview first. Looks at what listens, so off the view's path.
fn served_at(core: &Core, agent_id: &str, dir: &Path) -> Option<String> {
    let listening = ports::listening_now(core);
    let preview = match config::load(dir) {
        ConfigLoad::Valid { config, .. } => config.preview,
        _ => None,
    };
    if matches!(preview, Some(Stated::Off)) {
        return None;
    }
    if let Some(own) = (listening.iter()).find(|listening| listening.owner.started_by(agent_id)) {
        let previewed = (preview.as_ref())
            .and_then(Stated::given)
            .and_then(|preview| preview.url.clone())
            .filter(|url| ports::preview_port(url) == Some(own.port));
        return Some(previewed.unwrap_or_else(|| format!("http://localhost:{}/", own.port)));
    }
    let ports = ports::for_project(listening, dir);
    (ports.iter())
        .find(|port| port.preview)
        .or_else(|| {
            ports.iter().find(|port| {
                matches!(&port.listening.owner, PortOwner::Task { task_id, .. } if detect::is_run_task(task_id))
            })
        })
        .map(|port| port.url.clone())
}

/// Whether the agent's checkout at `dir` can be served: something serves it, or its
/// `sikemux.json` names the process that does. Looks at what listens, so off the view's path.
pub(crate) fn servable(core: &Core, agent_id: &str, dir: &Path) -> bool {
    serving_task(dir).is_some() || served_at(core, agent_id, dir).is_some()
}

/// Where the agent's checkout is served, starting its project's process there when nothing
/// serves it yet and waiting until it listens.
pub(crate) async fn serve(core: &Arc<Core>, agent_id: &str) -> Result<String, String> {
    let dir = checkout(core, agent_id).ok_or("The agent has no checkout to serve")?;
    let looked = {
        let (core, agent_id, dir) = (core.clone(), agent_id.to_owned(), dir.clone());
        tokio::task::spawn_blocking(move || (served_at(&core, &agent_id, &dir), serving_task(&dir)))
            .await
            .map_err(|error| error.to_string())?
    };
    let task = match looked {
        (Some(url), _) => return Ok(url),
        (None, Some(task)) => task,
        (None, None) => {
            return Err(
                "Nothing serves this checkout, and its sikemux.json names no process that does"
                    .into(),
            )
        }
    };
    let dir_text = dir.to_string_lossy().into_owned();
    let execution = super::harness::start_for_agent(core, &dir_text, agent_id, &task)?;
    let until = Instant::now() + SERVE_WAIT;
    loop {
        tokio::time::sleep(LOOK_EVERY).await;
        let run = core
            .harness
            .run_of(&execution)
            .ok_or("The process the core started is gone")?;
        if !run.status.is_active() {
            return Err(run
                .error
                .unwrap_or_else(|| format!("{task} stopped before it served the page")));
        }
        let found = {
            let (core, execution) = (core.clone(), execution.clone());
            tokio::task::spawn_blocking(move || {
                ports::listening_now(&core).into_iter().find(|listening| {
                    matches!(&listening.owner, PortOwner::Task { execution_id, .. } if *execution_id == execution)
                })
            })
            .await
            .map_err(|error| error.to_string())?
        };
        if let Some(listening) = found {
            return Ok(run
                .preview_url
                .filter(|url| ports::preview_port(url) == Some(listening.port))
                .unwrap_or_else(|| format!("http://localhost:{}/", listening.port)));
        }
        if Instant::now() >= until {
            return Err(format!(
                "{task} did not start listening within {} seconds",
                SERVE_WAIT.as_secs()
            ));
        }
    }
}

/// Why what `url` answers is not a page: it did not answer, or it answered with an error or a
/// listing of its files.
async fn not_a_page(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(ANSWER_WAIT)
        .build()
        .ok()?;
    let answered = match client.get(url).send().await {
        Ok(answered) => answered,
        Err(error) => return Some(error.to_string()),
    };
    if !answered.status().is_success() {
        return Some(format!("it answered {}", answered.status()));
    }
    let body = answered.text().await.unwrap_or_default();
    let title = (body.split_once("<title>"))
        .and_then(|(_, rest)| rest.split_once("</title>"))
        .map(|(title, _)| title.trim().to_owned())
        .unwrap_or_default();
    NOT_PAGES
        .iter()
        .any(|start| title.starts_with(start))
        .then(|| format!("it shows \"{title}\" rather than a page"))
}

/// Takes the page the agent's checkout serves at each width, for its turn that began at
/// `turn`, once per turn, off the caller's path.
pub(crate) fn take(core: &Core, agent_id: &str, project: &str, turn: u64) {
    if !core.window.is_open() || core.pictures.taken(agent_id, turn) {
        return;
    }
    if !core
        .pictures
        .taking
        .locked()
        .insert((agent_id.to_owned(), turn))
    {
        return;
    }
    let (Some(core), Ok(runtime)) = (core.arc(), tokio::runtime::Handle::try_current()) else {
        return;
    };
    let (agent, project) = (agent_id.to_owned(), project.to_owned());
    runtime.spawn(async move {
        let started = Instant::now();
        match shoot(&core, &agent, &project, turn).await {
            Ok(set) => {
                eprintln!(
                    "sikemux core: took {} at {} widths in {} ms",
                    set.page,
                    set.shots.len(),
                    started.elapsed().as_millis()
                );
                core.pictures.keep(set);
                core.changed(What::View);
            }
            Err(why) => eprintln!("sikemux core: the turn's page was not taken: {why}"),
        }
        core.pictures.taking.locked().remove(&(agent, turn));
    });
}

async fn shoot(core: &Arc<Core>, agent: &str, project: &str, turn: u64) -> Result<Set, String> {
    let url = serve(core, agent).await?;
    if let Some(why) = not_a_page(&url).await {
        return Err(format!("{url}: {why}"));
    }
    let folder = core
        .pictures
        .folder
        .clone()
        .ok_or("This core keeps no files")?;
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let at = epoch_ms();
    let mut shots = Vec::new();
    for width in WIDTHS {
        let request = HarnessRequest {
            id: format!("picture-{turn}-{width}"),
            project: project.to_owned(),
            agent_id: Some(agent.to_owned()),
            method: BROWSER_PICTURE_METHOD.to_owned(),
            params: json!({ "url": url, "width": width }),
        };
        let taken = forward(core, request).await?;
        let bytes = (taken["data"].as_str())
            .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
            .ok_or("The window sent no picture")?;
        let width = taken["width"]
            .as_u64()
            .and_then(|width| u32::try_from(width).ok())
            .unwrap_or_default();
        let path = folder.join(format!("{agent}-{turn}-{at}-w{width}.jpg"));
        sikemux_process::files::write_atomic(&path, &bytes, true)
            .map_err(|error| error.to_string())?;
        shots.push(Shot {
            width,
            path: path.to_string_lossy().into_owned(),
        });
    }
    let page = url::Url::parse(&url).map_or_else(|_| "/".to_owned(), |url| url.path().to_owned());
    Ok(Set {
        project: project.to_owned(),
        agent: agent.to_owned(),
        turn,
        page,
        at,
        shots,
    })
}

/// A row for each of `lead`'s workers whose turn finished, oldest first.
pub(crate) fn results(
    core: &Core,
    lead: &str,
    leads: &HashMap<String, String>,
    turns: &HashMap<String, TurnState>,
) -> Vec<ResultRow> {
    let mut rows: Vec<ResultRow> = (leads.iter())
        .filter(|(_, of)| of.as_str() == lead)
        .filter_map(|(worker, _)| {
            let turn = turns
                .get(worker)
                .filter(|turn| turn.state == TurnPhase::Done)?;
            let done = turn.done.clone()?;
            let started = core.inbox.ending(worker).map(|ended| ended.started_at);
            Some(ResultRow {
                agent_id: worker.clone(),
                name: core.names.name(worker),
                title: core.workspaces.title(worker).unwrap_or_default(),
                at: turn.since,
                undo: matches!(
                    done.changes,
                    WorkAt::Landed | WorkAt::Staged | WorkAt::Shipped
                ),
                pages: started
                    .map(|turn| core.pictures.pages_of(worker, turn))
                    .unwrap_or_default(),
                done,
            })
        })
        .collect();
    rows.sort_by_key(|row| row.at);
    rows
}
