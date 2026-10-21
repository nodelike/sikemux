// Where SigNoz lives, how this machine signs in to it, and the Keychain
// entries that hold the secret. An API key sits under the same Keychain
// service the `signoz` shell CLI reads, so either can use a key the other
// saved. A signed-in session keeps only its refresh token there.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::{keychain_service, store};

use crate::error::{SignozError, SignozResult};

/// The signoz CLI's own entry, which every build shares with it.
pub const API_KEY_SERVICE: &str = "signoz-api";
pub const SESSION_SERVICE: &str = keychain_service!("sikemux-signoz-session");

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum AuthMode {
    #[default]
    Session,
    ApiKey,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SignozConfig {
    pub url: String,
    #[serde(default)]
    pub auth: AuthMode,
    /// The Keychain account the secret is saved under.
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub email: String,
    /// Whether Sikemux saved the API key, and so may delete it on sign-out.
    #[serde(default)]
    pub owns_key: bool,
}

pub fn load(data_dir: &Path) -> SignozResult<SignozConfig> {
    let config: SignozConfig = store::load(data_dir)?;
    Ok(match sikemux_process::user_environment::var("SIGNOZ_URL") {
        Some(url) if !url.trim().is_empty() => with_url_override(config, &url),
        _ => config,
    })
}

/// A saved credential belongs to the address it was saved for, so pointing
/// SigNoz somewhere else leaves it behind.
fn with_url_override(config: SignozConfig, raw: &str) -> SignozConfig {
    let url = validate_url(raw).unwrap_or_default();
    if url == config.url {
        return config;
    }
    SignozConfig {
        url,
        account: String::new(),
        owns_key: false,
        ..config
    }
}

pub fn validate_url(raw: &str) -> SignozResult<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = url::Url::parse(trimmed)
        .map_err(|_| SignozError::BadArg("the SigNoz URL is not a URL".into()))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SignozError::BadArg(
            "leave credentials out of the URL".into(),
        ));
    }
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        _ => {
            return Err(SignozError::BadArg(
                "SigNoz must be reached over HTTPS, or HTTP on this machine".into(),
            ))
        }
    }
    Ok(trimmed.to_string())
}

/// Keychain entries are named after the host, so two SigNoz instances never
/// share one and nobody has to choose a name.
pub fn account_for(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "signoz".into())
}

pub fn validate_account(raw: &str) -> SignozResult<String> {
    let account = raw.trim();
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    if account.is_empty() || account.len() > 128 || !account.chars().all(allowed) {
        return Err(SignozError::BadArg(
            "the Keychain account is letters, digits, dots, dashes and underscores".into(),
        ));
    }
    Ok(account.to_string())
}

pub fn keychain_read(service: &str, account: &str) -> SignozResult<Option<String>> {
    Ok(sikemux_keychain::read(service, account)?)
}

pub fn keychain_write(service: &str, account: &str, secret: &str) -> SignozResult<()> {
    let account = validate_account(account)?;
    Ok(sikemux_keychain::write(service, &account, secret)?)
}

pub fn keychain_delete(service: &str, account: &str) -> SignozResult<()> {
    Ok(sikemux_keychain::delete(service, account)?)
}

pub fn env_api_key() -> Option<String> {
    sikemux_process::user_environment::var("SIGNOZ_API_KEY")
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_and_local_http_only() {
        assert_eq!(
            validate_url("https://logs.example.com/").unwrap(),
            "https://logs.example.com"
        );
        assert!(validate_url("http://localhost:8080").is_ok());
        assert!(validate_url("http://logs.example.com").is_err());
        assert!(validate_url("https://user:key@logs.example.com").is_err());
        assert!(validate_url("logs.example.com").is_err());
    }

    #[test]
    fn names_keychain_entries_after_the_host() {
        assert_eq!(account_for("https://logs.example.com"), "logs.example.com");
        assert_eq!(account_for("http://localhost:3301"), "localhost");
    }

    #[test]
    fn keeps_keychain_arguments_to_safe_characters() {
        assert!(validate_account("work").is_ok());
        assert!(validate_account("a b").is_err());
    }

    #[test]
    fn keeps_a_saved_credential_to_its_own_address() {
        let saved = SignozConfig {
            url: "https://logs.example.com".into(),
            auth: AuthMode::Session,
            account: "logs.example.com".into(),
            email: "me@example.com".into(),
            owns_key: true,
        };
        assert_eq!(
            with_url_override(saved.clone(), "https://logs.example.com/"),
            saved
        );
        let moved = with_url_override(saved.clone(), "https://other.example.com");
        assert_eq!(moved.url, "https://other.example.com");
        assert_eq!((moved.account.as_str(), moved.owns_key), ("", false));
        let refused = with_url_override(saved, "http://other.example.com");
        assert_eq!((refused.url.as_str(), refused.account.as_str()), ("", ""));
    }
}
