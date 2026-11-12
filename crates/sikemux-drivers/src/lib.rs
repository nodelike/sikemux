//! The agents a chat can run on, each driven over its own protocol: Claude Code, Codex, Pi,
//! Muse and those that speak ACP. A driver starts its agent, sends it prompts and tells the
//! chat what it says through a [`Sink`](server::chat::native::session::Sink); it knows nothing
//! of the core the chat runs in.

#![cfg(unix)]

pub mod server;

use sikemux_agents::{acp, agent_accounts, agent_catalog, git_guard, launch, sessions, skills};
use sikemux_corekit::accounts;
