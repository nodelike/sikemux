//! What the core shares with the accounts server at api.sikemux.com. The types
//! in [`protocol`] are generated from `server/protocol/schema`.

#[cfg(unix)]
pub mod live;

#[cfg(unix)]
pub use sikemux_client::accounts::{
    check_live, check_registration, check_user_id, live_message, network, registration_message,
    tls_config,
};
pub use sikemux_wire::accounts::protocol;

/// Where the accounts API is. Dev builds talk to a server on this computer,
/// or to `SIKEMUX_API_URL`.
pub fn api_base() -> String {
    if cfg!(debug_assertions) {
        std::env::var("SIKEMUX_API_URL").unwrap_or_else(|_| "http://127.0.0.1:4000".into())
    } else {
        "https://api.sikemux.com".into()
    }
}
