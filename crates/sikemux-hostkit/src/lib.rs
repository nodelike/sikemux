//! What an app and the `sikemux` command both do around the core without linking it: starting
//! it and keeping it on their build, the login item, the CLI's paths, and the crash, panic and
//! hang reports.

pub mod cli_paths;
#[cfg(unix)]
pub mod core_start;
pub mod crash_reports;
pub mod environment;
pub mod hang_reports;
pub mod login_item;
pub mod opened;
pub mod panic_reports;

// reqwest is built without a TLS crypto backend of its own, so every HTTP
// client in the app, the core and their plugins uses the one installed here.
pub fn install_tls_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
