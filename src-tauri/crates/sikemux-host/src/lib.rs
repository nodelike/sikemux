//! The `sikemux` command: the core a host runs, and the CLI, `$EDITOR` and
//! agent tools beside it. It never links the app's window, so a host with no
//! screen installs only this.

pub mod account;
pub mod cli_client;
pub mod cli_paths;
pub mod core_start;
pub mod environment;
pub mod hang_reports;
pub mod headless;
pub mod login_item;
pub mod panic_reports;
pub mod plugins;
mod style;
#[cfg(unix)]
pub mod update;

pub use plugins::provide_to_core;

/// The build of this binary and of the app built beside it, which share this
/// crate's build script output.
pub fn build_identity() -> sikemux_core::protocol::BuildIdentity {
    sikemux_core::protocol::BuildIdentity::new(
        env!("CARGO_PKG_VERSION"),
        env!("SIKEMUX_BUILD_COMMIT"),
        env!("SIKEMUX_BUILD_TIME").parse().unwrap_or(0),
        env!("SIKEMUX_BUILD_SOURCE"),
    )
}

// reqwest is built without a TLS crypto backend of its own, so every HTTP
// client in the app, the core and their plugins uses the one installed here.
pub fn install_tls_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
