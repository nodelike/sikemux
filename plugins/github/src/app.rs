// The GitHub App that is the agents' own identity. Its private key stays in the Keychain and
// travels to linked hosts through the account vault; each host that has it mints its own
// short-lived tokens, one repository at a time.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use reqwest::{Method, RequestBuilder};
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::asks::Merge;
use sikemux_plugin_api::credential::{Actor, Ask, Credential, Reach};
use sikemux_plugin_api::http::Read;
use sikemux_plugin_api::keychain_service;
use sikemux_plugin_api::store::{self, Held, Secrets};
use sikemux_plugin_api::vault::{self, Book, Listed, Listing, SignIn};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::client::{self, Session, API_VERSION, HTTP, JSON};
use crate::config;
use crate::error::{Github, GithubError, GithubResult};
use crate::repo::{self, Repo};

/// Each App's private key, as GitHub handed it out, under `host:app id`.
pub const KEYS: Secrets = Secrets::new(keychain_service!("sikemux-github-app-key"));
const APPS_FILE: &str = "apps.json";
const INSTALLATIONS_FILE: &str = "app-installations.json";
const USERNAME: &str = "x-access-token";
const IDENTITY_PREFIX: &str = "app:";
/// A held token is handed out until this close to the hour GitHub gives it.
const RENEW_BEFORE_MS: u64 = 5 * 60 * 1000;
/// How long a repository an App was not on is taken as still without it.
const RECHECK: Duration = Duration::from_secs(10 * 60);
const MAX_PAGES: u32 = 10;

/// What every token may do besides contents, which depends on who it is for.
const OTHER_PERMISSIONS: [(&str, &str); 6] = [
    ("pull_requests", "write"),
    ("issues", "write"),
    ("actions", "write"),
    ("checks", "read"),
    ("statuses", "read"),
    ("metadata", "read"),
];

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub id: u64,
    pub slug: String,
    pub client_id: String,
    pub host: String,
    pub owner: String,
    pub organization: bool,
    pub bot_id: u64,
    pub bot_login: String,
}

impl App {
    pub fn key_account(&self) -> String {
        format!("{}:{}", self.host, self.id)
    }

    pub fn actor(&self) -> Actor {
        Actor {
            name: self.bot_login.clone(),
            email: format!(
                "{}+{}@users.noreply.{}",
                self.bot_id, self.bot_login, self.host
            ),
        }
    }
}

impl Listing for App {
    fn secret_account(&self) -> Option<String> {
        Some(self.key_account())
    }

    fn label(&self) -> String {
        format!("{} on {}", self.slug, self.host)
    }

    fn identity(&self) -> Option<String> {
        Some(format!("{IDENTITY_PREFIX}{}", self.key_account()))
    }

    fn starts_kept(&self) -> bool {
        self.organization
    }
}

static APP_BOOK: Listed<App> = Listed::new(APPS_FILE, "apps", "id", None, KEYS);

/// The person's own sign-ins and each App, for the account vault.
pub struct Books;

pub static BOOKS: Books = Books;

impl Book for Books {
    fn sign_ins(&self, data_dir: &Path) -> Result<Vec<SignIn>, String> {
        let mut all = config::BOOK.sign_ins(data_dir)?;
        all.extend(APP_BOOK.sign_ins(data_dir)?);
        Ok(all)
    }

    fn take_in(&self, data_dir: &Path, sign_in: &SignIn) -> Result<(), String> {
        if !sign_in.identity.starts_with(IDENTITY_PREFIX) {
            return config::BOOK.take_in(data_dir, sign_in);
        }
        APP_BOOK.take_in(data_dir, sign_in)?;
        forget_known();
        Ok(())
    }

    fn let_go(&self, data_dir: &Path, identity: &str) -> Result<(), String> {
        if !identity.starts_with(IDENTITY_PREFIX) {
            return config::BOOK.let_go(data_dir, identity);
        }
        APP_BOOK.let_go(data_dir, identity)?;
        forget_known();
        Ok(())
    }

    fn renewer(&self) -> Option<&'static dyn vault::Renew> {
        config::BOOK.renewer()
    }
}

