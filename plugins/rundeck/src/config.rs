// The Rundeck this app is signed in to. The API token lives in the Keychain;
// the file beside it keeps the address, who signed in, which job options carry
// the branch, and which folders are production.

use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::keychain_service;
use sikemux_plugin_api::process::sync::Locked;
use sikemux_plugin_api::store::{self, Secrets};

use crate::error::{RundeckError, RundeckResult};

const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-rundeck-token"));
const TOKEN_ACCOUNT: &str = "api-token";

const DEFAULT_BRANCH_OPTIONS: [&str; 4] = ["BRANCH", "GIT_BRANCH", "GIT_REF", "REF"];
const DEFAULT_PROD_ENVS: [&str; 4] = ["prod", "production", "prd", "live"];

pub fn default_branch_options() -> Vec<String> {
    DEFAULT_BRANCH_OPTIONS.map(String::from).to_vec()
}

pub fn default_prod_envs() -> Vec<String> {
    DEFAULT_PROD_ENVS.map(String::from).to_vec()
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
    /// Words that mark a job's top folder or project as production.
    pub prod_envs: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            url: String::new(),
            user: String::new(),
            token_id: String::new(),
            allow_insecure_private_http: false,
            branch_options: default_branch_options(),
            prod_envs: default_prod_envs(),
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

/// Reading the Keychain starts a process, so the sign-in is kept in memory once read.
static SAVED: Mutex<Option<Saved>> = Mutex::new(None);

pub async fn load(data_dir: &Path) -> RundeckResult<Saved> {
    if let Some(saved) = SAVED.locked().clone() {
        return Ok(saved);
    }
    let dir = data_dir.to_path_buf();
    let saved = store::blocking(move || {
        Ok(Saved {
            settings: store::load(&dir)?,
            token: KEYCHAIN.read(TOKEN_ACCOUNT)?,
        })
    })
    .await?;
    *SAVED.locked() = Some(saved.clone());
    Ok(saved)
}

/// Saves the settings and puts the token in the Keychain, or takes it out when there is none.
pub async fn save(data_dir: &Path, saved: Saved) -> RundeckResult<()> {
    let dir = data_dir.to_path_buf();
    let kept = saved.clone();
    store::blocking(move || {
        match &kept.token {
            Some(token) => KEYCHAIN.write(TOKEN_ACCOUNT, token),
            None => KEYCHAIN.delete(TOKEN_ACCOUNT),
        }?;
        Ok(store::save(&dir, &kept.settings)?)
    })
    .await?;
    *SAVED.locked() = Some(saved);
    Ok(())
}

/// Saves the settings alone, leaving the token where it is.
pub async fn save_settings(data_dir: &Path, settings: Settings) -> RundeckResult<()> {
    let mut saved = load(data_dir).await?;
    let dir = data_dir.to_path_buf();
    let written = settings.clone();
    store::blocking(move || Ok(store::save(&dir, &written)?)).await?;
    saved.settings = settings;
    *SAVED.locked() = Some(saved);
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
    let url =
        url::Url::parse(raw).map_err(|_| RundeckError::BadArg("invalid Rundeck URL".into()))?;
    if url.username() != "" || url.password().is_some() {
        return Err(RundeckError::BadArg(
            "credentials in the Rundeck URL are not allowed".into(),
        ));
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(RundeckError::BadArg(
            "Rundeck URL must use HTTP or HTTPS".into(),
        ));
    }
    if url.host_str().is_none() {
        return Err(RundeckError::BadArg(
            "Rundeck URL must include a host".into(),
        ));
    }
    if url.scheme() == "http" && !allow_insecure_private_http {
        return Err(RundeckError::BadArg(
            "plaintext HTTP requires explicit private-network acknowledgement".into(),
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
    let url =
        url::Url::parse(raw).map_err(|_| RundeckError::BadArg("invalid Rundeck URL".into()))?;
    let host = url
        .host_str()
        .ok_or(RundeckError::BadArg(
            "Rundeck URL must include a host".into(),
        ))?
        .to_string();
    if url.scheme() == "https" {
        return Ok(ValidatedTransport {
            host,
            private_http_addresses: Vec::new(),
        });
    }
    let port = url.port_or_known_default().ok_or(RundeckError::BadArg(
        "Rundeck URL must include a valid port".into(),
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
        return Err(RundeckError::BadArg("plaintext Rundeck HTTP is allowed only when every resolved address is private or loopback".into()));
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
