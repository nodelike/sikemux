//! The folders the agents' saved chats ran in, offered as projects while Sikemux is set up.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::recent::{provider_hits, PageScan, RecentProvider};
use crate::git::remote::{main_remote, normalized_remote};

const MAX_CHATS_PER_AGENT: usize = 4_000;
const MAX_FOLDERS: usize = 512;
const ACTIVE_WITHIN_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const ACTIVE_CHATS: u32 = 3;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FoundProject {
    /// `owner/name` from the remote, or the folder's name.
    pub name: String,
    pub path: String,
    pub chats: u32,
    pub active_ms: u64,
    pub suggested: bool,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FoundProjects {
    pub projects: Vec<FoundProject>,
    /// The scan stopped at a limit, so older folders may be missing.
    pub partial: bool,
}

#[derive(Clone, Copy, Default)]
struct Activity {
    chats: u32,
    active_ms: u64,
}

impl Activity {
    fn add(&mut self, other: Activity) {
        self.chats += other.chats;
        self.active_ms = self.active_ms.max(other.active_ms);
    }
}

struct Checkout {
    path: PathBuf,
    activity: Activity,
}

/// One repository's clones, newest first, or a folder outside git.
struct Group {
    remote: Option<String>,
    git: bool,
    checkouts: Vec<Checkout>,
    activity: Activity,
}

enum Place {
    Repository(PathBuf),
    Folder(PathBuf),
}

/// Folders whose contents are never anyone's project: Downloads and the temporary folders
/// agents make scratch repositories in.
fn never_projects(home: Option<&Path>) -> Vec<PathBuf> {
    let temporary = std::env::temp_dir();
    let resolved = temporary.canonicalize().ok();
    home.map(|home| home.join("Downloads"))
        .into_iter()
        .chain([
            temporary,
            PathBuf::from("/tmp"),
            PathBuf::from("/private/tmp"),
        ])
        .chain(resolved)
        .collect()
}

/// Where a chat's folder counts as a project: the repository it is inside, or itself. Nothing
/// for a folder that is gone, in a place that is never a project, or in a linked worktree.
fn place_of(folder: &Path, home: Option<&Path>, never: &[PathBuf]) -> Option<Place> {
    if !folder.is_dir() || never.iter().any(|place| folder.starts_with(place)) {
        return None;
    }
    for ancestor in folder.ancestors() {
        if Some(ancestor) == home {
            break;
        }
        let git = ancestor.join(".git");
        if git.is_file() && linked_worktree(&git) {
            return None;
        }
        if git.exists() {
            return Some(Place::Repository(ancestor.to_path_buf()));
        }
    }
    Some(Place::Folder(folder.to_path_buf()))
}

/// A submodule's `.git` file points into `modules/`, a linked worktree's into `worktrees/`.
fn linked_worktree(git_file: &Path) -> bool {
    std::fs::read_to_string(git_file).is_ok_and(|pointer| {
        Path::new(pointer.trim().trim_start_matches("gitdir:").trim())
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|parent| parent == "worktrees")
    })
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// `owner/name` out of a normalised remote such as `github.com/owner/name`.
fn owner_and_name(remote: &str) -> Option<&str> {
    remote.split_once('/').map(|(_, path)| path)
}

/// How many chats each folder saw, and when last, newest folder first.
fn activity_by_folder(providers: &[RecentProvider]) -> (Vec<(String, Activity)>, bool) {
    let exclude = HashSet::new();
    let scans: Vec<_> = providers
        .par_iter()
        .map(|provider| {
            let scan = PageScan::all(provider.agent, None, MAX_CHATS_PER_AGENT, &exclude);
            provider_hits(provider, &scan)
        })
        .collect();
    let partial = scans.iter().any(|hits| hits.len() >= MAX_CHATS_PER_AGENT);
    let mut folders: HashMap<String, Activity> = HashMap::new();
    for hit in scans.into_iter().flatten() {
        folders.entry(hit.found.project).or_default().add(Activity {
            chats: 1,
            active_ms: hit.at_ms,
        });
    }
    let mut folders: Vec<_> = folders.into_iter().collect();
    folders.sort_unstable_by_key(|(_, activity)| std::cmp::Reverse(activity.active_ms));
    (folders, partial)
}

fn grouped(places: Vec<(Place, Activity)>) -> Vec<Group> {
    let mut repositories: HashMap<PathBuf, Activity> = HashMap::new();
    let mut folders: HashMap<PathBuf, Activity> = HashMap::new();
    for (place, activity) in places {
        match place {
            Place::Repository(path) => repositories.entry(path).or_default().add(activity),
            Place::Folder(path) => folders.entry(path).or_default().add(activity),
        }
    }
    let remotes: Vec<(PathBuf, Activity, Option<String>)> = repositories
        .into_par_iter()
        .map(|(path, activity)| {
            let remote = main_remote(&path.to_string_lossy())
                .map(|remote| normalized_remote(&remote))
                .filter(|remote| !remote.is_empty());
            (path, activity, remote)
        })
        .collect();
    let mut by_remote: HashMap<String, Group> = HashMap::new();
    for (path, activity, remote) in remotes {
        let key = remote
            .clone()
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let group = by_remote.entry(key).or_insert_with(|| Group {
            remote,
            git: true,
            checkouts: Vec::new(),
            activity: Activity::default(),
        });
        group.activity.add(activity);
        group.checkouts.push(Checkout { path, activity });
    }
    let mut groups: Vec<Group> = by_remote.into_values().collect();
    groups.extend(folders.into_iter().map(|(path, activity)| Group {
        remote: None,
        git: false,
        checkouts: vec![Checkout { path, activity }],
        activity,
    }));
    for group in &mut groups {
        group
            .checkouts
            .sort_unstable_by_key(|checkout| std::cmp::Reverse(checkout.activity.active_ms));
    }
    groups.sort_unstable_by_key(|group| {
        (
            !group.git,
            std::cmp::Reverse(group.activity.active_ms),
            group.checkouts[0].path.clone(),
        )
    });
    groups
}

/// The projects to offer, repositories before plain folders and newest activity first. Only the
/// newest clone of an active repository is suggested, so one remote does not open twice.
pub fn found_projects(providers: Vec<RecentProvider>, now_ms: u64) -> FoundProjects {
    let (mut folders, mut partial) = activity_by_folder(&providers);
    if folders.len() > MAX_FOLDERS {
        folders.truncate(MAX_FOLDERS);
        partial = true;
    }
    let home = sikemux_process::paths::home();
    let never = never_projects(home.as_deref());
    let places: Vec<(Place, Activity)> = folders
        .into_par_iter()
        .filter_map(|(folder, activity)| {
            Some((
                place_of(Path::new(&folder), home.as_deref(), &never)?,
                activity,
            ))
        })
        .collect();
    let projects = grouped(places)
        .into_iter()
        .flat_map(|group| {
            let active = group.git
                && group.activity.chats >= ACTIVE_CHATS
                && now_ms.saturating_sub(group.activity.active_ms) <= ACTIVE_WITHIN_MS;
            let name = group
                .remote
                .as_deref()
                .and_then(owner_and_name)
                .map(str::to_owned);
            group
                .checkouts
                .into_iter()
                .enumerate()
                .map(move |(at, checkout)| FoundProject {
                    name: name.clone().unwrap_or_else(|| folder_name(&checkout.path)),
                    path: checkout.path.to_string_lossy().into_owned(),
                    chats: checkout.activity.chats,
                    active_ms: checkout.activity.active_ms,
                    suggested: active && at == 0,
                })
        })
        .collect();
    FoundProjects { projects, partial }
}
