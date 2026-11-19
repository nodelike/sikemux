//! What the core is built on that knows nothing of a running core: its accounts client, the
//! protocol its command line speaks, tasks, releases and the small helpers they share.

pub mod accounts;
pub mod android_sdk;
pub mod backoff;
pub mod build;
pub mod cli;
pub mod client;
pub mod flights;
pub mod folder;
pub mod frozen;
pub mod fuzzy;
pub mod host_update;
pub mod join;
pub mod lines;
pub mod lock;
pub mod peer;
pub mod push;
pub mod release_feed;
pub mod releases;
pub mod tasks;
pub mod text;
pub mod themes;
pub mod timeout;
pub mod trailers;
pub mod usage;
pub mod waiting;
pub mod wall_clock;

use sikemux_base::http;

use std::path::PathBuf;

pub const SOCKET_ENV: &str = sikemux_process::vars::CORE_SOCKET;

/// `~/.config/sikemux/core.sock`, or `core.dev.sock` in debug builds, unless
/// `SIKEMUX_CORE_SOCKET` names another path. On Windows it names the core's pipe, and the
/// folder is `%LOCALAPPDATA%\Sikemux` unless `SIKEMUX_CONFIG_DIR` names one.
pub fn default_socket_path() -> Option<PathBuf> {
    use sikemux_process::paths;

    if let Some(path) = paths::env_path(SOCKET_ENV) {
        return Some(path);
    }
    let in_config = paths::config_file("core", "sock")?;
    if cfg!(windows) && paths::env_path(sikemux_process::vars::CONFIG_DIR).is_none() {
        let local = paths::env_path("LOCALAPPDATA")?.join("Sikemux");
        return Some(local.join(in_config.file_name()?));
    }
    Some(in_config)
}
