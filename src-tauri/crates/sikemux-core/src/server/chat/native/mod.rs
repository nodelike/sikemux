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

fn driver(provider: &str) -> Option<&'static ChatDriver> {
    crate::agent_catalog::agent(provider).and_then(|agent| agent.chat.as_ref())
}

/// Whether a resumed chat of `provider` can show its saved history before its agent starts:
/// only Claude's is a file the core reads itself.
fn shows_saved(provider: &str) -> bool {
    matches!(driver(provider), Some(ChatDriver::Claude))
}

/// The models and efforts `launch`'s agent offers, asked of its CLI without a session.
/// Only Claude and Codex tell them outside one; an ACP agent offers them in `session/new`.
pub(crate) async fn read_options(launch: &ChatLaunch) -> Result<Value, String> {
    let mut command = tokio::process::Command::from(
        sikemux_process::user_environment::agent_command(&launch.program),
    );
    command
        .envs(&launch.env)
        .envs(
            launch
                .account
                .iter()
                .flat_map(|account| account.env.clone()),
        )
        .kill_on_drop(true);
    match driver(&launch.provider) {
        Some(ChatDriver::Claude) => claude::read_options(command).await,
        Some(ChatDriver::Codex) => codex::read_options(command).await,
        _ => Err(format!(
            "{} offers its options only in a session",
            launch.provider
        )),
    }
}

/// Shows the saved history of the session a waiting chat resumes, when the core can read it.
pub(crate) fn show_saved(chat: &Arc<Chat>) {
    let Some(session) = chat.launch.resume_id.clone() else {
        return;
    };
    if !shows_saved(chat.provider()) {
        return;
    }
    if chat
        .saved_shown
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    let chat = chat.clone();
    tokio::task::spawn_blocking(move || {
        let sink = session::Sink::showing(chat.clone());
        if let Err(error) = claude::replay_saved(&chat.launch, &session, &sink) {
            eprintln!(
                "sikemux core: could not show {}'s saved chat: {error}",
                chat.agent_id()
            );
        }
    });
}

pub(super) async fn run(
    chat: Arc<Chat>,
    launch: ChatLaunch,
    commands: &mut mpsc::UnboundedReceiver<ChatCommand>,
    begin: Begin,
) -> Result<Outcome, String> {
    let driver =
        driver(chat.provider()).ok_or_else(|| format!("{} has no chat", chat.provider()))?;
    match driver {
        ChatDriver::Claude => session::run::<claude::Claude>(chat, launch, commands, begin).await,
        ChatDriver::Codex => session::run::<codex::Codex>(chat, launch, commands, begin).await,
        ChatDriver::Muse => session::run::<muse::Muse>(chat, launch, commands, begin).await,
        ChatDriver::Acp { .. } => session::run::<acp::Acp>(chat, launch, commands, begin).await,
    }
}
