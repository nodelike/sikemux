//! The chat agents the core starts and speaks to: Claude Code over its
//! stream-json control protocol, Codex over its app server, Muse Code over its
//! session protocol, and every other agent over ACP. Each tells the chat the
//! same ACP-shaped session updates, so the app reads them alike.

mod acp;
mod claude;
mod codex;
mod json;
mod muse;
mod parts;
mod patch;
mod pending;
mod process;
mod rpc;
mod servers;
mod session;

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::agent_catalog::ChatDriver;
use crate::protocol::ChatLaunch;

use super::rebind::Outcome;
use super::{Chat, ChatCommand};

pub(crate) use process::{kill_group, AgentRecord, Released};
pub(crate) use session::{Adopted, Begin, Handed, SessionState};

/// Stops an agent that was let go of but that no core will take up.
pub(crate) fn kill_agent(pid: u32) {
    process::kill_and_reap(pid);
}

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

/// How an agent is shown a file the person attached: a markdown link named
/// for the file, the way the agents' own clients write it.
fn file_link(name: Option<&str>, uri: &str) -> String {
    match name.filter(|name| !name.is_empty()) {
        Some(name) => format!("[@{name}]({uri})"),
        None => match uri.strip_prefix("file://") {
            Some(path) => format!("[@{}]({uri})", path.rsplit('/').next().unwrap_or(path)),
            None => uri.to_owned(),
        },
    }
}

pub(super) async fn run(
    chat: Arc<Chat>,
    launch: ChatLaunch,
    commands: &mut mpsc::UnboundedReceiver<ChatCommand>,
    begin: Begin,
) -> Result<Outcome, String> {
    let driver = crate::agent_catalog::agent(chat.provider())
        .and_then(|agent| agent.chat.as_ref())
        .ok_or_else(|| format!("{} has no chat", chat.provider()))?;
    match driver {
        ChatDriver::Claude => session::run::<claude::Claude>(chat, launch, commands, begin).await,
        ChatDriver::Codex => session::run::<codex::Codex>(chat, launch, commands, begin).await,
        ChatDriver::Muse => session::run::<muse::Muse>(chat, launch, commands, begin).await,
        ChatDriver::Acp { .. } => session::run::<acp::Acp>(chat, launch, commands, begin).await,
    }
}