/// The repositories one App is installed on, as `owner/name` in lower case, with the
/// installation that covers each.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Installations {
    #[serde(default)]
    pub checked_at: u64,
    #[serde(default)]
    pub repos: BTreeMap<String, u64>,
}

#[derive(Deserialize, Default)]
struct AppsFile {
    #[serde(default)]
    apps: Vec<App>,
}

/// Each App's installations, by its `host:app id`.
type AllInstallations = BTreeMap<String, Installations>;

#[derive(Clone, Default)]
struct Known {
    apps: Vec<App>,
    installations: AllInstallations,
}

/// What `acts_as` answers from, read from disk once and kept in step with every write here.
static KNOWN: Mutex<Option<Known>> = Mutex::new(None);
static WRITING: Mutex<()> = Mutex::new(());
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// Tokens kept for reuse, by `owner/name` and reach: an agent's own, and the merge's, with the
/// App each came from.
static MINTED: Mutex<BTreeMap<String, (App, Credential)>> = Mutex::new(BTreeMap::new());
/// Repositories GitHub said an App is not on, by `host:app id owner/name`, so a git command
/// there does not ask again.
static NOT_INSTALLED: Held<()> = Held::new(RECHECK);

fn forget_known() {
    *KNOWN.locked() = None;
    MINTED.locked().clear();
    NOT_INSTALLED.forget(None);
}

fn known(data_dir: &Path) -> Known {
    let mut kept = KNOWN.locked();
    kept.get_or_insert_with(|| Known {
        apps: read_apps(data_dir),
        installations: store::read_file(&data_dir.join(INSTALLATIONS_FILE)).unwrap_or_default(),
    })
    .clone()
}

fn read_apps(data_dir: &Path) -> Vec<App> {
    store::read_file::<AppsFile>(&data_dir.join(APPS_FILE))
        .map(|file| file.apps)
        .unwrap_or_default()
}

fn repo_key(repo: &Repo) -> String {
    format!("{}/{}", repo.owner, repo.name).to_ascii_lowercase()
}

fn not_installed_key(app: &App, repo_key: &str) -> String {
    format!("{} {repo_key}", app.key_account())
}

fn save_installations(data_dir: &Path, all: AllInstallations) -> GithubResult<()> {
    store::write_file(&data_dir.join(INSTALLATIONS_FILE), &all)?;
    if let Some(known) = KNOWN.locked().as_mut() {
        known.installations = all;
    }
    Ok(())
}

fn change_installations(
    data_dir: &Path,
    app: &App,
    change: impl FnOnce(&mut Installations),
) -> GithubResult<()> {
    let _turn = WRITING.locked();
    let mut all = known(data_dir).installations;
    change(all.entry(app.key_account()).or_default());
    save_installations(data_dir, all)
}

fn installed(data_dir: &Path, app: &App, key: &str, installation: Option<u64>) -> GithubResult<()> {
    change_installations(data_dir, app, |installations| {
        match installation {
            Some(id) => installations.repos.insert(key.to_owned(), id),
            None => installations.repos.remove(key),
        };
    })
}

pub fn apps(data_dir: &Path) -> Vec<App> {
    known(data_dir).apps
}

pub fn installations(data_dir: &Path, app: &App) -> Installations {
    (known(data_dir).installations)
        .remove(&app.key_account())
        .unwrap_or_default()
}

/// The Apps on `repo`'s GitHub, the ones its owner owns first.
fn candidates(known: &Known, repo: &Repo) -> Vec<App> {
    let mut apps: Vec<App> = (known.apps.iter())
        .filter(|app| app.host == repo.host)
        .cloned()
        .collect();
    apps.sort_by_key(|app| !app.owner.eq_ignore_ascii_case(&repo.owner));
    apps
}

/// The App known to be installed on `repo`, with its installation there.
fn installed_on(known: &Known, repo: &Repo) -> Option<(App, u64)> {
    let key = repo_key(repo);
    candidates(known, repo).into_iter().find_map(|app| {
        let on = known
            .installations
            .get(&app.key_account())?
            .repos
            .get(&key)
            .copied()?;
        Some((app, on))
    })
}

