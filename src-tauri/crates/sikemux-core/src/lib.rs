//! The background process that owns Sikemux terminals and chat agents, so
//! they outlive the window. `sikemux core` runs [`server`]; the app talks to
//! it with [`client`].

pub mod accounts;
pub mod acp;
#[cfg(unix)]
pub mod agent_accounts;
#[cfg(unix)]
mod agent_shell;
#[rustfmt::skip]
pub mod agent_catalog;
mod agent_ops;
mod backoff;
pub mod cli;
#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod files;
mod flights;
pub mod frozen;
pub mod fuzzy;
#[cfg(unix)]
pub mod git;
#[cfg(unix)]
pub mod git_guard;
pub mod harness;
pub mod host_update;
#[cfg(unix)]
pub mod join;
#[cfg(unix)]
pub mod launch;
pub use sikemux_base::{download, http, pages, runtime};
pub use sikemux_wire::grid;
pub use sikemux_wire::protocol;
#[cfg(unix)]
pub mod push;
pub mod release_feed;
pub mod releases;
#[cfg(unix)]
pub use sikemux_client::device;
#[cfg(unix)]
pub use sikemux_client::remote;
#[cfg(unix)]
#[cfg(unix)]
pub mod server;
#[cfg(unix)]
pub mod sessions;
pub mod skills;
pub mod tasks;
mod text;
#[cfg(unix)]
pub mod timeout;
pub mod trailers;
pub mod usage;
#[cfg(unix)]
pub mod waiting;

use std::path::PathBuf;

pub const SOCKET_ENV: &str = "SIKEMUX_CORE_SOCKET";

/// `~/.config/sikemux/core.sock`, or `core.dev.sock` in debug builds, unless
/// `SIKEMUX_CORE_SOCKET` names another path.
pub fn default_socket_path() -> Option<PathBuf> {
    if let Some(path) = sikemux_process::paths::env_path(SOCKET_ENV) {
        return Some(path);
    }
    sikemux_process::paths::config_file("core", "sock")
}
