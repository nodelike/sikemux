//! The background process that owns Sikemux terminals and chat agents, so
//! they outlive the window. `sikemux core` runs [`server`]; the app talks to
//! it with [`client`].

pub mod accounts;
pub mod acp;
pub mod agent_accounts;
#[rustfmt::skip]
pub mod agent_catalog;
mod backoff;
pub mod cli;
pub mod client;
pub mod download;
pub mod files;
mod flights;
pub mod frozen;
pub mod git;
pub mod git_guard;
pub mod harness;
pub mod host_update;
pub mod http;
pub mod join;
pub mod launch;
pub use sikemux_wire::grid;
pub use sikemux_wire::protocol;
pub mod push;
pub mod release_feed;
pub mod releases;
pub mod runtime;
pub use sikemux_client::device;
pub use sikemux_client::remote;
pub mod pages;
pub mod server;
pub mod sessions;
pub mod skills;
pub mod tasks;
mod text;
pub mod trailers;
pub mod usage;
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
