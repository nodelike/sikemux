//! What a core stands on: its terminals, the clients connected to it, who may ask what, and
//! saying that something changed. It knows nothing of the features built over it; what it
//! asks of them it asks through [`Above`](server::above::Above), which the core fills.

#![cfg(unix)]

pub mod server;

use sikemux_agents::{agent_catalog, git_guard, launch};
use sikemux_corekit::host_update;
