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

/// The commit this binary and the app built beside it were built from.
pub const BUILD_COMMIT: &str = env!("SIKEMUX_BUILD_COMMIT");

/// The build of this binary and of the app built beside it, which share this
/// crate's build script output.
pub fn build_identity() -> sikemux_wire::protocol::BuildIdentity {
    sikemux_core::build::identity(
        env!("CARGO_PKG_VERSION"),
        BUILD_COMMIT,
        env!("SIKEMUX_BUILD_TIME").parse().unwrap_or(0),
        env!("SIKEMUX_BUILD_SOURCE"),
    )
}
