//! What a turn's change shows: the page its checkout serves, which the core takes itself at a
//! phone's, a tablet's and a desktop's width in a view off screen that shares nothing with the
//! desk, so the person's own tab never moves and nothing signed in is seen. A set is kept beside
//! the last pictures of the same page, so a client shows before and after. The pictures are
//! shown, never proven. The core takes only a page something already serves on this machine;
//! only the person's Try it starts the project's process, as one they can see and stop.

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
use super::connection::blocking;
use super::harness::forward;
use super::kept::Kept;
use super::ports::{self, PortOwner};
use super::{Core, CoreResult};
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
/// How long a process Try it started may take to listen.
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
    /// Its file's name in the pictures folder, which clients fetch it by.
    id: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    sets: Vec<Set>,
}

pub(crate) struct Pictures {
    kept: Kept,
    folder: Option<PathBuf>,
    state: Mutex<Saved>,
    /// What each agent's turns showed, as clients are sent it, worked out once per change.
    shown: Mutex<HashMap<String, Vec<TurnShown>>>,
    /// The turns being taken now, by agent and turn.
    taking: Mutex<HashSet<(String, u64)>>,
}

impl Pictures {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let saved: Saved = kept.load();
        let folder = data_dir.map(|dir| dir.join(FOLDER));
        if let Some(folder) = &folder {
            sweep(folder, &saved);
        }
        let shown = shown_by_agent(&saved.sets);
        Self {
            kept,
            folder,
            state: Mutex::new(saved),
            shown: Mutex::new(shown),
            taking: Mutex::default(),
        }
    }

    fn taken(&self, agent: &str, turn: u64) -> bool {
        (self.state.locked().sets.iter()).any(|set| set.agent == agent && set.turn == turn)
    }

    /// Keeps `set`, letting go of the project's oldest past the bound.
    fn keep(&self, set: Set) {
        let project = set.project.clone();
        self.change(|sets| {
            sets.push(set);
            let mut gone = Vec::new();
            while sets.iter().filter(|set| set.project == project).count() > KEPT_PER_PROJECT {
                let superseded = (sets.iter().enumerate())
                    .filter(|(_, set)| set.project == project)
                    .find(|(_, old)| newer_of_page(sets, old))
                    .map(|(at, _)| at);
                let oldest =
                    superseded.or_else(|| sets.iter().position(|set| set.project == project));
                let Some(at) = oldest else { break };
                gone.push(sets.remove(at));
            }
            gone
        });
    }

    /// Lets go of the sets of projects no longer in the workspace, and keeps of an agent no longer
    /// open only its latest set of each page.
    fn tidy(&self, open: &HashSet<String>, projects: &HashSet<String>) {
        self.change(|sets| {
            let mut gone = Vec::new();
            let mut at = 0;
            while at < sets.len() {
                let set = &sets[at];
                let closed_and_old = !open.contains(&set.agent)
                    && (sets.iter()).any(|newer| {
                        newer.agent == set.agent
                            && newer.project == set.project
                            && newer.page == set.page
                            && newer.at > set.at
                    });
                if !projects.contains(&set.project) || closed_and_old {
                    gone.push(sets.remove(at));
                } else {
                    at += 1;
                }
            }
            gone
        });
    }

    /// Changes the sets kept, saving them, removing the files of those `change` lets go of and
    /// working out again what clients are sent.
    fn change(&self, change: impl FnOnce(&mut Vec<Set>) -> Vec<Set>) {
        let mut state = self.state.locked();
        let gone = change(&mut state.sets);
        self.kept.save_soon(&*state);
        let shown = shown_by_agent(&state.sets);
        drop(state);
        *self.shown.locked() = shown;
        let Some(folder) = &self.folder else { return };
        for shot in gone.iter().flat_map(|set| &set.shots) {
            let _ = std::fs::remove_file(folder.join(&shot.id));
        }
    }

    /// The pages each of the agent's latest turns changed, oldest first.
    pub(crate) fn turns_of(&self, agent: &str) -> Vec<TurnShown> {
        self.shown.locked().get(agent).cloned().unwrap_or_default()
    }

    /// The pages one turn of the agent's changed.
    fn pages_of(&self, agent: &str, turn: u64) -> Vec<PageShown> {
        (self.shown.locked().get(agent))
            .and_then(|turns| turns.iter().find(|shown| shown.turn == turn))
            .map(|shown| shown.pages.clone())
            .unwrap_or_default()
    }

    /// The file a picture's id names, when it is a plain file name the core keeps.
    fn file_of(&self, id: &str) -> Option<PathBuf> {
        let plain = !id.is_empty()
            && !id.starts_with('.')
            && !id.contains(['/', '\\'])
            && Path::new(id).file_name() == Some(std::ffi::OsStr::new(id));
        if !plain {
            return None;
        }
        let folder = self.folder.as_ref()?;
        let known = (self.state.locked().sets.iter())
            .flat_map(|set| &set.shots)
            .any(|shot| shot.id == id);
        known.then(|| folder.join(id))
    }
}

