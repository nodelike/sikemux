//! Which saved conversation each agent writes to. For the agents in the
//! workspace the core watches each CLI's session folders, binds a terminal
//! agent to the session it starts, keeps titles in step with the ones the CLI
//! saves, and writes a name the person gave back into the CLI's own session.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sikemux_process::sync::Locked;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::agent_accounts::Book;
use crate::agent_catalog::AgentKind;
use crate::protocol::WorkspaceAgent;
use crate::sessions::{self, AgentSession};

use super::connection::blocking;
use super::Core;

/// How long writes to sessions are let settle before the sessions are read.
const SETTLE: Duration = Duration::from_millis(200);
/// A session written to without pause is read at most this often.
const SETTLE_LONGEST: Duration = Duration::from_secs(2);
/// A title no longer than this that the session id starts with is the CLI's
/// placeholder, not a name.
const PLACEHOLDER_TITLE_CHARS: usize = 13;

/// One CLI's sessions for one folder and account.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Group {
    kind: AgentKind,
    cwd: String,
    config_path: Option<String>,
}

/// Where one group's sessions are written, as file events name them.
struct Places {
    group: Group,
    sessions: Vec<(PathBuf, RecursiveMode)>,
    listings: Vec<PathBuf>,
}

enum Touch {
    Session(PathBuf),
    Listing,
    /// A session folder itself came or went.
    Folder,
}

#[derive(Default)]
struct Touched {
    listing: bool,
    sessions: HashSet<PathBuf>,
}

#[derive(Default)]
struct Watching {
    places: Vec<Places>,
    folders: HashMap<PathBuf, RecursiveMode>,
    watcher: Option<RecommendedWatcher>,
}

#[derive(Default)]
pub(crate) struct SessionWatch {
    watching: Mutex<Watching>,
    /// The sessions there were when a terminal agent started fresh, so it never
    /// takes one it did not start.
    baselines: Mutex<HashMap<String, HashSet<String>>>,
    /// Names already written back, or refused, so neither is tried again.
    named: Mutex<HashSet<(String, String)>>,
}

fn group_of(book: &Book, agent: &WorkspaceAgent) -> Option<Group> {
    let kind = AgentKind::from_name(&agent.provider)?;
    let config_path = agent
        .profile_id
        .as_deref()
        .and_then(|id| book.profile(id))
        .and_then(|profile| profile.config_path.clone());
    Some(Group {
        kind,
        cwd: agent.cwd.to_string_lossy().into_owned(),
        config_path,
    })
}

fn agents(core: &Core) -> Vec<WorkspaceAgent> {
    core.workspaces
        .agent_ids()
        .into_iter()
        .filter_map(|agent_id| core.workspaces.agent(&agent_id))
        .filter(|agent| !agent.asleep)
        .collect()
}

/// Watches the session folders of every group an agent of the workspace is
/// in, and only those. Runs whenever the workspace changes.
pub(crate) fn follow(core: &Arc<Core>) {
    let book = core.workspaces.accounts().unwrap_or_default();
    let wanted: HashSet<Group> = agents(core)
        .iter()
        .filter_map(|agent| group_of(&book, agent))
        .collect();
    let mut watching = core.session_watch.watching.locked();
    let fresh: Vec<Group> = wanted
        .iter()
        .filter(|group| !watching.places.iter().any(|places| places.group == **group))
        .cloned()
        .collect();
    if fresh.is_empty() && watching.places.len() == wanted.len() {
        return;
    }
    place(core, &mut watching, wanted);
    drop(watching);
    for group in fresh {
        let owner = Arc::downgrade(core);
        tokio::spawn(async move { scan(&owner, &group).await });
    }
}

