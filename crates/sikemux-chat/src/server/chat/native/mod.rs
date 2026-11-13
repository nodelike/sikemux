//! The chat agents the core starts and speaks to: Claude Code over its
//! stream-json control protocol, Codex over its app server, Muse Code over its
//! session protocol, Pi over its RPC mode, and every other agent over ACP. Each tells the chat the
//! same ACP-shaped session updates, so the app reads them alike.

mod session;

use crate::server::chat_state::ChatParts;
use sikemux_drivers::server::chat::native::{acp, claude, codex, muse, pi, process};

use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::agent_catalog::ChatDriver;
use sikemux_wire::protocol::ChatLaunch;

use super::rebind::Outcome;
use super::{Chat, ChatCommand};

pub use claude::{saved_window, terminal_cache_env, AutoCompact};
pub use process::{kill_and_reap, kill_group, AgentRecord, Released};
pub use session::{Adopted, Begin, Handed, SessionState, Steered};

/// Why a session ended, so the chat can tell a stop it asked for from an agent
/// that died under it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionEnd {
    Requested,
    Exited,
    Failed,
}

pub fn ended_status(result: &Result<SessionEnd, String>, started: bool) -> Value {
    match result {
        Ok(end) => json!({ "state": "stopped", "reason": end }),
        Err(message) => json!({
            "state": "error",
            "reason": if started { SessionEnd::Exited } else { SessionEnd::Failed },
            "message": message,
        }),
    }
}

fn driver(provider: &str) -> Option<&'static ChatDriver> {
    crate::agent_catalog::agent(provider).and_then(|agent| agent.chat.as_ref())
}

/// Whether a resumed chat of `provider` can show its saved history before its agent starts.
fn shows_saved(provider: &str) -> bool {
    crate::agent_catalog::agent(provider).is_some_and(|agent| agent.history_before_start)
}

/// The models and efforts `launch`'s agent offers, asked of its CLI without a session.
pub async fn read_options(launch: &ChatLaunch) -> Result<Value, String> {
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
        Some(ChatDriver::Pi) => pi::read_options(command).await,
        Some(ChatDriver::Muse | ChatDriver::Acp { .. }) | None => Err(format!(
            "{} offers its options only in a session",
            launch.provider
        )),
    }
}

/// Shows the saved history of the session a waiting chat resumes, when the core can read it.
pub fn show_saved(chat: &Arc<Chat>) {
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
    chat.saved_reading.send_replace(true);
    let chat = chat.clone();
    tokio::task::spawn_blocking(move || {
        let sink = session::ChatTelling::new(chat.clone());
        let shown = match driver(chat.provider()) {
            Some(ChatDriver::Claude) => claude::replay_saved(&chat.launch, &session, &sink),
            Some(ChatDriver::Acp { .. }) => acp::replay_saved(&chat.launch, &session, &sink),
            Some(ChatDriver::Codex | ChatDriver::Muse | ChatDriver::Pi) | None => Err(format!(
                "Sikemux reads no saved chat of {} without its agent",
                chat.provider()
            )),
        };
        if let Err(error) = shown {
            chat.saved_shown
                .store(false, std::sync::atomic::Ordering::Release);
            eprintln!(
                "sikemux core: could not show {}'s saved chat: {error}",
                chat.agent_id()
            );
        }
        chat.saved_reading.send_replace(false);
        if let Some(core) = chat.core.upgrade() {
            (core.chat_above().chief_chat_ready)(&core, &chat);
        }
    });
}

pub async fn run(
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
        ChatDriver::Pi => session::run::<pi::Pi>(chat, launch, commands, begin).await,
        ChatDriver::Acp { .. } => session::run::<acp::Acp>(chat, launch, commands, begin).await,
    }
}
