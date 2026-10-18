//! What the core shares with the accounts server at api.sikemux.com. The types
//! in [`protocol`], [`share`] and [`host`] are generated from `server/protocol/schema`.

pub mod host;
#[cfg(unix)]
pub mod live;
pub mod share;

#[cfg(unix)]
pub use sikemux_client::accounts::{
    check_live, check_registration, check_user_id, live_message, network, registration_message,
    tls_config,
};
pub use sikemux_wire::accounts::protocol;

/// Where the web app is. A shared chat's link opens there, and it hands the
/// link on to the Mac or phone app when one is installed.
pub fn app_base() -> String {
    if cfg!(debug_assertions) {
        std::env::var("SIKEMUX_APP_URL").unwrap_or_else(|_| "http://localhost:5173".into())
    } else {
        "https://app.sikemux.com".into()
    }
}

/// What a host signs to open its share connection. The prefix keeps it from
/// ever passing for a live connection's hello.
pub fn share_message(nonce: &str, key: &str) -> String {
    format!("sikemux-share|{nonce}|{key}")
}

/// Where the accounts API is. Dev builds talk to a server on this computer,
/// or to `SIKEMUX_API_URL`.
pub fn api_base() -> String {
    if cfg!(debug_assertions) {
        std::env::var("SIKEMUX_API_URL").unwrap_or_else(|_| "http://127.0.0.1:4000".into())
    } else {
        "https://api.sikemux.com".into()
    }
}

#[cfg(test)]
type RoundTrip = serde_json::Result<serde_json::Value>;

#[cfg(test)]
fn through<T: serde::de::DeserializeOwned + serde::Serialize>(json: &str) -> RoundTrip {
    serde_json::to_value(serde_json::from_str::<T>(json)?)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every example in `server/protocol/fixtures-core` reads as its Rust type
    /// and writes back unchanged, as sikemux-wire checks the rest.
    #[test]
    fn core_fixtures_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../server/protocol/fixtures-core");
        let mut checked = 0;
        for kind in std::fs::read_dir(&fixtures)? {
            let kind = kind?;
            let name = kind.file_name().to_string_lossy().into_owned();
            for example in std::fs::read_dir(kind.path())? {
                let path = example?.path();
                let json = std::fs::read_to_string(&path)?;
                let written = super::share::round_trip(&name, &json)
                    .or_else(|| super::host::round_trip(&name, &json))
                    .ok_or_else(|| format!("{name} has fixtures but no Rust type"))?
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let read: serde_json::Value = serde_json::from_str(&json)?;
                assert_eq!(
                    written,
                    read,
                    "{} changed on the way through Rust",
                    path.display()
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "no fixtures under {}", fixtures.display());
        Ok(())
    }
}
