//! The workspace and the state kept beside it: the projects and agents a host shows, their
//! names, the preferences, the activity log, drafts, the files and searches clients open. It
//! stands on sikemux-serve and knows nothing of chats, the board or the other features built
//! over it; what it asks of them it asks through
//! [`WorkspaceAbove`](server::workspace_above::WorkspaceAbove), which the core fills.

#![cfg(unix)]

pub mod server;

use sikemux_agents::{acp, agent_accounts, agent_catalog, git, launch, sessions};
use sikemux_base::http;
use sikemux_corekit::{cli, fuzzy, text};
