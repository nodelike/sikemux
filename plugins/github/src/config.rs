// The GitHub accounts signed in here, on github.com or a company's own
// GitHub. A token typed in is saved in the Keychain; one already in the
// environment, or held by the `gh` CLI, is used where it is and never copied.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::process;
use sikemux_plugin_api::store::{self, Accounts, Held, Secrets};
use sikemux_plugin_api::{accounts, keychain_service};

use crate::error::{GithubError, GithubResult};

/// Each token typed in, under its account's id.
pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-github-token"));
pub const DEFAULT_HOST: &str = "github.com";
const GH_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_LIMIT: usize = 64 * 1024;

pub use sikemux_plugin_api::codehost::TokenSource;

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

impl store::Account for Account {
    fn id(&self) -> &str {
        &self.id
    }
}

accounts!(GithubConfig, Account, accounts, default);

impl GithubConfig {
    /// Adds the account, or replaces the one with the same id, and undoes
    /// signing the last account out.
    pub fn sign_in(&mut self, account: Account) {
        self.upsert(account);
        self.signed_out = false;
    }

    /// Takes the account out. Once none are left, a token the shell or `gh`
    /// holds is no longer used in their place.
    pub fn sign_out(&mut self, id: &str) -> Option<Account> {
        let removed = self.remove(id)?;
        self.signed_out = self.accounts.is_empty();
        Some(removed)
    }

    /// The GitHub a new sign-in starts on: the default account's, else
    /// whatever `GH_HOST` names, else github.com.
    pub fn host_hint(&self) -> String {
        host_for(
            self.account(None).map(|account| account.host.as_str()),
            process::user_environment::var("GH_HOST").as_deref(),
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

/// The host signed in to wins. `GH_HOST` only chooses one when nothing has
/// been signed in to yet.
fn host_for(saved: Option<&str>, gh_host: Option<&str>) -> String {
    saved
        .filter(|host| !host.is_empty())
        .map(str::to_string)
        .or_else(|| gh_host.and_then(|host| validate_host(host).ok()))
        .unwrap_or_else(|| DEFAULT_HOST.to_string())
}

pub fn load(data_dir: &Path) -> GithubResult<GithubConfig> {
    Ok(store::load(data_dir)?)
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
    process::run(command, input, timeout, OUTPUT_LIMIT, None)
        .map_err(|error| GithubError::Transport(error.to_string()))
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
        let token = process::user_environment::var(name)?.trim().to_string();
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
    let mut command = process::user_environment::command("gh");
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

/// Reading the Keychain means starting `security`, and asking the `gh` CLI
/// means starting that. A token is held for long enough to serve a screenful
/// of calls and briefly enough that a `gh auth login` in a terminal is still noticed.
static TOKENS: Held<String> = Held::secrets();

/// Drops a held token, or all of them, so the next call goes and looks again.
/// Signing in or out changes which token is right, and a refused one is worth
/// re-reading in case the shell or the Keychain has a newer one.
pub fn forget_token(id: Option<&str>) {
    TOKENS.forget(id);
}

fn read_token(account: &Account) -> GithubResult<Option<String>> {
    Ok(match account.source {
        TokenSource::Keychain => KEYCHAIN.read(&account.id)?,
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
    if let Some(token) = TOKENS.get(&account.id) {
        return Ok(Some((account, token)));
    }
    let Some(token) = read_token(&account)? else {
        return Ok(None);
    };
    TOKENS.put(&account.id, token.clone());
    Ok(Some((account, token)))
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
    fn only_the_hosts_signed_in_to_are_served() {
        let mut config = GithubConfig::default();
        config.sign_in(account("github.com", "work"));
        assert!(config.serves("github.com"));
        assert!(!config.serves("ghe.corp"));
    }

    #[test]
    fn signing_the_last_account_out_stops_borrowing_the_shells_token() {
        let mut config = GithubConfig::default();
        config.sign_in(account("github.com", "work"));
        config.sign_in(account("github.com", "home"));
        config.sign_out("github.com:work");
        assert!(!config.signed_out);
        config.sign_out("github.com:home");
        assert!(config.signed_out);
        assert!(matches!(resolve(&config, None), Ok(None)));
        config.sign_in(account("github.com", "home"));
        assert!(!config.signed_out);
    }

    #[test]
    fn an_account_that_is_not_signed_in_resolves_to_nothing() {
        let mut config = GithubConfig::default();
        config.sign_in(account("github.com", "work"));
        assert!(matches!(
            resolve(&config, Some("github.com:other")),
            Ok(None)
        ));
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
