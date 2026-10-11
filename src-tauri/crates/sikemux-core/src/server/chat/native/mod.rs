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

/// What a client rebuilding a chat needs to tell its turns apart: when a
/// replayed update first happened, in Unix milliseconds, and whether it began
/// a turn the agent started by itself.
fn stamp_replayed(update: &mut Value, at: Option<u64>, opens_turn: bool) {
    let Some(fields) = update
        .as_object_mut()
        .filter(|_| at.is_some() || opens_turn)
    else {
        return;
    };
    let meta = fields.entry("_meta").or_insert_with(|| json!({}));
    let Some(sikemux) = meta
        .as_object_mut()
        .map(|meta| meta.entry("sikemux").or_insert_with(|| json!({})))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if let Some(at) = at {
        sikemux.insert("at".into(), json!(at));
    }
    if opens_turn {
        sikemux.insert("opensTurn".into(), json!(true));
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
