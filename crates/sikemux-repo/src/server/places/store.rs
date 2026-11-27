//! Each project's places, kept in a file of their own beside the boards, so a core of an
//! earlier release that rewrites a board file never loses them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::{Place, Unplaced};
use crate::server::kept;

const FOLDER: &str = "places";

/// A workflow that deploys pushes to a branch, kept once seen so later edits can't hide it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushDeploy {
    pub file: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environments: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches_ignore: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths_ignore: Vec<String>,
}

impl PushDeploy {
    /// Whether a push to `branch` changing `changed` files starts this workflow. Unknown branch
    /// or unknown files match conservatively.
    pub fn matches(&self, branch: Option<&str>, changed: Option<&[String]>) -> bool {
        if let Some(branch) = branch {
            if !self.branches.is_empty() && !matches_filter(&self.branches, branch) {
                return false;
            }
            if !self.branches_ignore.is_empty() && matches_filter(&self.branches_ignore, branch) {
                return false;
            }
        }
        if self.paths.is_empty() && self.paths_ignore.is_empty() {
            return true;
        }
        let Some(files) = changed else {
            return true;
        };
        files.iter().any(|file| {
            let file = file.trim_start_matches("./");
            if !self.paths.is_empty() && !matches_filter(&self.paths, file) {
                return false;
            }
            if !self.paths_ignore.is_empty() && matches_filter(&self.paths_ignore, file) {
                return false;
            }
            true
        })
    }
}

fn matches_filter(patterns: &[String], value: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        if let Some(negated) = pattern.strip_prefix('!') {
            if glob_matches(negated, value) {
                matched = false;
            }
        } else if glob_matches(pattern, value) {
            matched = true;
        }
    }
    matched
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    let mut memo = vec![None; (p.len() + 1) * (v.len() + 1)];
    glob_at(&p, &v, 0, 0, &mut memo)
}

fn glob_at(p: &[char], v: &[char], pi: usize, vi: usize, memo: &mut [Option<bool>]) -> bool {
    let idx = pi * (v.len() + 1) + vi;
    if let Some(Some(cached)) = memo.get(idx) {
        return *cached;
    }
    let result = match (p.get(pi..), v.get(vi..)) {
        (Some([]), Some([])) => true,
        (Some(['*', '*', '/', ..]), _) => {
            glob_at(p, v, pi + 3, vi, memo) || (vi < v.len() && glob_at(p, v, pi, vi + 1, memo))
        }
        (Some(['*', '*', ..]), _) => {
            glob_at(p, v, pi + 2, vi, memo) || (vi < v.len() && glob_at(p, v, pi, vi + 1, memo))
        }
        (Some(['*', ..]), Some([ch, ..])) => {
            glob_at(p, v, pi + 1, vi, memo) || (*ch != '/' && glob_at(p, v, pi, vi + 1, memo))
        }
        (Some(['*', ..]), Some([])) => glob_at(p, v, pi + 1, vi, memo),
        (Some(['?', ..]), Some([ch, ..])) => *ch != '/' && glob_at(p, v, pi + 1, vi + 1, memo),
        (Some([pc, ..]), Some([vc, ..])) => pc == vc && glob_at(p, v, pi + 1, vi + 1, memo),
        _ => false,
    };
    if let Some(slot) = memo.get_mut(idx) {
        *slot = Some(result);
    }
    result
}

/// What a project keeps of its places.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub project: String,
    #[serde(default)]
    pub places: Vec<Place>,
    /// When the person confirmed the list of production places.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<u64>,
    /// What its agents sent out that no place covers, once each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unplaced: Vec<Unplaced>,
    /// Things sent out past what `unplaced` holds, which it doesn't list.
    #[serde(default)]
    pub more_seen: u32,
    /// Whether its files once said a push to its branch deploys it to production on Vercel.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deploys_pushes: bool,
    /// Workflows that deploy pushes to the project's branch, kept once seen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub push_workflows: Vec<PushDeploy>,
}

pub struct Store {
    folder: Option<PathBuf>,
    kept: Mutex<HashMap<String, Kept>>,
}

impl Store {
    pub fn new(data_dir: Option<&Path>) -> Self {
        let folder = data_dir.map(|dir| dir.join(FOLDER));
        let kept = read_all(data_dir)
            .into_iter()
            .map(|kept| (kept.project.clone(), kept))
            .collect();
        Self {
            folder,
            kept: Mutex::new(kept),
        }
    }

    pub fn of(&self, project: &str) -> Kept {
        (self.kept.locked().get(project).cloned()).unwrap_or_else(|| Kept {
            project: project.to_owned(),
            ..Kept::default()
        })
    }

    /// Changes what `project` keeps by `change` without writing it out, for what a later
    /// write may carry, such as how often a seen action ran.
    pub fn touch(&self, project: &str, change: impl FnOnce(&mut Kept)) {
        let mut all = self.kept.locked();
        let kept = all.entry(project.to_owned()).or_insert_with(|| Kept {
            project: project.to_owned(),
            ..Kept::default()
        });
        change(kept);
    }

    /// Changes what `project` keeps by `change`, and writes it out when it changed, which
    /// the last of what it answers says.
    pub fn change<R>(
        &self,
        project: &str,
        change: impl FnOnce(&mut Kept) -> Result<R, String>,
    ) -> Result<(R, Kept, bool), String> {
        let mut all = self.kept.locked();
        let kept = all.entry(project.to_owned()).or_insert_with(|| Kept {
            project: project.to_owned(),
            ..Kept::default()
        });
        let before = kept.clone();
        let answer = change(kept)?;
        let after = kept.clone();
        drop(all);
        let changed = after != before;
        if changed {
            write(self.folder.as_deref(), &after);
        }
        Ok((answer, after, changed))
    }
}

fn write(folder: Option<&Path>, kept: &Kept) {
    if let Some(Err(error)) = folder.map(std::fs::create_dir_all) {
        eprintln!("sikemux core: places will not be saved: {error}");
    }
    kept::Kept::new(folder, &kept::file_for(&kept.project)).save(kept);
}

/// Every project's places as kept on disk.
pub fn read_all(data_dir: Option<&Path>) -> Vec<Kept> {
    let folder = data_dir.map(|dir| dir.join(FOLDER));
    kept::load_all_upgraded::<Kept>(folder.as_deref(), |value| value)
}

/// Keeps `places`, carried over from a board's environments, as `project`'s, unless it
/// already keeps places of its own.
pub fn carried_over(data_dir: Option<&Path>, project: &str, places: Vec<Place>) {
    let folder = data_dir.map(|dir| dir.join(FOLDER));
    let file = folder.as_ref().map(|dir| dir.join(kept::file_for(project)));
    if file.is_some_and(|file| file.exists()) {
        return;
    }
    let kept = Kept {
        project: project.to_owned(),
        places,
        ..Kept::default()
    };
    write(folder.as_deref(), &kept);
}
