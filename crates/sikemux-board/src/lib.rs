//! The board: a project's cards, goals and slices, the checks and proof a slice passes, and Auto,
//! which starts, follows, recovers and finishes the runs that build them, here or on a linked
//! host. It stands on sikemux-repo and knows nothing of messages between agents, questions, the
//! inbox, hosts or the other features built over it; what it asks of them it asks through
//! [`BoardAbove`](server::board_above::BoardAbove), which the core fills.

#![cfg(unix)]

pub mod server;

use sikemux_agents::git;
use sikemux_corekit::{cli, tasks, text};
