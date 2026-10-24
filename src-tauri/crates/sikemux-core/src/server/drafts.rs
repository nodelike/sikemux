//! What the person has typed and not sent yet, such as a commit message, kept by key so
//! it outlives a restart and every app shows the same draft. Keys name what a draft is
//! for: `git:<repository>`, `chat:<agent>`. Beside its text a draft can hold `extra`,
//! JSON only the apps read, such as a message's attachments.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::kept::Kept;
use super::{CoreError, CoreResult};

const FILE: &str = "drafts.json";
/// The longest draft kept, in bytes.
const MAX_DRAFT: usize = 256 * 1024;
/// Past this many drafts the oldest goes.
const MAX_DRAFTS: usize = 512;
const MAX_KEY: usize = 512;

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    shown: Draft,
    /// When it was last typed, in ms since the epoch, so the oldest goes first.
    at: u64,
}

/// A draft as the apps read it.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Draft {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extra: Option<Value>,
}

pub(crate) struct Drafts(Arc<Held>);

struct Held {
    drafts: Mutex<BTreeMap<String, Stored>>,
    kept: Kept,
    /// A save is on its way and will write what `drafts` holds by then.
    saving: AtomicBool,
    /// Taken for each save, so a later one never writes before an earlier.
    writing: Mutex<()>,
}

impl Drafts {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self(Arc::new(Held {
            drafts: Mutex::new(kept.load()),
            kept,
            saving: AtomicBool::new(false),
            writing: Mutex::new(()),
        }))
    }

    /// The text of the draft under `key`, taken out of it; whatever the draft
    /// holds beside its text stays.
    pub(crate) fn take_text(&self, key: &str) -> CoreResult<String> {
        let Some(draft) = self.get(Some(&[key.to_owned()])).remove(key) else {
            return Ok(String::new());
        };
        self.set(key, String::new(), draft.extra)?;
        Ok(draft.text)
    }

    /// The drafts under `keys`, else every one, by key.
    pub(crate) fn get(&self, keys: Option<&[String]>) -> BTreeMap<String, Draft> {
        let drafts = self.0.drafts.locked();
        match keys {
            Some(keys) => keys
                .iter()
                .filter_map(|key| Some((key.clone(), drafts.get(key)?.shown.clone())))
                .collect(),
            None => drafts
                .iter()
                .map(|(key, draft)| (key.clone(), draft.shown.clone()))
                .collect(),
        }
    }

    /// Keeps `text` and `extra` as the draft for `key`; an empty text with no extra
    /// drops it.
    pub(crate) fn set(&self, key: &str, text: String, extra: Option<Value>) -> CoreResult<()> {
        if key.is_empty() || key.len() > MAX_KEY {
            return Err(CoreError::from("a draft's key must be 1 to 512 bytes"));
        }
        let extra = extra.filter(|extra| !extra.is_null());
        let size = text.len() + extra.as_ref().map_or(0, |extra| extra.to_string().len());
        if size > MAX_DRAFT {
            return Err(CoreError::from(
                "that draft is longer than the 256 KB a draft can be",
            ));
        }
        {
            let mut drafts = self.0.drafts.locked();
            if text.is_empty() && extra.is_none() {
                drafts.remove(key);
            } else {
                let shown = Draft { text, extra };
                drafts.insert(
                    key.to_owned(),
                    Stored {
                        shown,
                        at: epoch_ms(),
                    },
                );
            }
            while drafts.len() > MAX_DRAFTS {
                let Some(oldest) = drafts
                    .iter()
                    .min_by_key(|(_, draft)| draft.at)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                drafts.remove(&oldest);
            }
        }
        self.save_soon();
        Ok(())
    }

    /// Drops the drafts kept under `keys`, answering the ones there were.
    pub(crate) fn forget(&self, keys: impl IntoIterator<Item = String>) -> Vec<String> {
        let forgotten: Vec<String> = {
            let mut drafts = self.0.drafts.locked();
            keys.into_iter()
                .filter(|key| drafts.remove(key).is_some())
                .collect()
        };
        if !forgotten.is_empty() {
            self.save_soon();
        }
        forgotten
    }

    /// Saves off the caller's thread; a burst of keystrokes is written once each save
    /// has finished.
    fn save_soon(&self) {
        if self.0.saving.swap(true, Ordering::AcqRel) {
            return;
        }
        let held = self.0.clone();
        let save = move || {
            let _writing = held.writing.locked();
            held.saving.store(false, Ordering::Release);
            let drafts = held.drafts.locked().clone();
            held.kept.save(&drafts);
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(save)),
            Err(_) => save(),
        }
    }
}