/// Who commits to `remote` carry as committer, from what is known here. A miss on a GitHub
/// with an App starts a look at every App's installations there, for the next ask.
pub fn acts_as(ctx: &sikemux_plugin_api::PluginContext, remote: &str) -> Option<Actor> {
    let data_dir = ctx.data_dir();
    let repo = repo::from_remote(remote)?;
    let known = known(data_dir);
    if let Some((app, _)) = installed_on(&known, &repo) {
        return Some(app.actor());
    }
    let stale = candidates(&known, &repo).iter().any(|app| {
        let checked_at = (known.installations.get(&app.key_account()))
            .map_or(0, |installations| installations.checked_at);
        epoch_ms().saturating_sub(checked_at) > RECHECK.as_millis() as u64
    });
    if stale && !REFRESHING.swap(true, Ordering::AcqRel) {
        let data_dir = data_dir.to_path_buf();
        ctx.spawn(async move {
            for app in apps(&data_dir) {
                let _ = refresh(&data_dir, &app).await;
            }
            REFRESHING.store(false, Ordering::Release);
        });
    }
    None
}

/// A token for one command. Only an agent's own token is held here; the core revokes the others
/// once the push they were made for is done.
pub async fn credential(data_dir: &Path, ask: &Ask) -> GithubResult<Option<Credential>> {
    let keep = ask.reach == Reach::Agent;
    let minted = token(data_dir, &ask.remote, ask.reach, keep).await?;
    Ok(minted.map(|(_, credential)| credential))
}

/// Merges a pull request as the App, with a token that never leaves the plugin. False when the
/// App is not on that repository.
pub async fn merge(data_dir: &Path, remote: &str, merge: Merge) -> GithubResult<bool> {
    let asked = format!("{}/{}", merge.pull.repo.owner, merge.pull.repo.name).to_ascii_lowercase();
    let Some(repo) = repo::from_remote(remote).filter(|repo| repo_key(repo) == asked) else {
        return Err(GithubError::BadArg(
            "that pull request is not in the repository named".into(),
        ));
    };
    let Some((app, held)) = token(data_dir, remote, Reach::Core, true).await? else {
        return Ok(false);
    };
    let session = Session::of_app(&repo.host, &app.bot_login, held.secret);
    client::as_app(session, crate::pulls::merge(data_dir, merge)).await?;
    Ok(true)
}

fn permissions(reach: Reach) -> Vec<(&'static str, &'static str)> {
    let contents = match reach {
        Reach::Agent => "read",
        Reach::Core | Reach::CoreWorkflows => "write",
    };
    let mut permissions = vec![("contents", contents)];
    permissions.extend(OTHER_PERMISSIONS);
    if reach == Reach::CoreWorkflows {
        permissions.push(("workflows", "write"));
    }
    permissions
}

/// The App to use on `repo` with its installation there, asking GitHub about each App whose
/// installations here do not name it.
async fn pick(data_dir: &Path, repo: &Repo) -> GithubResult<Option<(App, String, u64)>> {
    let known = known(data_dir);
    if let Some((app, installation)) = installed_on(&known, repo) {
        let jwt = jwt(&app).await?;
        return Ok(Some((app, jwt, installation)));
    }
    let key = repo_key(repo);
    for app in candidates(&known, repo) {
        let asked = not_installed_key(&app, &key);
        if NOT_INSTALLED.get(&asked).is_some() {
            continue;
        }
        let jwt = jwt(&app).await?;
        match installation_of(&app, &jwt, repo).await? {
            Some(installation) => {
                installed(data_dir, &app, &key, Some(installation))?;
                return Ok(Some((app, jwt, installation)));
            }
            None => NOT_INSTALLED.put(&asked, ()),
        }
    }
    Ok(None)
}

