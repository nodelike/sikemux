//! The background process that owns Sikemux terminals and chat agents, so
//! they outlive the window. `sikemux core` runs [`server`]; the app talks to
//! it with [`client`].

pub mod accounts;
pub mod acp;
#[cfg(unix)]
pub mod agent_accounts;
pub mod agent_catalog;
pub mod cli;
#[cfg(unix)]
pub mod client;
pub mod frozen;
#[cfg(unix)]
pub mod git_guard;
pub mod harness;
#[cfg(unix)]
pub mod join;
#[cfg(unix)]
pub mod launch;
pub use sikemux_wire::protocol;
#[cfg(unix)]
pub mod push;
#[cfg(unix)]
pub use sikemux_client::remote;
#[cfg(unix)]
pub mod server;
#[cfg(unix)]
pub mod sessions;
pub mod skills;
pub mod trailers;

use std::path::PathBuf;

pub const SOCKET_ENV: &str = "SIKEMUX_CORE_SOCKET";

/// `~/.config/sikemux/core.sock`, or `core.dev.sock` in debug builds, unless
/// `SIKEMUX_CORE_SOCKET` names another path.
pub fn default_socket_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SOCKET_ENV).filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    sikemux_process::paths::config_file("core", "sock")
}