/// Watches the folders `groups` write their sessions to, every group through
/// one watcher.
fn place(core: &Arc<Core>, watching: &mut Watching, groups: impl IntoIterator<Item = Group>) {
    let places: Vec<Places> = groups.into_iter().map(Places::of).collect();
    let mut folders: HashMap<PathBuf, RecursiveMode> = HashMap::new();
    for (dir, mode) in places.iter().flat_map(Places::folders) {
        let known = folders.entry(dir).or_insert(mode);
        if mode == RecursiveMode::Recursive {
            *known = mode;
        }
    }
    if watching.watcher.is_none() {
        watching.watcher = start(core);
    }
    let Watching {
        places: kept,
        folders: watched,
        watcher,
    } = watching;
    *kept = places;
    let Some(watcher) = watcher else {
        return;
    };
    if *watched == folders {
        return;
    }
    let mut paths = watcher.paths_mut();
    for (dir, mode) in watched.iter() {
        if folders.get(dir) != Some(mode) {
            let _ = paths.remove(dir);
        }
    }
    folders.retain(|dir, mode| watched.get(dir) == Some(mode) || paths.add(dir, *mode).is_ok());
    let _ = paths.commit();
    *watched = folders;
}

fn start(core: &Arc<Core>) -> Option<RecommendedWatcher> {
    let (changed, heard) = mpsc::unbounded_channel::<Vec<PathBuf>>();
    let watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        let Ok(event) = event else {
            return;
        };
        if matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            let _ = changed.send(event.paths);
        }
    })
    .ok()?;
    tokio::spawn(listen(Arc::downgrade(core), heard));
    Some(watcher)
}

