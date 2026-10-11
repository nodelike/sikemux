//! Agents the core speaks to in their own protocol rather than ACP: Claude
//! Code over its stream-json control protocol and Codex over its app server.
//! Both tell the chat the same ACP-shaped session updates every other agent
//! does, so the app reads them alike.

mod claude;
mod codex;
mod process;
mod rpc;
mod session;

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::protocol::ChatLaunch;

use super::rebind::{Outcome, Rebind};
use super::{Chat, ChatCommand};

/// When a replayed update first happened, in Unix milliseconds, so a client
/// rebuilding the chat can tell how long each turn took.
fn stamp_replayed(update: &mut Value, at: u64) {
    if let Some(fields) = update.as_object_mut() {
        let meta = fields.entry("_meta").or_insert_with(|| json!({}));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert("sikemux".into(), json!({ "at": at }));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Claude,
    Codex,
}

impl Kind {
    pub fn of(provider: &str) -> Option<Self> {
        match provider {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

pub(super) async fn run(
    kind: Kind,
    chat: Arc<Chat>,
    launch: ChatLaunch,
    commands: &mut mpsc::UnboundedReceiver<ChatCommand>,
    rebind: Option<Box<Rebind>>,
) -> Result<Outcome, String> {
    match kind {
        Kind::Claude => session::run::<claude::Claude>(chat, launch, commands, rebind).await,
        Kind::Codex => session::run::<codex::Codex>(chat, launch, commands, rebind).await,
    }
}
