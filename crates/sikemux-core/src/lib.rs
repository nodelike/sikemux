//! The background process that owns Sikemux terminals and chat agents, so
//! they outlive the window. `sikemux core` runs [`server`]; the app talks to
//! it with [`client`].

pub use sikemux_corekit::accounts;
pub mod acp;
#[cfg(unix)]
pub mod agent_accounts;
#[rustfmt::skip]
pub mod agent_catalog;
mod agent_ops;
#[cfg(unix)]
mod agent_shell;
use sikemux_corekit::{android_sdk, backoff, flights, text};
pub use sikemux_corekit::{
    build, cli, frozen, fuzzy, host_update, release_feed, releases, tasks, trailers,
};
#[cfg(unix)]
pub use sikemux_corekit::{client, join, push, timeout, waiting};
#[cfg(unix)]
pub use sikemux_git::files;
#[cfg(unix)]
pub mod git;
#[cfg(unix)]
pub mod git_guard;
pub mod harness;
#[cfg(unix)]
pub mod launch;
pub use sikemux_base::{download, http, pages, runtime};
#[cfg(unix)]
pub mod server;
#[cfg(unix)]
pub mod sessions;
mod ship;
pub mod skills;

pub use sikemux_corekit::{default_socket_path, SOCKET_ENV};
