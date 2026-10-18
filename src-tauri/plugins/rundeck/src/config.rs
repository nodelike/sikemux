// The Rundeck this app is signed in to. The API token lives in the Keychain;
// the file beside it keeps the address, who signed in, and which job options
// carry the branch.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::error::{RundeckError, RundeckResult};

/// A development build keeps an entry of its own, so it never spends or replaces
/// the installed app's sign-in.
#[cfg(not(test))]
const TOKEN_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-rundeck-token-dev"
} else {
    "sikemux-rundeck-token"
};
/// Tests keep to an entry of their own, so they never replace or delete a real token.
#[cfg(test)]
const TOKEN_SERVICE: &str = "sikemux-rundeck-token-test";
const TOKEN_ACCOUNT: &str = "api-token";

const DEFAULT_BRANCH_OPTIONS: [&str; 4] = ["BRANCH", "GIT_BRANCH", "GIT_REF", "REF"];

fn default_branch_options() -> Vec<String> {
    DEFAULT_BRANCH_OPTIONS.map(String::from).to_vec()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub url: String,
    pub user: String,
    /// Id of the token this app minted, so signing out can revoke it. Empty for a pasted token.
    pub token_id: String,
    /// Explicit user acknowledgement for plaintext HTTP. Even when enabled,
    /// every request must resolve exclusively to private or loopback addresses.
    pub allow_insecure_private_http: bool,
    /// Job options that hold the git branch, tried in order, in any case.
    pub branch_options: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            url: String::new(),
            user: String::new(),
            token_id: String::new(),
            allow_insecure_private_http: false,
            branch_options: default_branch_options(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Saved {
    pub settings: Settings,
    pub token: Option<String>,
}

impl Saved {
    pub fn is_signed_in(&self) -> bool {
        !self.settings.url.is_empty() && self.token.is_some()
    }
}

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

fn read_settings(data_dir: &Path) -> RundeckResult<Settings> {
    match std::fs::read(settings_path(data_dir)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(error.into()),
    }
}

fn write_settings(data_dir: &Path, settings: &Settings) -> RundeckResult<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = settings_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(&staged, &path)?;
    Ok(())
}

fn keychain_error(error: sikemux_keychain::KeychainError) -> RundeckError {
    match error {
        sikemux_keychain::KeychainError::Invalid(message) => RundeckError::Api(message),
        sikemux_keychain::KeychainError::Failed(message) => {
            RundeckError::Api(format!("the Keychain would not answer: {message}"))
        }
    }
}

/// Runs file and Keychain work on a thread meant for blocking, away from the few threads every plugin shares.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> RundeckResult<T> + Send + 'static,
) -> RundeckResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| RundeckError::Api(error.to_string()))?
}

/// Reading the Keychain starts a process, so the sign-in is kept in memory once read.
fn cache() -> &'static Mutex<Option<(PathBuf, Saved)>> {
    static CACHE: OnceLock<Mutex<Option<(PathBuf, Saved)>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn remember(data_dir: &Path, saved: &Saved) {
    *cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some((data_dir.to_path_buf(), saved.clone()));
}

pub async fn load(data_dir: &Path) -> RundeckResult<Saved> {
    let cached = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .filter(|(dir, _)| dir == data_dir)
        .map(|(_, saved)| saved.clone());
    if let Some(saved) = cached {
        return Ok(saved);
    }
    let dir = data_dir.to_path_buf();
    let saved = blocking(move || {
        Ok(Saved {
            settings: read_settings(&dir)?,
            token: sikemux_keychain::read(TOKEN_SERVICE, TOKEN_ACCOUNT).map_err(keychain_error)?,
        })
    })
    .await?;
    remember(data_dir, &saved);
    Ok(saved)
}

/// Saves the settings and puts the token in the Keychain, or takes it out when there is none.
pub async fn save(data_dir: &Path, saved: Saved) -> RundeckResult<()> {
    let dir = data_dir.to_path_buf();
    let kept = saved.clone();
    blocking(move || {
        match &kept.token {
            Some(token) => sikemux_keychain::write(TOKEN_SERVICE, TOKEN_ACCOUNT, token),
            None => sikemux_keychain::delete(TOKEN_SERVICE, TOKEN_ACCOUNT),
        }
        .map_err(keychain_error)?;
        write_settings(&dir, &kept.settings)
    })
    .await?;
    remember(data_dir, &saved);
    Ok(())
}

