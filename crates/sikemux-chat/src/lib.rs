//! Chats: an agent's conversation as the core keeps it, from its start and its turns to its
//! feed, its replay from disk, hand-overs between accounts and agents, and the model proxy.
//! It stands on sikemux-workspace and the drivers, and knows nothing of the board, the inbox
//! or the other features built over it; what it asks of them it asks through
//! [`ChatAbove`](server::chat_above::ChatAbove), which the core fills.

#![cfg(unix)]

pub mod server;

use sikemux_agents::{acp, agent_accounts, agent_catalog, launch, sessions};
use sikemux_base::pages;
