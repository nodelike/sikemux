//! The background process that owns Sikemux terminals and chat agents, so
//! they outlive the window. `sikemux core` runs [`server`]; the app talks to
//! it with [`client`].

use sikemux_agents::agent_shell;
#[cfg(test)]
use sikemux_agents::ship;
pub use sikemux_agents::{acp, agent_catalog, harness, skills};
pub use sikemux_agents::{agent_accounts, git, git_guard, launch, sessions};
pub use sikemux_base::{download, http, pages, runtime};
pub use sikemux_corekit::accounts;
use sikemux_corekit::{backoff, flights, text};
pub use sikemux_corekit::{
    build, cli, frozen, fuzzy, host_update, release_feed, releases, tasks, trailers,
};
pub use sikemux_corekit::{client, join, push, timeout, waiting};
pub use sikemux_corekit::{default_socket_path, SOCKET_ENV};
pub use sikemux_git::files;
pub mod server;
