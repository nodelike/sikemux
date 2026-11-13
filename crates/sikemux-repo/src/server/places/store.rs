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
    /// The place each open call offers to keep on the person's answer, by call: a target the
    /// tool's own config named.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub offered: std::collections::BTreeMap<String, Place>,
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
