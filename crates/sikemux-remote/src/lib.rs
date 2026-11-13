//! Reaching a host from elsewhere and its account: pairing devices, listening for them, the
//! account and its devices, the vault of sign-ins, joining, sharing a chat, and telling the
//! person's devices what happened. It stands on sikemux-chat and knows nothing of hosts'
//! teams, the inbox or the other features built over it; what it asks of them it asks through
//! [`RemoteAbove`](server::remote_above::RemoteAbove), which the core fills.

#![cfg(unix)]

pub mod server;

use sikemux_agents::agent_catalog;
use sikemux_base::http;
use sikemux_corekit::{accounts, backoff, join, push, text};
