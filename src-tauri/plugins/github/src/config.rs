// The GitHub accounts signed in here, on github.com or a company's own
// GitHub. A token typed in is saved in the Keychain; one already in the
// environment, or held by the `gh` CLI, is used where it is and never copied.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{GithubError, GithubResult};

#[cfg(not(test))]
pub const TOKEN_SERVICE: &str = "sikemux-github-token";
/// Tests keep to an entry of their own, so they never replace or delete a real token.
#[cfg(test)]
pub const TOKEN_SERVICE: &str = "sikemux-github-token-test";
pub const DEFAULT_HOST: &str = "github.com";
const GH_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_LIMIT: usize = 64 * 1024;
/// Reading the Keychain means starting `security`, and asking the `gh` CLI
/// means starting that. Doing either on every request costs more than the
/// request. A token is held for long enough to serve a screenful of calls and
/// briefly enough that a `gh auth login` in a terminal is still noticed.
const TOKEN_TTL: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum TokenSource {
    Keychain,
    Environment,
    GhCli,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// `host:login`, which is also the Keychain entry a saved token is under.
    pub id: String,
    pub host: String,
    pub login: String,
    /// Where the token lives. Only that place is read, so an older token
    /// somewhere else never stands in for it.
    pub source: TokenSource,
    /// Whether Sikemux saved the token, and so may delete it on sign-out.
    #[serde(default)]
    pub owns_token: bool,
}

