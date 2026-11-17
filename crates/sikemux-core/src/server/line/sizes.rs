//! How much memory and time each project's heavy commands took the last times they ran, kept
//! in the core's data folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

const FILE: &str = "line-sizes.json";
/// How many of a command's latest runs its size is judged by.
const RUNS_KEPT: usize = 5;

#[derive(Clone, Copy)]
pub(super) struct Size {
    pub(super) peak: u64,
    pub(super) took_ms: u64,
}

pub(super) struct Sizes {
    file: Option<PathBuf>,
    kept: Mutex<Option<Kept>>,
}

/// Project, then command, then its latest runs.
type Kept = BTreeMap<String, BTreeMap<String, Runs>>;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Runs {
    peaks: Vec<u64>,
    took_ms: Vec<u64>,
}

impl Sizes {
    pub(super) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            file: data_dir.map(|dir| dir.join(FILE)),
            kept: Mutex::default(),
        }
    }

    pub(super) fn of(&self, project: &str, key: &str) -> Option<Size> {
        let mut kept = self.kept.locked();
        let runs = self.loaded(&mut kept).get(project)?.get(key)?;
        let mut took = runs.took_ms.clone();
        took.sort_unstable();
        Some(Size {
            peak: runs.peaks.iter().copied().max()?,
            took_ms: took.get(took.len() / 2).copied().unwrap_or_default(),
        })
    }

    pub(super) fn record(&self, project: &str, key: &str, peak: u64, took_ms: u64) {
        let mut kept = self.kept.locked();
        let all = self.loaded(&mut kept);
        let runs = (all.entry(project.to_owned()).or_default())
            .entry(key.to_owned())
            .or_default();
        for (list, value) in [(&mut runs.peaks, peak), (&mut runs.took_ms, took_ms)] {
            list.push(value);
            if list.len() > RUNS_KEPT {
                list.remove(0);
            }
        }
        let Some(file) = &self.file else {
            return;
        };
        let written = serde_json::to_vec(all)
            .map_err(std::io::Error::other)
            .and_then(|json| sikemux_process::files::write_atomic_unsynced(file, &json, false));
        if let Err(error) = written {
            eprintln!("sikemux core: how much heavy commands take was not kept: {error}");
        }
    }

    fn loaded<'a>(&self, kept: &'a mut Option<Kept>) -> &'a mut Kept {
        kept.get_or_insert_with(|| {
            (self.file.as_ref())
                .and_then(|file| std::fs::read(file).ok())
                .and_then(|json| serde_json::from_slice(&json).ok())
                .unwrap_or_default()
        })
    }
}
