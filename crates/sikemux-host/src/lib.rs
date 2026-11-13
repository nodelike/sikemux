//! The `sikemux` command: the core a host runs, and the CLI, `$EDITOR` and
//! agent tools beside it. It never links the app's window, so a host with no
//! screen installs only this.

pub mod account;
pub mod cli_client;
#[cfg(unix)]
pub mod headless;
pub mod plugins;
mod problems;
mod style;
#[cfg(unix)]
pub mod update;

pub use plugins::provide_to_core;
#[cfg(unix)]
pub use sikemux_hostkit::core_start;
pub use sikemux_hostkit::{
    cli_paths, crash_reports, environment, hang_reports, install_tls_crypto, login_item,
    panic_reports,
};

pub use sikemux_buildinfo::{identity as build_identity, COMMIT as BUILD_COMMIT};