pub fn account_id(host: &str, login: &str) -> String {
    format!("{host}:{login}")
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GithubConfig {
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// The account a project uses when it names none.
    #[serde(default)]
    pub default: Option<String>,
    /// Set by signing the last account out, so a token the shell or `gh`
    /// still holds is not picked up again until somebody signs in.
    #[serde(default)]
    pub signed_out: bool,
}

impl GithubConfig {
    /// The account named, or with none named, the default one.
    pub fn account(&self, id: Option<&str>) -> Option<&Account> {
        match id.or(self.default.as_deref()) {
            Some(wanted) => self.accounts.iter().find(|account| account.id == wanted),
            None => self.accounts.first(),
        }
    }

    /// Adds the account, or replaces the one with the same id. The first account becomes the default.
    pub fn upsert(&mut self, account: Account) {
        match self.accounts.iter_mut().find(|kept| kept.id == account.id) {
            Some(kept) => *kept = account,
            None => self.accounts.push(account),
        }
        let default_is_signed_in = self
            .default
            .as_ref()
            .is_some_and(|id| self.accounts.iter().any(|account| &account.id == id));
        if !default_is_signed_in {
            self.default = self.accounts.first().map(|account| account.id.clone());
        }
        self.signed_out = false;
    }

    /// Takes the account out; a removed default passes to the first account left.
    pub fn remove(&mut self, id: &str) -> Option<Account> {
        let index = self.accounts.iter().position(|account| account.id == id)?;
        let removed = self.accounts.remove(index);
        if self.default.as_deref() == Some(id) {
            self.default = self.accounts.first().map(|account| account.id.clone());
        }
        self.signed_out = self.accounts.is_empty();
        Some(removed)
    }

    /// Every account, the default first, which is the order to try them in.
    pub fn in_order(&self) -> Vec<Account> {
        let mut ordered = self.accounts.clone();
        ordered.sort_by_key(|account| Some(&account.id) != self.default.as_ref());
        ordered
    }

    /// The GitHub a new sign-in starts on: the default account's, else
    /// whatever `GH_HOST` names, else github.com.
    pub fn host_hint(&self) -> String {
        host_for(
            self.account(None).map(|account| account.host.as_str()),
            sikemux_process::user_environment::var("GH_HOST").as_deref(),
        )
    }

    /// The host of the account named, or of the default one.
    pub fn host_of(&self, id: Option<&str>) -> String {
        self.account(id)
            .map_or_else(|| self.host_hint(), |account| account.host.clone())
    }

    /// Whether any account signed in here is on this host.
    pub fn serves(&self, host: &str) -> bool {
        if self.accounts.is_empty() {
            return self.host_hint() == host;
        }
        self.accounts.iter().any(|account| account.host == host)
    }
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

/// The host signed in to wins. `GH_HOST` only chooses one when nothing has
/// been signed in to yet.
fn host_for(saved: Option<&str>, gh_host: Option<&str>) -> String {
    saved
        .filter(|host| !host.is_empty())
        .map(str::to_string)
        .or_else(|| gh_host.and_then(|host| validate_host(host).ok()))
        .unwrap_or_else(|| DEFAULT_HOST.to_string())
}

pub fn load(data_dir: &Path) -> GithubConfig {
    std::fs::read(config_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn io_error(error: std::io::Error) -> GithubError {
    GithubError::Transport(format!("saving settings: {error}"))
}

pub fn save(data_dir: &Path, config: &GithubConfig) -> GithubResult<()> {
    std::fs::create_dir_all(data_dir).map_err(io_error)?;
    let path = config_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(config)?).map_err(io_error)?;
    std::fs::rename(&staged, &path).map_err(io_error)
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A bare hostname, with a port only when one was given. People paste whole
/// URLs, so a scheme, a user and a path are trimmed off rather than refused.
pub fn validate_host(raw: &str) -> GithubResult<String> {
    let bad = || GithubError::BadArg("that is not a GitHub hostname".into());
    let trimmed = raw.trim();
    let after_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    let authority = authority.split('@').next_back().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if let Some(port) = port {
        let numeric =
            !port.is_empty() && port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit());
        if !numeric {
            return Err(bad());
        }
    }
    if host.is_empty() || host.len() > 253 || !host.split('.').all(valid_label) {
        return Err(bad());
    }
    Ok(authority.to_ascii_lowercase())
}

/// github.com serves its API from a host of its own; every other GitHub serves
/// it from `/api/v3` on the host itself.
pub fn api_base(host: &str) -> String {
    if host == DEFAULT_HOST {
        "https://api.github.com".to_string()
    } else {
        format!("https://{host}/api/v3")
    }
}

fn run(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> GithubResult<std::process::Output> {
    sikemux_process::run(command, input, timeout, OUTPUT_LIMIT, None)
        .map_err(|error| GithubError::Transport(error.to_string()))
}

fn keychain_error(error: sikemux_keychain::KeychainError) -> GithubError {
    match error {
        sikemux_keychain::KeychainError::Invalid(message) => GithubError::BadArg(message),
        sikemux_keychain::KeychainError::Failed(message) => GithubError::Keychain(message),
    }
}

pub fn keychain_read(id: &str) -> GithubResult<Option<String>> {
    sikemux_keychain::read(TOKEN_SERVICE, id).map_err(keychain_error)
}

pub fn keychain_write(id: &str, secret: &str) -> GithubResult<()> {
    sikemux_keychain::write(TOKEN_SERVICE, id, secret).map_err(keychain_error)
}

pub fn keychain_delete(id: &str) -> GithubResult<()> {
    sikemux_keychain::delete(TOKEN_SERVICE, id).map_err(keychain_error)
}

/// The variables the `gh` CLI reads a token from. A github.com token is never
/// sent to a company GitHub, nor the other way round.
fn token_vars(host: &str) -> [&'static str; 2] {
    if host == DEFAULT_HOST {
        ["GH_TOKEN", "GITHUB_TOKEN"]
    } else {
        ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
    }
}

fn env_entry(host: &str) -> Option<(&'static str, String)> {
    token_vars(host).into_iter().find_map(|name| {
        let token = sikemux_process::user_environment::var(name)?
            .trim()
            .to_string();
        (!token.is_empty()).then_some((name, token))
    })
}

pub fn env_token(host: &str) -> Option<String> {
    env_entry(host).map(|(_, token)| token)
}

/// Which variable the environment's token is read from, for telling the person.
pub fn env_variable(host: &str) -> Option<&'static str> {
    env_entry(host).map(|(name, _)| name)
}

/// The token the `gh` CLI is already signed in with, so somebody who has run
/// `gh auth login` never types one here.
pub fn gh_cli_token(host: &str, login: Option<&str>) -> Option<String> {
    let mut command = sikemux_process::user_environment::command("gh");
    command.args(["auth", "token", "--hostname", host]);
    if let Some(login) = login.filter(|login| !login.is_empty()) {
        command.args(["--user", login]);
    }
    let output = run(&mut command, None, GH_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// A token already in the environment or held by `gh`, which can be used
/// without anybody typing one. Starts a process, so it belongs inside `blocking`.
pub fn offered_token(host: &str) -> Option<(String, TokenSource)> {
    env_token(host)
        .map(|token| (token, TokenSource::Environment))
        .or_else(|| gh_cli_token(host, None).map(|token| (token, TokenSource::GhCli)))
}

struct Cached {
    token: String,
    at: Instant,
}

static TOKENS: Mutex<BTreeMap<String, Cached>> = Mutex::new(BTreeMap::new());

fn cached_for(id: &str) -> Option<String> {
    let held = TOKENS.lock().ok()?;
    let cached = held.get(id)?;
    (cached.at.elapsed() < TOKEN_TTL).then(|| cached.token.clone())
}

/// Drops a held token, or all of them, so the next call goes and looks again.
/// Signing in or out changes which token is right, and a refused one is worth
/// re-reading in case the shell or the Keychain has a newer one.
pub fn forget_token(id: Option<&str>) {
    if let Ok(mut held) = TOKENS.lock() {
        match id {
            Some(id) => {
                held.remove(id);
            }
            None => held.clear(),
        }
    }
}

fn read_token(account: &Account) -> GithubResult<Option<String>> {
    Ok(match account.source {
        TokenSource::Keychain => keychain_read(&account.id)?,
        TokenSource::Environment => env_token(&account.host),
        TokenSource::GhCli => gh_cli_token(&account.host, Some(&account.login)),
    })
}

/// The account a call runs as, and its token. With nobody signed in yet, a
/// token the shell or `gh` holds stands in, under an account with no login.
/// Starts a process or waits on the Keychain, so it belongs inside `blocking`.
pub fn resolve(
    config: &GithubConfig,
    chosen: Option<&str>,
) -> GithubResult<Option<(Account, String)>> {
    let account = match config.account(chosen) {
        Some(account) => account.clone(),
        None if config.accounts.is_empty() && !config.signed_out && chosen.is_none() => {
            let host = config.host_hint();
            let Some((token, source)) = offered_token(&host) else {
                return Ok(None);
            };
            let account = Account {
                id: account_id(&host, ""),
                host,
                login: String::new(),
                source,
                owns_token: false,
            };
            return Ok(Some((account, token)));
        }
        None => return Ok(None),
    };
    if let Some(token) = cached_for(&account.id) {
        return Ok(Some((account, token)));
    }
    let Some(token) = read_token(&account)? else {
        return Ok(None);
    };
    if let Ok(mut held) = TOKENS.lock() {
        held.insert(
            account.id.clone(),
            Cached {
                token: token.clone(),
                at: Instant::now(),
            },
        );
    }
    Ok(Some((account, token)))
}

/// Runs work that starts a process or touches the Keychain on a thread meant
/// for blocking, away from the few threads every plugin shares.
pub async fn blocking<T: Send + 'static>(
    work: Box<dyn FnOnce() -> GithubResult<T> + Send>,
) -> GithubResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| GithubError::Keychain(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_hostname_out_of_whatever_was_pasted() -> GithubResult<()> {
        for raw in [
            "github.com",
            "https://github.com",
            "https://github.com/",
            "https://github.com/nodelike",
            "  GitHub.com  ",
            "git@github.com",
        ] {
            assert_eq!(validate_host(raw)?, "github.com", "{raw}");
        }
        assert_eq!(
            validate_host("git.example.com:8443")?,
            "git.example.com:8443"
        );
        Ok(())
    }

    #[test]
    fn turns_down_things_that_are_not_hostnames() {
        for raw in [
            "",
            "   ",
            "git hub.com",
            "https://",
            "/",
            "github.com:",
            "github.com:notaport",
            "-github.com",
            "git_hub.com",
        ] {
            assert!(validate_host(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn github_dot_com_has_its_own_api_host() {
        assert_eq!(api_base("github.com"), "https://api.github.com");
        assert_eq!(
            api_base("git.example.com"),
            "https://git.example.com/api/v3"
        );
    }

    fn account(host: &str, login: &str) -> Account {
        Account {
            id: account_id(host, login),
            host: host.into(),
            login: login.into(),
            source: TokenSource::Keychain,
            owns_token: true,
        }
    }

    #[test]
    fn round_trips_the_config_file() -> GithubResult<()> {
        let dir = std::env::temp_dir().join(format!("sikemux-gha-{}", std::process::id()));
        let mut config = GithubConfig::default();
        config.upsert(account("git.example.com", "octocat"));
        save(&dir, &config)?;
        assert_eq!(load(&dir), config);
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn the_first_account_is_the_default_and_each_can_be_named() {
        let mut config = GithubConfig::default();
        config.upsert(account("github.com", "work"));
        config.upsert(account("github.com", "home"));
        assert_eq!(config.default.as_deref(), Some("github.com:work"));
        assert_eq!(
            config
                .account(Some("github.com:home"))
                .map(|a| a.login.as_str()),
            Some("home")
        );
        assert!(config.account(Some("github.com:gone")).is_none());
        assert!(config.serves("github.com"));
        assert!(!config.serves("ghe.corp"));
    }

    #[test]
    fn signing_the_last_account_out_stops_borrowing_the_shells_token() {
        let mut config = GithubConfig::default();
        config.upsert(account("github.com", "work"));
        config.upsert(account("github.com", "home"));
        config.remove("github.com:work");
        assert_eq!(config.default.as_deref(), Some("github.com:home"));
        assert!(!config.signed_out);
        config.remove("github.com:home");
        assert!(config.signed_out);
        assert!(matches!(resolve(&config, None), Ok(None)));
    }

    #[test]
    fn an_account_that_is_not_signed_in_resolves_to_nothing() {
        let mut config = GithubConfig::default();
        config.upsert(account("github.com", "work"));
        assert!(matches!(
            resolve(&config, Some("github.com:other")),
            Ok(None)
        ));
    }

    #[test]
    fn a_held_token_is_kept_per_account_and_does_not_outlive_its_welcome() {
        let hold = |id: &str, at: Instant| {
            if let Ok(mut held) = TOKENS.lock() {
                held.insert(
                    id.into(),
                    Cached {
                        token: "ghp_held".into(),
                        at,
                    },
                );
            }
        };
        let id = "held.test:someone";
        forget_token(Some(id));
        assert_eq!(cached_for(id), None);
        hold(id, Instant::now());
        assert_eq!(cached_for(id).as_deref(), Some("ghp_held"));
        assert_eq!(cached_for("held.test:other"), None);
        hold(id, Instant::now() - TOKEN_TTL - Duration::from_secs(1));
        assert_eq!(cached_for(id), None, "held too long");
        hold(id, Instant::now());
        forget_token(Some(id));
        assert_eq!(cached_for(id), None);
    }

    #[test]
    fn the_host_signed_in_to_wins_over_gh_host() {
        assert_eq!(host_for(Some("github.com"), Some("ghe.corp")), "github.com");
        assert_eq!(host_for(None, Some("ghe.corp")), "ghe.corp");
        assert_eq!(host_for(Some(""), Some("ghe.corp")), "ghe.corp");
        assert_eq!(host_for(None, Some("not a host")), DEFAULT_HOST);
        assert_eq!(host_for(None, None), DEFAULT_HOST);
    }

    #[test]
    fn a_github_dot_com_token_only_goes_to_github_dot_com() {
        assert_eq!(token_vars("github.com"), ["GH_TOKEN", "GITHUB_TOKEN"]);
        for host in ["ghe.corp", "ghe.corp:8443", "git.example.com"] {
            assert_eq!(
                token_vars(host),
                ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"],
                "{host}"
            );
        }
    }
}
