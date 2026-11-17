//! The board: a project's cards, which the person and agents add, claim, update and finish. It
//! stands on sikemux-repo and knows nothing of messages between agents, questions, the inbox,
//! hosts or the other features built over it; what it asks of them it asks through
//! [`BoardAbove`](server::board_above::BoardAbove), which the core fills.

pub mod server;

use sikemux_agents::git;
use sikemux_corekit::{cli, text};
