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

/// What to send a host.
pub(crate) struct Sending {
    pub method: &'static str,
    pub params: Value,
    pub key: Option<String>,
    pub body: Option<PathBuf>,
    pub keep: bool,
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

    /// Queues `sending` behind what already waits for `host`.
    pub(crate) fn push(&self, host: &str, sending: Sending) {
        let mut record = self.record.locked();
        record.next += 1;
        let post = Post {
            seq: record.next,
            method: sending.method.to_owned(),
            params: sending.params,
            key: sending.key,
            body: sending.body,
            keep: sending.keep,
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

    /// Drops everything waiting for a host no longer linked.
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
