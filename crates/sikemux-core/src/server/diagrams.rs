//! Where the person moved the tables of each diagram the apps draw, such as a DBML file's, and
//! how far it was panned and zoomed, kept by key so the arrangement outlives a restart. The core
//! keeps each as the apps wrote it: what it means is theirs. Keys name what a diagram is of:
//! `file:<host>:<path>`, `chat:<agent>:<text>`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::kept::Kept;
use super::{CoreError, CoreResult};

const FILE: &str = "diagrams.json";
/// Past this many diagrams the one looked at longest ago goes.
const MAX_DIAGRAMS: usize = 512;
/// The largest arrangement kept, in bytes of JSON.
const MAX_LAYOUT: usize = 64 * 1024;
const MAX_KEY: usize = 1024;

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    layout: Value,
    /// When it was last read or written, in ms since the epoch, so the oldest goes first.
    at: u64,
}

pub(crate) struct Diagrams {
    held: Mutex<BTreeMap<String, Stored>>,
    kept: Kept,
}

impl Diagrams {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            held: Mutex::new(kept.load()),
            kept,
        }
    }

    /// The arrangement kept under `key`, if any.
    pub(crate) fn get(&self, key: &str) -> Option<Value> {
        let mut held = self.held.locked();
        let stored = held.get_mut(key)?;
        stored.at = epoch_ms();
        Some(stored.layout.clone())
    }

    /// Keeps `layout` under `key`, or forgets the arrangement there without one.
    pub(crate) fn set(&self, key: &str, layout: Option<Value>) -> CoreResult<()> {
        if key.is_empty() || key.len() > MAX_KEY {
            return Err(CoreError::from("a diagram's key must be 1 to 1024 bytes"));
        }
        let mut held = self.held.locked();
        match layout {
            Some(layout) => {
                if serde_json::to_vec(&layout).map_or(usize::MAX, |bytes| bytes.len()) > MAX_LAYOUT
                {
                    return Err(CoreError::from("a diagram's arrangement is at most 64 KB"));
                }
                let at = epoch_ms();
                held.insert(key.to_owned(), Stored { layout, at });
                while held.len() > MAX_DIAGRAMS {
                    let oldest = (held.iter())
                        .min_by_key(|(_, stored)| stored.at)
                        .map(|(key, _)| key.clone());
                    match oldest {
                        Some(oldest) => held.remove(&oldest),
                        None => break,
                    };
                }
            }
            None => {
                if held.remove(key).is_none() {
                    return Ok(());
                }
            }
        }
        self.kept.save_soon(&*held);
        Ok(())
    }
}