/// Reads the sessions written to once their writes settle.
async fn listen(owner: Weak<Core>, mut heard: mpsc::UnboundedReceiver<Vec<PathBuf>>) {
    let mut pending: HashMap<Group, Touched> = HashMap::new();
    let mut due: Option<(Instant, Instant)> = None;
    loop {
        let wake = due.map(|(quiet, latest)| quiet.min(latest));
        let settled = async move {
            match wake {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            paths = heard.recv() => {
                let Some(paths) = paths else {
                    return;
                };
                let Some(core) = owner.upgrade() else {
                    return;
                };
                if route(&core, &paths, &mut pending) {
                    let now = Instant::now();
                    let latest = due.map_or(now + SETTLE_LONGEST, |(_, latest)| latest);
                    due = Some((now + SETTLE, latest));
                }
            }
            () = settled => {
                due = None;
                let Some(core) = owner.upgrade() else {
                    return;
                };
                for (group, touched) in std::mem::take(&mut pending) {
                    refresh(&core, group, touched).await;
                }
            }
        }
    }
}

/// Notes the sessions of each group `paths` touched. Whether there were any.
fn route(core: &Arc<Core>, paths: &[PathBuf], pending: &mut HashMap<Group, Touched>) -> bool {
    let mut touches: Vec<(Group, Touch)> = Vec::new();
    let mut moved = false;
    {
        let mut watching = core.session_watch.watching.locked();
        for places in &watching.places {
            for path in paths {
                if let Some(touch) = places.touched(path) {
                    moved |= matches!(touch, Touch::Folder);
                    touches.push((places.group.clone(), touch));
                }
            }
        }
        if moved {
            let groups: Vec<Group> = watching
                .places
                .iter()
                .map(|places| places.group.clone())
                .collect();
            place(core, &mut watching, groups);
        }
    }
    let mut working: HashMap<Group, Vec<String>> = HashMap::new();
    let mut any = false;
    for (group, touch) in touches {
        match touch {
            Touch::Session(place) => {
                let streaming = working
                    .entry(group.clone())
                    .or_insert_with_key(|group| streaming(core, group));
                if streamed(&place, streaming) {
                    continue;
                }
                pending.entry(group).or_default().sessions.insert(place);
            }
            Touch::Listing | Touch::Folder => pending.entry(group).or_default().listing = true,
        }
        any = true;
    }
    any
}

/// Reads again the sessions a group's files say changed.
async fn refresh(core: &Arc<Core>, group: Group, touched: Touched) {
    let rows = if touched.listing {
        listed(group.clone()).await
    } else {
        let read = group.clone();
        blocking(move || {
            Ok(touched
                .sessions
                .iter()
                .filter_map(|place| {
                    sessions::session_at(read.kind, place, &read.cwd, read.config_path.as_deref())
                })
                .collect())
        })
        .await
        .unwrap_or_default()
    };
    if !rows.is_empty() {
        settle(core, &group, rows).await;
    }
}

/// The sessions of the group's chats with a turn running.
fn streaming(core: &Core, group: &Group) -> Vec<String> {
    core.chats
        .ids()
        .into_iter()
        .filter_map(|agent_id| core.chats.get(&agent_id))
        .filter(|chat| {
            chat.provider() == group.kind.as_str()
                && chat.launch.cwd.to_string_lossy() == group.cwd
                && chat.turn_running()
        })
        .filter_map(|chat| Some(chat.feed.start()?.session_id))
        .collect()
}

/// Whether `place` is the transcript of a chat at work, which the core already
/// has every word of. Each CLI names the file after its session, or by it.
fn streamed(place: &Path, sessions: &[String]) -> bool {
    let name = place
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    sessions
        .iter()
        .any(|id| Path::new(id) == place || name.contains(id.as_str()))
}

impl Places {
    fn of(group: Group) -> Self {
        let places = group
            .kind
            .ops()
            .session_places(&group.cwd, group.config_path.as_deref());
        Places {
            group,
            sessions: places
                .sessions
                .into_iter()
                .map(|(dir, mode)| (real(&dir), mode))
                .collect(),
            listings: places.listings.iter().map(|place| real(place)).collect(),
        }
    }

    /// The folders to watch: each session folder, or the one it will appear
    /// in, and the folder of each listing.
    fn folders(&self) -> Vec<(PathBuf, RecursiveMode)> {
        let sessions = self.sessions.iter().map(|(dir, mode)| (dir, *mode));
        let listings = self
            .listings
            .iter()
            .map(|place| (place, RecursiveMode::NonRecursive));
        sessions
            .chain(listings)
            .filter_map(|(path, mode)| {
                if path.is_dir() {
                    return Some((path.clone(), mode));
                }
                let parent = path.parent().filter(|parent| parent.is_dir())?;
                Some((parent.to_path_buf(), RecursiveMode::NonRecursive))
            })
            .collect()
    }

    fn touched(&self, path: &Path) -> Option<Touch> {
        if self.sessions.iter().any(|(dir, _)| dir == path) {
            return Some(Touch::Folder);
        }
        if self
            .listings
            .iter()
            .any(|place| place == path || path.parent() == Some(place.as_path()))
        {
            return Some(Touch::Listing);
        }
        let place = sessions::session_place(self.group.kind, path)?;
        let within = self.sessions.iter().any(|(dir, mode)| match mode {
            RecursiveMode::Recursive => place.starts_with(dir),
            RecursiveMode::NonRecursive => place.parent() == Some(dir.as_path()),
        });
        within.then_some(Touch::Session(place))
    }
}

/// `path` as file events name it, through any link, even before it exists.
fn real(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    match (path.parent().map(Path::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

/// Notes the sessions there are as a terminal agent starts without one, so
/// the one it binds to is the one it starts.
pub(crate) async fn note_launch(core: &Arc<Core>, agent: &WorkspaceAgent) {
    let book = core.workspaces.accounts().unwrap_or_default();
    let Some(group) = group_of(&book, agent) else {
        return;
    };
    let listed = listed(group).await;
    let ids = listed.into_iter().map(|session| session.id).collect();
    core.session_watch
        .baselines
        .locked()
        .insert(agent.id.clone(), ids);
}

async fn listed(group: Group) -> Vec<AgentSession> {
    blocking(move || {
        Ok(sessions::agent_sessions(
            group.kind,
            &group.cwd,
            group.config_path.as_deref(),
        ))
    })
    .await
    .unwrap_or_default()
}

/// Reads the group's sessions again and settles its agents on them.
async fn scan(core: &Weak<Core>, group: &Group) {
    let rows = listed(group.clone()).await;
    let Some(core) = core.upgrade() else {
        return;
    };
    if !rows.is_empty() {
        settle(&core, group, rows).await;
    }
}

fn usable_title(row: &AgentSession) -> Option<String> {
    let title = row.title.trim();
    let placeholder = title.chars().count() <= PLACEHOLDER_TITLE_CHARS && row.id.starts_with(title);
    (!title.is_empty() && !placeholder).then(|| title.to_owned())
}

/// Takes the CLI's title for an agent the person has not named, when it is new.
fn take_title(core: &Core, agent: &WorkspaceAgent, row: &AgentSession) {
    let title = usable_title(row).filter(|title| *title != agent.title);
    super::workspace::retitle(core, &agent.id, title, false);
}

async fn settle(core: &Arc<Core>, group: &Group, rows: Vec<AgentSession>) {
    let book = core.workspaces.accounts().unwrap_or_default();
    let members: Vec<WorkspaceAgent> = agents(core)
        .into_iter()
        .filter(|agent| group_of(&book, agent).as_ref() == Some(group))
        .collect();
    let by_id: HashMap<&str, &AgentSession> =
        rows.iter().map(|row| (row.id.as_str(), row)).collect();
    let mut claimed: HashSet<String> = HashSet::new();
    let mut names = Vec::new();
    for agent in &members {
        let Some(resume_id) = &agent.resume_id else {
            continue;
        };
        claimed.insert(resume_id.clone());
        let Some(row) = by_id.get(resume_id.as_str()) else {
            continue;
        };
        if agent.renamed {
            if row.title != agent.title {
                names.push((resume_id.clone(), agent.title.clone()));
            }
        } else {
            take_title(core, agent, row);
        }
    }
    let mut fresh: Vec<&WorkspaceAgent> = members
        .iter()
        .filter(|agent| agent.resume_id.is_none() && agent.pty_id.is_some())
        .collect();
    fresh.sort_by_key(|agent| std::cmp::Reverse(agent.created_at));
    let mut candidates: Vec<&AgentSession> = rows
        .iter()
        .filter(|row| !claimed.contains(&row.id))
        .collect();
    candidates.sort_by_key(|row| std::cmp::Reverse(row.mtime));
    for agent in fresh {
        let baseline = core
            .session_watch
            .baselines
            .locked()
            .get(&agent.id)
            .cloned();
        let launched = agent.created_at / 1000;
        let found = candidates.iter().position(|row| match &baseline {
            Some(baseline) => !baseline.contains(&row.id),
            None => row.mtime >= launched,
        });
        let Some(index) = found else {
            continue;
        };
        let row = candidates.remove(index);
        let resumed = core
            .workspaces
            .edit_agent(&agent.id, |agent| agent.resume_id = Some(row.id.clone()));
        if resumed.is_err() {
            continue;
        }
        core.session_watch.baselines.locked().remove(&agent.id);
        if agent.renamed {
            names.push((row.id.clone(), agent.title.clone()));
        } else {
            take_title(core, agent, row);
        }
        core.view_changed();
    }
    for (session_id, name) in names {
        name_session(core, group, session_id, name).await;
    }
}

/// Writes a name the person gave an agent into its CLI's own session, once.
async fn name_session(core: &Core, group: &Group, session_id: String, name: String) {
    let key = (session_id.clone(), name.clone());
    let first = core.session_watch.named.locked().insert(key);
    if !first {
        return;
    }
    let renamed = sessions::rename::rename(
        group.kind,
        group.cwd.clone(),
        session_id,
        name,
        None,
        group.config_path.clone(),
    )
    .await;
    if let Err(error) = renamed {
        eprintln!("sikemux core: an agent's name was not saved with its session: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{usable_title, AgentSession};

    fn row(id: &str, title: &str) -> AgentSession {
        AgentSession {
            id: id.into(),
            title: title.into(),
            mtime: 0,
        }
    }

    #[test]
    fn the_cli_s_placeholder_is_no_title_and_its_own_name_is() {
        let id = "019fd81d-c861-7920-bc88-a41a6b17aca4";
        assert_eq!(usable_title(&row(id, &id[..8])), None);
        assert_eq!(usable_title(&row(id, "  ")), None);
        assert_eq!(usable_title(&row(id, "Hello")).as_deref(), Some("Hello"));
    }
}
