//! What the core shares with the accounts server at api.sikemux.com. The types
//! in [`protocol`], [`share`] and [`host`] are generated from `server/protocol/schema`.

pub mod host;
pub mod live;
pub mod share;
pub(crate) mod socket;

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

/// What a Mac signs to watch a shared chat as the host's account.
pub fn share_viewer_message(nonce: &str, key: &str) -> String {
    format!("sikemux-share-viewer|{nonce}|{key}")
}

/// One request a host makes to the accounts API as itself, signed in its
/// `Authorization: SikemuxHost <key>.<time>.<signature>` header.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRequest {
    pub method: String,
    /// The path and query exactly as sent, such as `/v1/devices?role=client`.
    pub path: String,
    /// Unix seconds.
    pub time: u64,
    /// The lowercase hex SHA-256 of the body; an empty body hashes the empty string.
    pub body_sha256: String,
}

const HOST_REQUEST_METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const MAX_HOST_REQUEST_PATH: usize = 2048;

impl HostRequest {
    pub fn message(&self) -> String {
        format!(
            "sikemux-host-request|{}|{}|{}|{}",
            self.method, self.path, self.time, self.body_sha256
        )
    }

    /// Accepts only an HTTP request to the API, so a request to sign one
    /// cannot make the core sign any other text.
    pub fn check(&self) -> Result<(), &'static str> {
        if !HOST_REQUEST_METHODS.contains(&self.method.as_str()) {
            return Err("the method is not one the accounts API takes");
        }
        let path_fits = self.path.starts_with("/v1/")
            && self.path.len() <= MAX_HOST_REQUEST_PATH
            && self.path.bytes().all(|b| b.is_ascii_graphic() && b != b'|');
        if !path_fits {
            return Err("the path is not an accounts API path");
        }
        let is_sha256 = self.body_sha256.len() == 64
            && self
                .body_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !is_sha256 {
            return Err("the body's hash is 64 lowercase hex characters");
        }
        Ok(())
    }
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

/// Why the accounts server turned a request down, in words for the person.
pub async fn refusal(response: reqwest::Response) -> String {
    let status = response.status();
    match response.json::<protocol::ApiError>().await {
        Ok(body) => format!("the account server refused: {}", body.error.message),
        Err(_) => format!("the account server answered {status}"),
    }
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
