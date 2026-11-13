//! What the core is built on that knows nothing of a running core: its accounts client, the
//! protocol its command line speaks, tasks, releases and the small helpers they share.

pub mod accounts;
pub mod android_sdk;
pub mod backoff;
pub mod build;
pub mod cli;
#[cfg(unix)]
pub mod client;
pub mod flights;
pub mod frozen;
pub mod fuzzy;
pub mod host_update;
#[cfg(unix)]
pub mod join;
pub mod lines;
#[cfg(unix)]
pub mod push;
pub mod release_feed;
pub mod releases;
pub mod tasks;
pub mod text;
pub mod themes;
#[cfg(unix)]
pub mod timeout;
pub mod trailers;
#[cfg(unix)]
pub mod waiting;
#[cfg(unix)]
pub mod wall_clock;

use sikemux_base::http;

use std::path::PathBuf;

pub const SOCKET_ENV: &str = sikemux_process::vars::CORE_SOCKET;

/// `~/.config/sikemux/core.sock`, or `core.dev.sock` in debug builds, unless
/// `SIKEMUX_CORE_SOCKET` names another path.
pub fn default_socket_path() -> Option<PathBuf> {
    if let Some(path) = sikemux_process::paths::env_path(SOCKET_ENV) {
        return Some(path);
    }
    sikemux_process::paths::config_file("core", "sock")
}