/// Saves the settings alone, leaving the token where it is.
pub async fn save_settings(data_dir: &Path, settings: Settings) -> RundeckResult<()> {
    let mut saved = load(data_dir).await?;
    let dir = data_dir.to_path_buf();
    let written = settings.clone();
    blocking(move || write_settings(&dir, &written)).await?;
    saved.settings = settings;
    remember(data_dir, &saved);
    Ok(())
}

#[cfg(test)]
pub fn validate_base_url(raw: &str) -> RundeckResult<()> {
    validate_base_url_with_policy(raw, false)
}

pub fn validate_base_url_with_policy(
    raw: &str,
    allow_insecure_private_http: bool,
) -> RundeckResult<()> {
    let url = url::Url::parse(raw).map_err(|_| RundeckError::BadArg("invalid Rundeck URL"))?;
    if url.username() != "" || url.password().is_some() {
        return Err(RundeckError::BadArg(
            "credentials in the Rundeck URL are not allowed",
        ));
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(RundeckError::BadArg("Rundeck URL must use HTTP or HTTPS"));
    }
    if url.host_str().is_none() {
        return Err(RundeckError::BadArg("Rundeck URL must include a host"));
    }
    if url.scheme() == "http" && !allow_insecure_private_http {
        return Err(RundeckError::BadArg(
            "plaintext HTTP requires explicit private-network acknowledgement",
        ));
    }
    Ok(())
}

fn is_allowed_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        IpAddr::V6(ip) => {
            ip.to_ipv4_mapped()
                .is_some_and(|mapped| is_allowed_private_ip(mapped.into()))
                || ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
        }
    }
}

#[derive(Clone, Debug)]
pub struct ValidatedTransport {
    host: String,
    private_http_addresses: Vec<std::net::SocketAddr>,
}

impl ValidatedTransport {
    pub fn pins_private_dns(&self) -> bool {
        !self.private_http_addresses.is_empty()
    }

    /// Identifies exactly what a client built from this transport is pinned to.
    pub fn pin_key(&self) -> String {
        let mut addresses: Vec<String> = self
            .private_http_addresses
            .iter()
            .map(std::net::SocketAddr::to_string)
            .collect();
        addresses.sort();
        format!("{}|{}", self.host, addresses.join(","))
    }

    pub fn pin_dns(&self, builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        if self.private_http_addresses.is_empty() {
            builder
        } else {
            builder.resolve_to_addrs(&self.host, &self.private_http_addresses)
        }
    }
}

/// Validate transport policy immediately before sending credentials or a
/// bearer token. HTTP is supported for private Rundeck installations only
/// after explicit acknowledgement and only while DNS remains private.
pub async fn validate_transport(
    raw: &str,
    allow_insecure_private_http: bool,
) -> RundeckResult<ValidatedTransport> {
    validate_base_url_with_policy(raw, allow_insecure_private_http)?;
    let url = url::Url::parse(raw).map_err(|_| RundeckError::BadArg("invalid Rundeck URL"))?;
    let host = url
        .host_str()
        .ok_or(RundeckError::BadArg("Rundeck URL must include a host"))?
        .to_string();
    if url.scheme() == "https" {
        return Ok(ValidatedTransport {
            host,
            private_http_addresses: Vec::new(),
        });
    }
    let port = url.port_or_known_default().ok_or(RundeckError::BadArg(
        "Rundeck URL must include a valid port",
    ))?;
    let addresses: Vec<_> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| {
            RundeckError::Transport(format!(
                "can't reach Rundeck: {host} does not resolve; check the network or VPN"
            ))
        })?
        .collect();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !is_allowed_private_ip(address.ip()))
    {
        return Err(RundeckError::BadArg(
            "plaintext Rundeck HTTP is allowed only when every resolved address is private or loopback",
        ));
    }
    Ok(ValidatedTransport {
        host,
        private_http_addresses: addresses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_schemes_credentials_and_plaintext_policy() {
        assert!(validate_base_url("https://rundeck.example.com").is_ok());
        assert!(validate_base_url("http://localhost:4440").is_err());
        assert!(validate_base_url("http://rundeck.example.com").is_err());
        assert!(validate_base_url_with_policy("http://localhost:4440", true).is_ok());
        assert!(validate_base_url("ftp://rundeck.example.com").is_err());
        assert!(validate_base_url("https://user:pass@rundeck.example.com").is_err());
    }
}
