// Pushes the core makes to a Bitbucket repository with the person's own sign-in, which only
// the core holds. The plugin says yes to a repository only once Bitbucket has said one of the
// accounts signed in here can push to it, and keeps that answer in a file beside its accounts,
// so the core can ask without the network.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use sikemux_plugin_api::credential::{Acting, Ask, Credential, Reach};
use sikemux_plugin_api::http::Read;
use sikemux_plugin_api::PluginContext;

use crate::client::{self, Credential as Signed, Session, API};
use crate::config::{self, Account, Method as AuthMethod};
use crate::error::BitbucketResult;
use crate::repo;

const FILE: &str = "pushers.json";
/// How long a repository no account could push to waits before it is looked at again.
const RECHECK: Duration = Duration::from_secs(10 * 60);
/// What git is told a credential lasts. It is asked for each push, so it need only outlive one.
const LASTS: Duration = Duration::from_secs(60);
/// Scopes of a browser sign-in that let it push. Writing pull requests includes pushing.
const OAUTH_PUSH: [&str; 2] = ["repository:write", "pullrequest:write"];
/// The scope an API token or access token needs to push, as Bitbucket names it in its answers.
const TOKEN_PUSH: [&str; 3] = [
    "repository:write",
    "pullrequest:write",
    "write:repository:bitbucket",
];

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pushers {
    /// The account that pushes to each repository, by `workspace/repository` in lower case.
    #[serde(default)]
    repos: BTreeMap<String, String>,
    /// When each repository with no account that can push was last looked at.
    #[serde(default)]
    looked: BTreeMap<String, u64>,
}

static KNOWN: Mutex<Option<Pushers>> = Mutex::new(None);
static LOOKING: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

fn known<T>(data_dir: &Path, read: impl FnOnce(&mut Pushers) -> T) -> T {
    let mut kept = KNOWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read(kept.get_or_insert_with(|| {
        std::fs::read(path(data_dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }))
}

fn change(data_dir: &Path, edit: impl FnOnce(&mut Pushers)) {
    known(data_dir, |pushers| {
        edit(pushers);
        if let Ok(bytes) = serde_json::to_vec(pushers) {
            let _ = std::fs::write(path(data_dir), bytes);
        }
    });
}

/// Lets the next ask start a look, however the one it marks ends.
struct Looking;

impl Drop for Looking {
    fn drop(&mut self) {
        LOOKING.store(false, Ordering::Release);
    }
}

fn key_of(remote: &str) -> Option<String> {
    let found = repo::from_remote(remote).filter(|found| found.host == repo::HOST)?;
    Some(repo::slug(&found).to_ascii_lowercase())
}

/// Whether the core pushes to `remote` with the person's sign-in. A repository not looked at
/// lately starts a look, for the next ask.
pub fn acts_as(ctx: &PluginContext, remote: &str) -> Option<Acting> {
    let key = key_of(remote)?;
    let data_dir = ctx.data_dir();
    let (pushes, looked) = known(data_dir, |pushers| {
        let looked = pushers.looked.get(&key).copied().unwrap_or(0);
        (pushers.repos.contains_key(&key), looked)
    });
    if pushes {
        return Some(Acting::Person);
    }
    if now_ms().saturating_sub(looked) > RECHECK.as_millis() as u64
        && !LOOKING.swap(true, Ordering::AcqRel)
    {
        let data_dir = data_dir.to_path_buf();
        let looking = Looking;
        ctx.spawn(async move {
            let _looking = looking;
            let pusher = pusher(&data_dir, &key).await;
            change(&data_dir, |pushers| match pusher {
                Some(account) => {
                    pushers.looked.remove(&key);
                    pushers.repos.insert(key, account);
                }
                None => {
                    pushers.looked.insert(key, now_ms());
                }
            });
        });
    }
    None
}

/// The first account signed in here, the default one first, that Bitbucket says can push to
/// the repository `key` names.
async fn pusher(data_dir: &Path, key: &str) -> Option<String> {
    let config = config::load(data_dir).ok()?;
    let mut accounts: Vec<Account> = config.accounts.clone();
    accounts.sort_by_key(|account| Some(&account.id) != config.default.as_ref());
    for account in accounts {
        let id = account.id.clone();
        if can_push(account, key).await.unwrap_or(false) {
            return Some(id);
        }
    }
    None
}

async fn can_push(account: Account, key: &str) -> BitbucketResult<bool> {
    let session = Session::of(account).await?;
    let seen = API
        .send_as(
            &session,
            Method::GET,
            &format!("/repositories/{key}"),
            &[("fields", "full_name".to_owned())],
            None,
            Read::ALL,
        )
        .await?;
    if !seen.status.is_success() {
        return Ok(false);
    }
    let scopes = match session.account.method {
        AuthMethod::Oauth => session.scopes.clone(),
        AuthMethod::Token => (seen.headers.get("x-oauth-scopes"))
            .and_then(|scopes| scopes.to_str().ok())
            .map(|scopes| {
                (scopes.split([' ', ',']))
                    .filter(|scope| !scope.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    };
    let pushing = match session.account.method {
        AuthMethod::Oauth => &OAUTH_PUSH[..],
        AuthMethod::Token => &TOKEN_PUSH[..],
    };
    if !scopes.iter().any(|scope| pushing.contains(&scope.as_str())) {
        return Ok(false);
    }
    let access_token = session.account.method == AuthMethod::Token
        && matches!(session.credential, Signed::Bearer(_));
    if access_token {
        return Ok(true);
    }
    let permission: client::Page<Permission> = API
        .send_as(
            &session,
            Method::GET,
            "/user/permissions/repositories",
            &[("q", format!("repository.full_name=\"{key}\""))],
            None,
            Read::ALL,
        )
        .await?
        .succeeded()?
        .parsed()?;
    Ok(
        (permission.values.iter())
            .any(|held| matches!(held.permission.as_str(), "write" | "admin")),
    )
}

#[derive(Deserialize)]
struct Permission {
    permission: String,
}

/// The person's sign-in in the shape git takes over HTTPS, for the core's own push. Nothing for
/// an agent's command, which never gets the person's sign-in.
pub async fn credential(data_dir: &Path, ask: &Ask) -> BitbucketResult<Option<Credential>> {
    if ask.reach != Reach::Core {
        return Ok(None);
    }
    let Some(key) = key_of(&ask.remote) else {
        return Ok(None);
    };
    let Some(id) = known(data_dir, |pushers| pushers.repos.get(&key).cloned()) else {
        return Ok(None);
    };
    let config = config::load(data_dir)?;
    let Some(account) = config.accounts.iter().find(|account| account.id == id) else {
        change(data_dir, |pushers| {
            pushers.repos.remove(&key);
        });
        return Ok(None);
    };
    let session = Session::of(account.clone()).await?;
    let (username, secret) = match session.credential {
        Signed::Basic { token, .. } => ("x-bitbucket-api-token-auth", token),
        Signed::Bearer(token) => ("x-token-auth", token),
    };
    Ok(Some(Credential {
        username: username.to_owned(),
        secret,
        expires_at: now_ms() + LASTS.as_millis() as u64,
    }))
}

/// Forgets which repositories `account` pushes to, as it signs in or out, and has every
/// repository no account could push to looked at again.
pub fn forget(data_dir: &Path, account: &str) {
    change(data_dir, |pushers| {
        pushers.repos.retain(|_, pusher| pusher != account);
        pushers.looked.clear();
    });
}
