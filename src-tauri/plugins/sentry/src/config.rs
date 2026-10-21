// The Sentry this app is signed in to. The auth token lives in the Keychain; the
// file beside it keeps the address, the organisation and project in use, and who signed in.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{SentryError, SentryResult};

/// A development build keeps an entry of its own, so it never spends or replaces
/// the installed app's sign-in.
#[cfg(not(test))]
const TOKEN_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-sentry-token-dev"
} else {
    "sikemux-sentry-token"
};
/// Tests keep to an entry of their own, so they never replace or delete a real token.
#[cfg(test)]
const TOKEN_SERVICE: &str = "sikemux-sentry-token-test";
const TOKEN_ACCOUNT: &str = "auth-token";

pub const SENTRY_IO: &str = "https://sentry.io";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// sentry.io, a regional host such as de.sentry.io, or a self-hosted Sentry.
    pub url: String,
    /// The organisation's slug.
    pub org: String,
    pub org_name: String,
    /// The project shown first, by slug; none for every project.
    pub project: Option<String>,
    pub user_id: String,
    pub user: String,
}

impl Settings {
    pub fn is_signed_in(&self) -> bool {
        !self.url.is_empty() && !self.org.is_empty()
    }
}

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

pub fn load(data_dir: &Path) -> Settings {
    std::fs::read(settings_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn io_error(error: std::io::Error) -> SentryError {
    SentryError::Transport(format!("saving settings: {error}"))
}

pub fn save(data_dir: &Path, settings: &Settings) -> SentryResult<()> {
    std::fs::create_dir_all(data_dir).map_err(io_error)?;
    let path = settings_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(settings)?).map_err(io_error)?;
    std::fs::rename(&staged, &path).map_err(io_error)
}

pub fn forget(data_dir: &Path) -> SentryResult<()> {
    match std::fs::remove_file(settings_path(data_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn keychain_error(error: sikemux_keychain::KeychainError) -> SentryError {
    match error {
        sikemux_keychain::KeychainError::Invalid(message) => SentryError::BadArg(message),
        sikemux_keychain::KeychainError::Failed(message) => SentryError::Keychain(message),
    }
}

pub fn token_read() -> SentryResult<Option<String>> {
    sikemux_keychain::read(TOKEN_SERVICE, TOKEN_ACCOUNT).map_err(keychain_error)
}

pub fn token_write(token: &str) -> SentryResult<()> {
    sikemux_keychain::write(TOKEN_SERVICE, TOKEN_ACCOUNT, token).map_err(keychain_error)
}

pub fn token_delete() -> SentryResult<()> {
    sikemux_keychain::delete(TOKEN_SERVICE, TOKEN_ACCOUNT).map_err(keychain_error)
}

/// Runs work that touches the Keychain or the disk on a thread meant for blocking.
pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> SentryResult<T> + Send + 'static,
) -> SentryResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| SentryError::Keychain(error.to_string()))?
}

/// The address a person typed, as the base every API path joins. Blank means sentry.io.
pub fn base_url(raw: &str) -> SentryResult<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(SENTRY_IO.into());
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let url = url::Url::parse(&with_scheme)
        .map_err(|_| SentryError::BadArg("that is not a Sentry address".into()))?;
    let host = url
        .host_str()
        .ok_or_else(|| SentryError::BadArg("the Sentry address needs a host".into()))?;
    if url.username() != "" || url.password().is_some() {
        return Err(SentryError::BadArg(
            "leave the sign-in out of the Sentry address".into(),
        ));
    }
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        _ => {
            return Err(SentryError::BadArg(
                "Sentry must be reached over HTTPS, or HTTP on this Mac".into(),
            ))
        }
    }
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Ok(format!("{}://{host}{port}", url.scheme()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_read_as_its_scheme_and_host() {
        assert_eq!(base_url("").ok().as_deref(), Some(SENTRY_IO));
        assert_eq!(
            base_url("de.sentry.io").ok().as_deref(),
            Some("https://de.sentry.io")
        );
        assert_eq!(
            base_url("https://sentry.acme.dev/organizations/acme/issues/")
                .ok()
                .as_deref(),
            Some("https://sentry.acme.dev")
        );
        assert_eq!(
            base_url("http://localhost:9000").ok().as_deref(),
            Some("http://localhost:9000")
        );
        assert!(base_url("http://sentry.acme.dev").is_err());
        assert!(base_url("https://me:secret@sentry.acme.dev").is_err());
    }

    #[test]
    fn settings_survive_a_save_and_nothing_saved_reads_as_signed_out() {
        let dir =
            std::env::temp_dir().join(format!("sikemux-sentry-config-{}", std::process::id()));
        assert!(!load(&dir).is_signed_in());
        let settings = Settings {
            url: SENTRY_IO.into(),
            org: "acme".into(),
            org_name: "Acme".into(),
            project: Some("storefront".into()),
            user_id: "42".into(),
            user: "Ankit".into(),
        };
        save(&dir, &settings).expect("saves");
        assert_eq!(load(&dir), settings);
        forget(&dir).expect("forgets");
        assert!(!load(&dir).is_signed_in());
        std::fs::remove_dir_all(dir).ok();
    }
}
