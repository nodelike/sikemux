//! What waits to go to each linked host while it is out of reach, in the
//! order it was sent, kept in `host-outbox.json` so it outlives a restart.
//! Only the newest of what replaces itself, such as an agent's state, waits.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::super::kept::Kept;

const FILE: &str = "host-outbox.json";
/// Bundles waiting to go, beside the outbox.
const BODIES: &str = "host-outbox";
const MAX_POSTS: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Post {
    pub seq: u64,
    pub method: String,
    pub params: Value,
    /// A later post with the same key replaces this one.
    pub key: Option<String>,
    /// A file that goes with it, removed once it has gone.
    pub body: Option<PathBuf>,
    /// Never dropped to make room, such as a worker's report to its lead.
    #[serde(default)]
    pub keep: bool,
    /// The agent here that hears when the other host turns it away.
    #[serde(default)]
    pub from: Option<String>,
    /// Names it to the other host, which does it once however often it is sent.
    #[serde(default)]
    pub call: Option<String>,
    pub at: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    next: u64,
    posts: BTreeMap<String, VecDeque<Post>>,
}

pub(crate) struct Outbox {
    record: Mutex<Record>,
    kept: Kept,
    bodies: PathBuf,
}

pub(crate) struct Sending {
    method: &'static str,
    params: Value,
    key: Option<String>,
    body: Option<PathBuf>,
    keep: bool,
    from: Option<String>,
    call: Option<String>,
}

impl Sending {
    pub(crate) fn new(method: &'static str, params: Value) -> Self {
        Self {
            method,
            params,
            key: None,
            body: None,
            keep: false,
            from: None,
            call: None,
        }
    }

    /// Replaces whatever waits under the same `key`.
    pub(crate) fn keyed(mut self, key: String) -> Self {
        self.key = Some(key);
        self
    }

    /// Must arrive: never dropped to make room.
    pub(crate) fn kept(mut self) -> Self {
        self.keep = true;
        self
    }

    pub(crate) fn body(mut self, body: PathBuf) -> Self {
        self.body = Some(body);
        self
    }

    /// Sends it as the call `id`, already tried directly.
    pub(crate) fn call(mut self, id: String) -> Self {
        self.call = Some(id);
        self
    }

    /// `agent` hears if the other host turns it away.
    pub(crate) fn from(mut self, agent: &str) -> Self {
        self.from = Some(agent.to_owned());
        self
    }
}

impl Outbox {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            record: Mutex::new(kept.load()),
            kept,
            bodies: data_dir.map_or_else(
                || std::env::temp_dir().join("sikemux-host-outbox"),
                |dir| dir.join(BODIES),
            ),
        }
    }

    /// Where a file to send waits until it has gone.
    pub(crate) fn body_path(&self, name: &str) -> PathBuf {
        let _ = std::fs::create_dir_all(&self.bodies);
        self.bodies.join(name)
    }

    pub(crate) fn push(&self, host: &str, sending: Sending) {
        let mut record = self.record.locked();
        record.next += 1;
        let call = (sending.call).or_else(|| sending.key.is_none().then(super::call_id));
        let post = Post {
            seq: record.next,
            method: sending.method.to_owned(),
            params: sending.params,
            key: sending.key,
            body: sending.body,
            keep: sending.keep,
            from: sending.from,
            call,
            at: epoch_ms(),
        };
        let posts = record.posts.entry(host.to_owned()).or_default();
        if let Some(key) = &post.key {
            posts.retain(|waiting| {
                let replaced = waiting.key.as_ref() == Some(key);
                if replaced {
                    drop_body(waiting);
                }
                !replaced
            });
        }
        posts.push_back(post);
        while posts.len() > MAX_POSTS {
            let oldest = posts.iter().position(|waiting| !waiting.keep).unwrap_or(0);
            if let Some(dropped) = posts.remove(oldest) {
                eprintln!(
                    "sikemux core: dropped a {} waiting for a linked host, which had {MAX_POSTS} waiting",
                    dropped.method
                );
                drop_body(&dropped);
            }
        }
        self.kept.save(&*record);
    }

    pub(crate) fn front(&self, host: &str) -> Option<Post> {
        self.record.locked().posts.get(host)?.front().cloned()
    }

    pub(crate) fn has(&self, host: &str) -> bool {
        self.record
            .locked()
            .posts
            .get(host)
            .is_some_and(|posts| !posts.is_empty())
    }

    /// Takes away the post `seq`, which has gone.
    pub(crate) fn sent(&self, host: &str, seq: u64) {
        let mut record = self.record.locked();
        let Some(posts) = record.posts.get_mut(host) else {
            return;
        };
        posts.retain(|post| {
            let gone = post.seq == seq;
            if gone {
                drop_body(post);
            }
            !gone
        });
        if posts.is_empty() {
            record.posts.remove(host);
        }
        self.kept.save(&*record);
    }

    pub(crate) fn forget(&self, host: &str) {
        let mut record = self.record.locked();
        if let Some(posts) = record.posts.remove(host) {
            posts.iter().for_each(drop_body);
            self.kept.save(&*record);
        }
    }

    /// Drops what waits for hosts no longer linked, as a restart finds it.
    pub(crate) fn keep_only(&self, linked: &[String]) {
        let gone: Vec<String> = self
            .record
            .locked()
            .posts
            .keys()
            .filter(|host| !linked.contains(host))
            .cloned()
            .collect();
        for host in gone {
            self.forget(&host);
        }
    }
}

fn drop_body(post: &Post) {
    if let Some(body) = &post.body {
        let _ = std::fs::remove_file(body);
    }
}
