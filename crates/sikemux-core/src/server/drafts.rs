//! What the person has typed and not sent yet, such as a commit message, kept by key so
//! it outlives a restart and every app shows the same draft. Keys name what a draft is
//! for: `git:<repository>`, `chat:<agent>`. Beside its text a draft can hold `extra`,
//! JSON only the apps read, such as a message's attachments.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
pub use sikemux_calls::drafts::{Draft, DraftsParams, SetDraftParams};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use sikemux_view::chat::{chat_draft_key, ChatDraft, DraftExtra, CHAT_DRAFT};
use sikemux_wire::protocol::ChatContext;

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

/// What the core itself put in a draft, such as an item shared to the chat: a file and words
/// added after the text. It is put back into an app's save until the app has read a revision
/// holding it, so a save already on its way cannot lose it.
#[derive(Clone)]
pub(crate) struct Delivery {
    pub paths: Vec<String>,
    /// A link or text, as the draft's chips hold it and the message sends it.
    pub context: Option<ChatContext>,
    pub words: String,
}

impl Delivery {
    fn add_to(&self, text: &mut String, extra: &mut Option<DraftExtra>) {
        if !self.paths.is_empty() || self.context.is_some() {
            let extra = extra.get_or_insert_default();
            for path in &self.paths {
                if !extra.attachments.contains(path) {
                    extra.attachments.push(path.clone());
                }
            }
            if let Some(context) = &self.context {
                if !extra.context.iter().any(|kept| kept.uri == context.uri) {
                    extra.context.push(context.clone());
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

/// The drafts under `keys` changed: every app reads them again, and a chat's clients hear its
/// draft with the chat.
pub(crate) fn changed(core: &Core, keys: Vec<String>) {
    for agent in keys.iter().filter_map(|key| key.strip_prefix(CHAT_DRAFT)) {
        (core.workspace_above().chat_draft_changed)(core, agent);
    }
    core.changed(What::ReadIn(Topic::Drafts, None, Some(keys)));
}

pub(crate) struct Drafts(Held);

struct Held {
    drafts: Mutex<BTreeMap<String, Stored>>,
    /// The deliveries no app has read yet, by draft key, each with the revision it made.
    delivered: Mutex<BTreeMap<String, Vec<(u64, Delivery)>>>,
    /// The last revision given; it starts at the time so it keeps growing across restarts.
    rev: AtomicU64,
    kept: Kept,
}

impl Drafts {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self(Held {
            drafts: Mutex::new(kept.load()),
            delivered: Mutex::default(),
            rev: AtomicU64::new(epoch_ms()),
            kept,
        })
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
        let draft = &drafts.get(&chat_draft_key(agent))?.shown;
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
    /// drops it. What the core delivered to the draft before is part of this write, so
    /// no app's later save puts it back.
    pub(crate) fn set(&self, key: &str, text: String, extra: Option<DraftExtra>) -> CoreResult<()> {
        let mut drafts = self.0.drafts.locked();
        self.put(&mut drafts, key, text, extra)?;
        self.0.delivered.locked().remove(key);
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
        mut extra: Option<DraftExtra>,
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
        extra: Option<DraftExtra>,
    ) -> CoreResult<u64> {
        if key.is_empty() || key.len() > MAX_KEY {
            return Err(CoreError::from("a draft's key must be 1 to 512 bytes"));
        }
        let size = text.len()
            + extra
                .as_ref()
                .and_then(|extra| serde_json::to_string(extra).ok())
                .map_or(0, |extra| extra.len());
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

    fn save_soon(&self) {
        self.0.kept.save_soon(&*self.0.drafts.locked());
    }
}

impl super::part::Part for Drafts {
    fn flush(&self) {
        self.0.kept.flush();
    }
}

/// Keeps the draft `params` names and tells every client.
pub(crate) fn set(core: &Core, params: SetDraftParams) -> CoreResult<()> {
    core.drafts()
        .save(&params.key, params.text, params.extra, params.rev)?;
    changed(core, vec![params.key]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stashed prompt came back into the phone's draft: the stash emptied the draft, which
    /// left no revision for the phone to name, and its next save brought back what the core
    /// had delivered before the stash.
    #[test]
    fn a_draft_the_core_empties_keeps_no_delivery_for_the_next_save() {
        let drafts = Drafts::new(None);
        let key = "chat:agent";
        let delivery = Delivery {
            paths: Vec::new(),
            context: None,
            words: "a prompt worth keeping".into(),
        };
        drafts.deliver(key, delivery).unwrap();
        drafts.set(key, String::new(), None).unwrap();
        drafts.save(key, "see".into(), None, None).unwrap();
        assert_eq!(drafts.get(Some(&[key.to_owned()]))[key].text, "see");
    }
}
