// Where SigNoz lives, how this machine signs in to it, and the Keychain
// entries that hold the secret. An API key sits under the same Keychain
// service the `signoz` shell CLI reads, so either can use a key the other
// saved. A signed-in session keeps only its refresh token there.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{SignozError, SignozResult};

pub const API_KEY_SERVICE: &str = "signoz-api";
pub const SESSION_SERVICE: &str = "sikemux-signoz-session";
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(10);
const KEYCHAIN_OUTPUT_LIMIT: usize = 64 * 1024;

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

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

pub fn load(data_dir: &Path) -> SignozConfig {
    let mut config: SignozConfig = std::fs::read(config_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if let Ok(url) = std::env::var("SIGNOZ_URL") {
        if !url.trim().is_empty() {
            config.url = url.trim().trim_end_matches('/').to_string();
        }
    }
    config
}

fn io_error(error: std::io::Error) -> SignozError {
    SignozError::Transport(format!("saving settings: {error}"))
}

pub fn save(data_dir: &Path, config: &SignozConfig) -> SignozResult<()> {
    std::fs::create_dir_all(data_dir).map_err(io_error)?;
    let path = config_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(config)?).map_err(io_error)?;
    std::fs::rename(&staged, &path).map_err(io_error)
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

/// Secrets reach `security -i` on a command line it splits on spaces, so
/// anything that could end the value early is refused rather than escaped.
fn validate_secret(raw: &str) -> SignozResult<String> {
    let secret = raw.trim();
    let allowed =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-' | '.' | '~');
    if secret.is_empty() || secret.len() > 4096 || !secret.chars().all(allowed) {
        return Err(SignozError::BadArg(
            "that does not look like a SigNoz key or token".into(),
        ));
    }
    Ok(secret.to_string())
}

fn run_security(args: &[&str], input: Option<&[u8]>) -> SignozResult<std::process::Output> {
    let mut command = Command::new("security");
    command.args(args);
    sikemux_process::run(
        &mut command,
        input,
        KEYCHAIN_TIMEOUT,
        KEYCHAIN_OUTPUT_LIMIT,
        None,
    )
    .map_err(|error| SignozError::Keychain(error.to_string()))
}

pub fn keychain_read(service: &str, account: &str) -> SignozResult<Option<String>> {
    let output = run_security(
        &["find-generic-password", "-s", service, "-a", account, "-w"],
        None,
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!secret.is_empty()).then_some(secret))
}

/// `security -i` reads the command from stdin, so the secret never shows up
/// in the process list the way an argument would.
pub fn keychain_write(service: &str, account: &str, secret: &str) -> SignozResult<()> {
    let account = validate_account(account)?;
    let secret = validate_secret(secret)?;
    let line = format!("add-generic-password -U -s {service} -a {account} -w {secret}\n");
    let output = run_security(&["-i"], Some(line.as_bytes()))?;
    if !output.status.success() {
        return Err(SignozError::Keychain(
            "the Keychain refused to save it".into(),
        ));
    }
    Ok(())
}

pub fn keychain_delete(service: &str, account: &str) -> SignozResult<()> {
    run_security(
        &["delete-generic-password", "-s", service, "-a", account],
        None,
    )?;
    Ok(())
}

pub fn env_api_key() -> Option<String> {
    std::env::var("SIGNOZ_API_KEY")
        .ok()
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
        assert!(validate_secret("eyJhbGciOi.J9-_~+/=").is_ok());
        assert!(validate_secret("abc def").is_err());
        assert!(validate_secret("abc\n-a other").is_err());
        assert!(validate_secret("a\"b").is_err());
    }

    #[test]
    fn round_trips_the_config_file() {
        let dir = std::env::temp_dir().join(format!("sikemux-signoz-{}", std::process::id()));
        let config = SignozConfig {
            url: "https://logs.example.com".into(),
            auth: AuthMode::ApiKey,
            account: "work".into(),
            email: String::new(),
            owns_key: false,
        };
        save(&dir, &config).unwrap();
        assert_eq!(load(&dir).auth, AuthMode::ApiKey);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(load(&dir).url, "");
    }
}