/// Whether a set of the same page was taken after `old`.
fn newer_of_page(sets: &[Set], old: &Set) -> bool {
    (sets.iter())
        .any(|newer| newer.project == old.project && newer.page == old.page && newer.at > old.at)
}

/// Removes the files in the pictures folder its index no longer lists, as a crash between
/// writing one and keeping its set leaves.
fn sweep(folder: &Path, saved: &Saved) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let listed: HashSet<&str> = (saved.sets.iter())
        .flat_map(|set| &set.shots)
        .map(|shot| shot.id.as_str())
        .collect();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_str().is_some_and(|name| listed.contains(name)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// What every agent's latest turns showed, each page beside the set of it taken before.
fn shown_by_agent(sets: &[Set]) -> HashMap<String, Vec<TurnShown>> {
    let mut by_page: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (at, set) in sets.iter().enumerate() {
        by_page
            .entry((&set.project, &set.page))
            .or_default()
            .push(at);
    }
    let mut before: HashMap<usize, usize> = HashMap::new();
    for taken in by_page.values_mut() {
        taken.sort_by_key(|at| sets[*at].at);
        for pair in taken.windows(2) {
            before.insert(pair[1], pair[0]);
        }
    }
    let picture = |set: &Set, width: u32| {
        (set.shots.iter())
            .find(|shot| shot.width == width)
            .map(|shot| Picture {
                id: shot.id.clone(),
                at: set.at,
            })
    };
    let mut turns: HashMap<String, Vec<(u64, PageShown)>> = HashMap::new();
    for (at, set) in sets.iter().enumerate() {
        let earlier = before.get(&at).map(|earlier| &sets[*earlier]);
        let page = PageShown {
            page: set.page.clone(),
            widths: (set.shots.iter())
                .filter_map(|shot| {
                    Some(WidthShown {
                        width: shot.width,
                        after: picture(set, shot.width)?,
                        before: earlier.and_then(|earlier| picture(earlier, shot.width)),
                    })
                })
                .collect(),
        };
        turns
            .entry(set.agent.clone())
            .or_default()
            .push((set.turn, page));
    }
    (turns.into_iter())
        .map(|(agent, mut pages)| {
            pages.sort_by_key(|(turn, _)| *turn);
            let mut shown: Vec<TurnShown> = Vec::new();
            for (turn, page) in pages {
                match shown.last_mut() {
                    Some(last) if last.turn == turn => last.pages.push(page),
                    _ => shown.push(TurnShown {
                        turn,
                        pages: vec![page],
                    }),
                }
            }
            let from = shown.len().saturating_sub(SHOWN_TURNS);
            (agent, shown.split_off(from))
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

/// Where the checkout at `dir` is served now, on this machine: the port its preview names, or
/// one its process for serving or running holds, held by a process of that checkout. Only the
/// preview's path is taken from `sikemux.json`, never its host. Looks at what listens, so off
/// the view's path.
fn served_at(core: &Core, dir: &Path) -> Option<String> {
    let preview = match config::load(dir) {
        ConfigLoad::Valid { config, .. } => config.preview,
        _ => None,
    };
    if matches!(preview, Some(Stated::Off)) {
        return None;
    }
    let previewed = (preview.as_ref())
        .and_then(Stated::given)
        .and_then(|preview| preview.url.clone())
        .and_then(|url| Some((ports::preview_port(&url)?, url::Url::parse(&url).ok()?)));
    let serving = serving_task(dir);
    let ports = ports::for_project(ports::listening_now(core), dir);
    let port = (ports.iter()).find_map(|port| {
        let held = port.listening.port;
        let runs = matches!(&port.listening.owner, PortOwner::Task { task_id, .. }
            if detect::is_run_task(task_id) || serving.as_deref() == Some(task_id.as_str()));
        let previews = previewed.as_ref().is_some_and(|(at, _)| *at == held);
        (previews || runs).then_some(held)
    })?;
    let path = (previewed.as_ref())
        .filter(|(at, _)| *at == port)
        .map_or("/", |(_, url)| url.path());
    Some(format!("http://localhost:{port}{path}"))
}

/// Whether the agent's checkout can be served: something serves it, or its `sikemux.json`
/// names the process that does. Looks at what listens, so off the view's path.
pub(crate) fn servable(core: &Core, dir: &Path) -> bool {
    serving_task(dir).is_some() || served_at(core, dir).is_some()
}

/// Where the agent's checkout is served, for the person's Try it: starting the project's process
/// there when nothing serves it yet, as a run of the person's that they see and can stop, one per
/// checkout and reused, and waiting until it listens.
pub(crate) async fn serve(core: &Arc<Core>, agent_id: &str) -> Result<String, String> {
    let dir = checkout(core, agent_id).ok_or("The agent has no checkout to serve")?;
    let looked = {
        let (core, dir) = (core.clone(), dir.clone());
        tokio::task::spawn_blocking(move || (served_at(&core, &dir), serving_task(&dir)))
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
    let execution = super::harness::start_served(core, &dir_text, &task)?;
    let until = Instant::now() + SERVE_WAIT;
    loop {
        tokio::time::sleep(LOOK_EVERY).await;
        let run = (core.harness.run_of(&execution)).ok_or("The process Try it started is gone")?;
        if !run.status.is_active() {
            return Err(run
                .error
                .unwrap_or_else(|| format!("{task} stopped before it served the page")));
        }
        let found = {
            let (core, dir) = (core.clone(), dir.clone());
            tokio::task::spawn_blocking(move || served_at(&core, &dir))
                .await
                .map_err(|error| error.to_string())?
        };
        if let Some(url) = found {
            return Ok(url);
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
/// listing of its files. Redirects are not followed.
async fn not_a_page(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(ANSWER_WAIT)
        .redirect(reqwest::redirect::Policy::none())
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

/// Takes the page the agent's checkout serves at each width, for its turn that began at `turn`
/// with its work at `head`, once per turn, off the caller's path; nothing when nothing serves it.
pub(crate) fn take(core: &Core, agent_id: &str, project: &str, turn: u64, head: Option<String>) {
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
        match shoot(&core, &agent, &project, turn, head).await {
            Ok(Some(set)) => {
                eprintln!(
                    "sikemux core: took {} at {} widths in {} ms",
                    set.page,
                    set.shots.len(),
                    started.elapsed().as_millis()
                );
                core.pictures.keep(set);
                core.changed(What::View);
            }
            Ok(None) => {}
            Err(why) => eprintln!("sikemux core: the turn's page was not taken: {why}"),
        }
        core.pictures.taking.locked().remove(&(agent, turn));
    });
}

async fn shoot(
    core: &Arc<Core>,
    agent: &str,
    project: &str,
    turn: u64,
    head: Option<String>,
) -> Result<Option<Set>, String> {
    let Some(dir) = checkout(core, agent) else {
        return Ok(None);
    };
    let served = {
        let core = core.clone();
        blocking(move || Ok(served_at(&core, &dir)))
            .await
            .map_err(|error| error.to_string())?
    };
    let Some(url) = served else {
        return Ok(None);
    };
    if let Some(why) = not_a_page(&url).await {
        return Err(format!("{url}: {why}"));
    }
    let folder = (core.pictures.folder.clone()).ok_or("This core keeps no files")?;
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let at = epoch_ms();
    let mut shots = Vec::new();
    let mut decoded = Vec::new();
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
        let width = (taken["width"].as_u64())
            .and_then(|width| u32::try_from(width).ok())
            .unwrap_or_default();
        let id = format!("{}-{at}-w{width}.jpg", uuid::Uuid::new_v4().simple());
        sikemux_process::files::write_atomic(&folder.join(&id), &bytes, true)
            .map_err(|error| error.to_string())?;
        shots.push(Shot { width, id });
        decoded.push((bytes, Some(width)));
    }
    if let Some(head) = head {
        let (keeping, project, agent, opened) = (
            core.clone(),
            project.to_owned(),
            agent.to_owned(),
            url.clone(),
        );
        tokio::task::spawn_blocking(move || {
            super::board::turn_pictures(&keeping, &project, &agent, &head, &opened, &decoded)
        });
    }
    let page = url::Url::parse(&url).map_or_else(|_| "/".to_owned(), |url| url.path().to_owned());
    Ok(Some(Set {
        project: project.to_owned(),
        agent: agent.to_owned(),
        turn,
        page,
        at,
        shots,
    }))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPictureParams {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPictureData {
    pub mime_type: String,
    pub data: String,
}

/// A picture the core took of a turn's page, by the id a chat's head names it by.
pub(crate) async fn picture(
    core: &Arc<Core>,
    params: TurnPictureParams,
) -> CoreResult<TurnPictureData> {
    let path = (core.pictures.file_of(&params.id)).ok_or("The core keeps no picture by that id")?;
    let bytes = blocking(move || Ok(std::fs::read(path)?)).await?;
    Ok(TurnPictureData {
        mime_type: "image/jpeg".to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Lets go of pictures no chat will show again: those of removed projects, and with
/// `closed_agents` all but the latest of each page of the agents not open now.
pub(crate) fn tidy(core: &Core, closed_agents: bool) {
    let mut open: HashSet<String> = core.open_agents().into_iter().collect();
    if !closed_agents {
        open.extend(
            core.pictures
                .state
                .locked()
                .sets
                .iter()
                .map(|set| set.agent.clone()),
        );
    }
    let projects: HashSet<String> = (core.workspaces.project_paths().into_iter())
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    core.pictures.tidy(&open, &projects);
}

/// A row for each of a lead's `workers` whose turn finished, oldest first.
pub(crate) fn results(
    core: &Core,
    workers: &[&String],
    turns: &HashMap<String, TurnState>,
) -> Vec<ResultRow> {
    let mut rows: Vec<ResultRow> = (workers.iter())
        .filter_map(|worker| {
            let turn = (turns.get(*worker)).filter(|turn| turn.state == TurnPhase::Done)?;
            let done = turn.done.clone()?;
            let started = core.inbox.ending(worker).map(|ended| ended.started_at);
            Some(ResultRow {
                agent_id: (*worker).clone(),
                name: core.names.name(worker),
                title: core.workspaces.title(worker).unwrap_or_default(),
                at: turn.since,
                undo: done.changes == WorkAt::Landed
                    && core
                        .inbox
                        .ending(worker)
                        .and_then(|ended| ended.work)
                        .is_some_and(|work| !work.own.is_empty()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_kept_set_survives_a_tidy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (data, folder) = (dir.path().join("data"), dir.path().join("shop"));
        std::fs::create_dir_all(&folder).expect("project folder");
        let core = Core::new(
            sikemux_wire::protocol::BuildIdentity::default(),
            Some(&data),
        )
        .expect("core");
        let added = super::super::workspace::add_project(
            &core,
            super::super::workspace::AddProjectParams::at(folder),
        )
        .await
        .expect("project");
        core.pictures.keep(Set {
            project: added.path.to_string_lossy().into_owned(),
            agent: "agent-a".to_owned(),
            turn: 1,
            page: "/".to_owned(),
            at: 1,
            shots: Vec::new(),
        });
        tidy(&core, true);
        assert_eq!(core.pictures.turns_of("agent-a").len(), 1);
    }
}