async fn token(
    data_dir: &Path,
    remote: &str,
    reach: Reach,
    keep: bool,
) -> GithubResult<Option<(App, Credential)>> {
    let Some(repo) = repo::from_remote(remote) else {
        return Ok(None);
    };
    let key = repo_key(&repo);
    let held_as = format!("{key} {reach:?}");
    if keep {
        let held = MINTED.locked().get(&held_as).cloned();
        if let Some((app, held)) =
            held.filter(|(_, held)| held.expires_at > epoch_ms() + RENEW_BEFORE_MS)
        {
            KEYS.used(&app.key_account());
            return Ok(Some((app, held)));
        }
    }
    let Some((app, jwt, installation)) = pick(data_dir, &repo).await? else {
        return Ok(None);
    };
    let minted = match mint(
        &app,
        &jwt,
        installation,
        Some(&repo.name),
        &permissions(reach),
    )
    .await
    {
        Err(GithubError::NotFound(_)) => {
            installed(data_dir, &app, &key, None)?;
            NOT_INSTALLED.put(&not_installed_key(&app, &key), ());
            return Ok(None);
        }
        minted => minted?,
    };
    let credential = Credential {
        username: USERNAME.to_owned(),
        secret: minted.token,
        expires_at: minted.expires_at,
    };
    if keep {
        MINTED
            .locked()
            .insert(held_as, (app.clone(), credential.clone()));
    }
    Ok(Some((app, credential)))
}

pub async fn revoke(remote: &str, secret: &str) -> GithubResult<()> {
    let Some(repo) = repo::from_remote(remote) else {
        return Ok(());
    };
    MINTED.locked().retain(|_, (_, held)| held.secret != secret);
    end_token(&repo.host, secret).await
}

/// Learns every repository `app` is installed on, so installs made on GitHub since are noticed
/// without a restart.
pub async fn refresh(data_dir: &Path, app: &App) -> GithubResult<Installations> {
    let jwt = jwt(app).await?;
    let mut repos = BTreeMap::new();
    for installation in installation_ids(app, &jwt).await? {
        let token = mint(app, &jwt, installation, None, &[("metadata", "read")]).await?;
        let listed = repositories(app, &token.token).await;
        end_token(&app.host, &token.token).await.ok();
        for name in listed? {
            repos.insert(name.to_ascii_lowercase(), installation);
        }
    }
    let installations = Installations {
        checked_at: epoch_ms(),
        repos,
    };
    let kept = installations.clone();
    change_installations(data_dir, app, |installations| *installations = kept)?;
    NOT_INSTALLED.forget(None);
    Ok(installations)
}

fn write_apps(data_dir: &Path, apps: &[App]) -> GithubResult<()> {
    store::write_file(&data_dir.join(APPS_FILE), &json!({ "apps": apps }))?;
    Ok(())
}

/// Adds a new App and its key here, beside the others.
pub fn remember(data_dir: &Path, app: &App, pem: &str) -> GithubResult<()> {
    let _turn = WRITING.locked();
    KEYS.write(&app.key_account(), pem)?;
    let mut apps = read_apps(data_dir);
    apps.retain(|kept| kept.key_account() != app.key_account());
    apps.push(app.clone());
    write_apps(data_dir, &apps)?;
    let mut all: AllInstallations =
        store::read_file(&data_dir.join(INSTALLATIONS_FILE)).unwrap_or_default();
    all.remove(&app.key_account());
    store::write_file(&data_dir.join(INSTALLATIONS_FILE), &all)?;
    forget_known();
    Ok(())
}

/// Forgets the App under `key_account` on this host. It stays on GitHub until the person deletes
/// it there.
pub fn forget(data_dir: &Path, key_account: &str) -> GithubResult<()> {
    let _turn = WRITING.locked();
    let mut apps = read_apps(data_dir);
    let Some(at) = apps.iter().position(|app| app.key_account() == key_account) else {
        return Ok(());
    };
    apps.remove(at);
    KEYS.delete(key_account)?;
    write_apps(data_dir, &apps)?;
    let mut all: AllInstallations =
        store::read_file(&data_dir.join(INSTALLATIONS_FILE)).unwrap_or_default();
    all.remove(key_account);
    store::write_file(&data_dir.join(INSTALLATIONS_FILE), &all)?;
    forget_known();
    Ok(())
}

fn request(host: &str, method: Method, path: &str, bearer: &str) -> GithubResult<RequestBuilder> {
    Ok(HTTP
        .client()?
        .request(method, format!("{}{path}", config::api_base(host)))
        .bearer_auth(bearer)
        .header("Accept", JSON)
        .header("X-GitHub-Api-Version", API_VERSION))
}

async fn ask<T: serde::de::DeserializeOwned>(request: RequestBuilder) -> GithubResult<T> {
    HTTP.ask(request, Read::ALL)
        .await?
        .succeeded::<Github>()?
        .parsed::<T, Github>()
}

