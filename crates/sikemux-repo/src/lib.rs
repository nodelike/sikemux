//! What the core does with a project's repository: recording who wrote which lines and
//! committing, landing and pushing them, collisions between agents, the project's check and its
//! culprit search, the code host and its CI, places and deploys held for the person, the
//! decisions put to the person, language servers and the skill library. It stands on
//! sikemux-remote and knows nothing of the board, messages between agents, the roster, tasks or
//! the other features built over it; what it asks of them it asks through
//! [`RepoAbove`](server::repo_above::RepoAbove), which the core fills.

#![cfg(unix)]

pub mod server;

use sikemux_agents::{agent_catalog, git, git_guard, ship, skills};
use sikemux_base::http;
use sikemux_corekit::{backoff, cli, tasks, text, trailers};
use sikemux_git::files;
