//! What the person has typed and not sent yet, such as a commit message, kept by key so
//! it outlives a restart and every app shows the same draft. Keys name what a draft is
//! for: `git:<repository>`, `chat:<agent>`. Beside its text a draft can hold `extra`,
//! JSON only the apps read, such as a message's attachments.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use sikemux_wire::chat::ChatDraft;

use super::changed::Topic;
use super::kept::Kept;
use super::{Core, CoreError, CoreResult};

const FILE: &str = "drafts.json";
/// The longest draft kept, in bytes.
const MAX_DRAFT: usize = 256 * 1024;
/// Past this many drafts the oldest goes.
const MAX_DRAFTS: usize = 512;
const MAX_KEY: usize = 512;
/// Past this many unseen deliveries to one draft the oldest is no longer put back.
const MAX_DELIVERED: usize = 64;

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    shown: Draft,
    /// When it was last typed, in ms since the epoch, so the oldest goes first.
    at: u64,
}

/// A draft as the apps read it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    /// What the app keeps beside the text, such as its chips and files, as the app shaped it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
    /// Moves on with every change, so an app saving the draft says which one it last read.
    #[serde(default)]
    pub rev: u64,
}

/// What the core itself put in a draft, such as an item shared to the chat: a file and words
/// added after the text. It is put back into an app's save until the app has read a revision
/// holding it, so a save already on its way cannot lose it.
#[derive(Clone)]
pub(crate) struct Delivery {
    pub paths: Vec<String>,
    /// A link or text, as the draft's chips hold it and the message sends it: `{uri, title, text}`.
    pub context: Option<Value>,
    pub words: String,
}

/// The list under `key` in the draft's extra, made when there is none.
fn list<'a>(extra: &'a mut Option<Value>, key: &str) -> Option<&'a mut Vec<Value>> {
    if !matches!(extra, Some(Value::Object(_))) {
        *extra = Some(Value::Object(Default::default()));
    }
    let Some(Value::Object(extra)) = extra else {
        return None;
    };
    extra
        .entry(key)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
}

impl Delivery {
    fn add_to(&self, text: &mut String, extra: &mut Option<Value>) {
        for path in &self.paths {
            if let Some(attachments) = list(extra, "attachments") {
                if !attachments.iter().any(|kept| kept.as_str() == Some(path)) {
                    attachments.push(Value::from(path.as_str()));
                }
            }
        }
        if let Some(context) = &self.context {
            if let Some(held) = list(extra, "context") {
                if !held.iter().any(|kept| kept["uri"] == context["uri"]) {
                    held.push(context.clone());
                }
            }
        }
        let words = self.words.trim();
        if words.is_empty() || text.contains(words) {
            return;
        }
        let written = text.trim_end().len();
        text.truncate(written);
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(words);
    }
}

/// The key a chat's draft is kept under.
pub(crate) fn chat_key(agent: &str) -> String {
    format!("chat:{agent}")
}

/// The drafts under `keys` changed: every app reads them again, and a chat's clients hear its
/// draft with the chat.
pub(crate) fn changed(core: &Core, keys: Vec<String>) {
    for agent in keys.iter().filter_map(|key| key.strip_prefix("chat:")) {
        if let Some(feed) = core.chats.heard(agent) {
            feed.set_draft(core.drafts.chat_draft(agent));
        }
    }
    core.changes.changed(Topic::Drafts, None, Some(keys));
}

pub(crate) struct Drafts(Arc<Held>);

struct Held {
    drafts: Mutex<BTreeMap<String, Stored>>,
    /// The deliveries no app has read yet, by draft key, each with the revision it made.
    delivered: Mutex<BTreeMap<String, Vec<(u64, Delivery)>>>,
    /// The last revision given; it starts at the time so it keeps growing across restarts.
    rev: AtomicU64,
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
            delivered: Mutex::default(),
            rev: AtomicU64::new(epoch_ms()),
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

    /// The draft of `agent`'s chat, as the chat's clients are told it.
    pub(crate) fn chat_draft(&self, agent: &str) -> Option<ChatDraft> {
        let drafts = self.0.drafts.locked();
        let draft = &drafts.get(&chat_key(agent))?.shown;
        Some(ChatDraft {
            text: draft.text.clone(),
            extra: draft.extra.clone(),
            rev: draft.rev,
        })
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
        let mut drafts = self.0.drafts.locked();
        self.put(&mut drafts, key, text, extra)?;
        drop(drafts);
        self.save_soon();
        Ok(())
    }

    /// An app's save of the draft for `key`, written over revision `read`, the last it read.
    /// What the core delivered after that revision is put back in, as the app never saw it.
    pub(crate) fn save(
        &self,
        key: &str,
        mut text: String,
        mut extra: Option<Value>,
        read: Option<u64>,
    ) -> CoreResult<()> {
        let mut drafts = self.0.drafts.locked();
        let mut delivered = self.0.delivered.locked();
        let unseen: Vec<Delivery> = delivered
            .get(key)
            .into_iter()
            .flatten()
            .filter(|(rev, _)| read.is_none_or(|read| *rev > read))
            .map(|(_, delivery)| delivery.clone())
            .collect();
        for delivery in &unseen {
            delivery.add_to(&mut text, &mut extra);
        }
        let rev = self.put(&mut drafts, key, text, extra)?;
        delivered.remove(key);
        if read.is_some() && !unseen.is_empty() {
            delivered.insert(
                key.to_owned(),
                unseen.into_iter().map(|delivery| (rev, delivery)).collect(),
            );
        }
        drop((drafts, delivered));
        self.save_soon();
        Ok(())
    }

    /// Adds `delivery` to the draft for `key`, after what is written there.
    pub(crate) fn deliver(&self, key: &str, delivery: Delivery) -> CoreResult<()> {
        let mut drafts = self.0.drafts.locked();
        let (mut text, mut extra) = drafts
            .get(key)
            .map(|draft| (draft.shown.text.clone(), draft.shown.extra.clone()))
            .unwrap_or_default();
        delivery.add_to(&mut text, &mut extra);
        let rev = self.put(&mut drafts, key, text, extra)?;
        let mut delivered = self.0.delivered.locked();
        let unseen = delivered.entry(key.to_owned()).or_default();
        unseen.push((rev, delivery));
        if unseen.len() > MAX_DELIVERED {
            unseen.remove(0);
        }
        drop((drafts, delivered));
        self.save_soon();
        Ok(())
    }

    fn put(
        &self,
        drafts: &mut BTreeMap<String, Stored>,
        key: &str,
        text: String,
        extra: Option<Value>,
    ) -> CoreResult<u64> {
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
        let rev = self.0.rev.fetch_add(1, Ordering::AcqRel) + 1;
        if text.is_empty() && extra.is_none() {
            drafts.remove(key);
        } else {
            let shown = Draft { text, extra, rev };
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
        Ok(rev)
    }

    /// Drops the drafts kept under `keys`, answering the ones there were.
    pub(crate) fn forget(&self, keys: impl IntoIterator<Item = String>) -> Vec<String> {
        let forgotten: Vec<String> = {
            let mut drafts = self.0.drafts.locked();
            let mut delivered = self.0.delivered.locked();
            keys.into_iter()
                .filter(|key| {
                    delivered.remove(key);
                    drafts.remove(key).is_some()
                })
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