/// Signed with the App's key, which is read through the vault so each use is logged.
async fn jwt(app: &App) -> GithubResult<String> {
    let account = app.key_account();
    let pem = store::blocking(move || Ok(KEYS.read(&account)?))
        .await?
        .ok_or(GithubError::Unconfigured)?;
    sign_jwt(&app.client_id, &pem, epoch_ms() / 1000)
}

pub fn sign_jwt(issuer: &str, pem: &str, now_secs: u64) -> GithubResult<String> {
    let unreadable = || GithubError::Auth("the App's private key could not be read".into());
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = STANDARD.decode(body).map_err(|_| unreadable())?;
    let key = RsaKeyPair::from_der(&der)
        .or_else(|_| RsaKeyPair::from_pkcs8(&der))
        .map_err(|_| unreadable())?;
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = json!({ "iat": now_secs - 60, "exp": now_secs + 9 * 60, "iss": issuer });
    let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
    let message = format!("{header}.{claims}");
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        message.as_bytes(),
        &mut signature,
    )
    .map_err(|_| unreadable())?;
    Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

#[derive(Deserialize)]
struct IdRow {
    id: u64,
}

async fn installation_of(app: &App, jwt: &str, repo: &Repo) -> GithubResult<Option<u64>> {
    let path = format!("/repos/{}/{}/installation", repo.owner, repo.name);
    match ask::<IdRow>(request(&app.host, Method::GET, &path, jwt)?).await {
        Ok(row) => Ok(Some(row.id)),
        Err(GithubError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn installation_ids(app: &App, jwt: &str) -> GithubResult<Vec<u64>> {
    let mut ids = Vec::new();
    for page in 1..=MAX_PAGES {
        let path = format!("/app/installations?per_page=100&page={page}");
        let rows: Vec<IdRow> = ask(request(&app.host, Method::GET, &path, jwt)?).await?;
        let full = rows.len() == 100;
        ids.extend(rows.into_iter().map(|row| row.id));
        if !full {
            break;
        }
    }
    Ok(ids)
}

#[derive(Deserialize)]
struct RepoPage {
    repositories: Vec<NamedRow>,
}

#[derive(Deserialize)]
struct NamedRow {
    full_name: String,
}

async fn repositories(app: &App, token: &str) -> GithubResult<Vec<String>> {
    let mut names = Vec::new();
    for page in 1..=MAX_PAGES {
        let path = format!("/installation/repositories?per_page=100&page={page}");
        let rows: RepoPage = ask(request(&app.host, Method::GET, &path, token)?).await?;
        let full = rows.repositories.len() == 100;
        names.extend(rows.repositories.into_iter().map(|row| row.full_name));
        if !full {
            break;
        }
    }
    Ok(names)
}

struct Minted {
    token: String,
    expires_at: u64,
}

#[derive(Deserialize)]
struct TokenRow {
    token: String,
    expires_at: String,
}

async fn mint(
    app: &App,
    jwt: &str,
    installation: u64,
    repository: Option<&str>,
    permissions: &[(&str, &str)],
) -> GithubResult<Minted> {
    let permissions: serde_json::Map<String, Value> = permissions
        .iter()
        .map(|(name, level)| ((*name).to_owned(), Value::from(*level)))
        .collect();
    let body = match repository {
        Some(repository) => json!({ "permissions": permissions, "repositories": [repository] }),
        None => json!({ "permissions": permissions }),
    };
    let path = format!("/app/installations/{installation}/access_tokens");
    let row: TokenRow = ask(request(&app.host, Method::POST, &path, jwt)?.json(&body)).await?;
    let expires_at = time::OffsetDateTime::parse(
        &row.expires_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map(|at| (at.unix_timestamp_nanos() / 1_000_000) as u64)
    .map_err(|_| GithubError::Response("GitHub gave a token with no readable expiry".into()))?;
    Ok(Minted {
        token: row.token,
        expires_at,
    })
}

async fn end_token(host: &str, token: &str) -> GithubResult<()> {
    HTTP.ask(
        request(host, Method::DELETE, "/installation/token", token)?,
        Read::ALL,
    )
    .await?
    .succeeded::<Github>()?;
    Ok(())
}
