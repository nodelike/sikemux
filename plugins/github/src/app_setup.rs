// Making the agents' App from Sikemux with GitHub's manifest flow: a page on this machine posts
// the App's description to GitHub, the person confirms there, and GitHub sends the browser back
// with a code the core trades for the App and its private key.

use std::path::Path;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sikemux_plugin_api::http::Read;
use sikemux_plugin_api::oauth::{ended, new_state, Callback};
use sikemux_plugin_api::store::Accounts;
use sikemux_plugin_api::StreamSink;

use crate::app::{self, App};
use crate::client::{API_VERSION, HTTP, JSON};
use crate::config;
use crate::error::{Github, GithubError, GithubResult};

const CALLBACK: &str = "/github/app/callback";
const START: &str = "/github/app/new";
/// The manifest's code lasts an hour on GitHub's side.
const BROWSER_WAIT: Duration = Duration::from_secs(30 * 60);
const MAX_NAME: usize = 34;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Create {
    /// The organization that owns the App; the person's own account when none.
    #[serde(default)]
    pub organization: Option<String>,
}

#[derive(Deserialize)]
pub struct Forget {
    pub app: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    /// What `forgetApp` names it by.
    pub id: String,
    pub app: String,
    pub owner: String,
    pub organization: bool,
    pub repositories: usize,
    pub install_url: String,
    pub problem: Option<String>,
}

#[derive(Serialize)]
pub struct Apps {
    pub apps: Vec<AppStatus>,
}

pub async fn status(data_dir: &Path) -> Apps {
    let apps = app::apps(data_dir);
    let looked =
        futures::future::join_all(apps.iter().map(|app| app::refresh(data_dir, app))).await;
    let apps = apps
        .into_iter()
        .zip(looked)
        .map(|(app, looked)| {
            let (repositories, problem) = match looked {
                Ok(installations) => (installations.repos.len(), None),
                Err(error) => (
                    app::installations(data_dir, &app).repos.len(),
                    Some(sikemux_plugin_api::PluginError::from(error).message),
                ),
            };
            AppStatus {
                id: app.key_account(),
                install_url: install_url(&app),
                organization: app.organization,
                app: app.slug,
                owner: app.owner,
                repositories,
                problem,
            }
        })
        .collect();
    Apps { apps }
}

fn install_url(app: &App) -> String {
    format!("https://{}/apps/{}/installations/new", app.host, app.slug)
}

/// Runs the whole flow, sending each page to open down `sink`: the local page that posts the
/// manifest, then the page where the person picks repositories.
pub async fn create(data_dir: &Path, ask: Create, sink: &StreamSink) -> GithubResult<()> {
    if ask
        .organization
        .as_deref()
        .is_some_and(|org| !plain_name(org))
    {
        return Err(GithubError::BadArg(
            "that is not an organization name".into(),
        ));
    }
    let config = config::load(data_dir).unwrap_or_default();
    let host = config.host_of(None);
    let login = config.account(None).map(|account| account.login.clone());
    let state = new_state().map_err(|error| GithubError::Auth(error.0))?;
    let callback = Callback::bind(CALLBACK, "GitHub")
        .await
        .map_err(|error| GithubError::Auth(error.0))?;
    let target = match &ask.organization {
        Some(org) => format!("https://{host}/organizations/{org}/settings/apps/new"),
        None => format!("https://{host}/settings/apps/new"),
    };
    let manifest = manifest(
        &proposed_name(ask.organization.as_deref().or(login.as_deref())),
        callback.redirect_uri(),
    );
    let page = posting_page(&format!("{target}?state={state}"), &manifest.to_string());
    let callback = callback.serving(START, page);
    sink.send(json!({ "url": callback.address_of(START) }))
        .map_err(|_| GithubError::Auth("making the App was closed".into()))?;
    let landed = tokio::time::timeout(BROWSER_WAIT, callback.code(&state))
        .await
        .map_err(|_| GithubError::Auth("nobody finished making the App".into()))?
        .map_err(|error| GithubError::Auth(error.0))?;
    drop(callback);
    let made = make(data_dir, &host, landed.code()).await;
    landed.answer(ended(&made)).await;
    let made = made?;
    sink.send(json!({ "url": install_url(&made) }))
        .map_err(|_| GithubError::Auth("making the App was closed".into()))?;
    Ok(())
}

/// Trades the manifest's code for the App, and keeps it here.
async fn make(data_dir: &Path, host: &str, code: &str) -> GithubResult<App> {
    if !plain_name(code) {
        return Err(GithubError::Auth(
            "GitHub came back with a code it could not have made".into(),
        ));
    }
    let (made, pem) = convert(host, code).await?;
    let saved = made.clone();
    let data = data_dir.to_path_buf();
    sikemux_plugin_api::store::blocking(move || app::remember(&data, &saved, &pem)).await?;
    Ok(made)
}

pub async fn forget(data_dir: &Path, ask: Forget) -> GithubResult<()> {
    let data = data_dir.to_path_buf();
    sikemux_plugin_api::store::blocking(move || app::forget(&data, &ask.app)).await
}

fn plain_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || letter == '-')
}

fn proposed_name(login: Option<&str>) -> String {
    let name = match login.filter(|login| !login.is_empty()) {
        Some(login) => format!("{login} Sikemux agents"),
        None => "Sikemux agents".to_owned(),
    };
    name.chars()
        .take(MAX_NAME)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn manifest(name: &str, redirect: &str) -> serde_json::Value {
    json!({
        "name": name,
        "url": "https://sikemux.com",
        "description": "The identity Sikemux agents commit, push and open pull requests as.",
        "redirect_url": redirect,
        "public": false,
        "default_events": [],
        "default_permissions": {
            "contents": "write",
            "pull_requests": "write",
            "issues": "write",
            "actions": "write",
            "checks": "read",
            "statuses": "read",
            "metadata": "read",
            "workflows": "write",
        },
    })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn posting_page(action: &str, manifest: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>Sikemux</title>\
         <body style=\"font:15px -apple-system,sans-serif;padding:48px\">\
         <form method=post action=\"{}\"><input type=hidden name=manifest value=\"{}\">\
         <p>Taking you to GitHub to create the GitHub App.</p>\
         <button type=submit>Continue to GitHub</button></form>\
         <script>document.forms[0].submit()</script>",
        escape(action),
        escape(manifest)
    )
}

#[derive(Deserialize)]
struct Owner {
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct Converted {
    id: u64,
    slug: String,
    client_id: String,
    pem: String,
    owner: Owner,
}

#[derive(Deserialize)]
struct Bot {
    id: u64,
    login: String,
}

async fn convert(host: &str, code: &str) -> GithubResult<(App, String)> {
    let client = HTTP.client()?;
    let base = config::api_base(host);
    let request = |method: Method, path: String| {
        client
            .request(method, format!("{base}{path}"))
            .header("Accept", JSON)
            .header("X-GitHub-Api-Version", API_VERSION)
    };
    let converted: Converted = HTTP
        .ask(
            request(Method::POST, format!("/app-manifests/{code}/conversions")),
            Read::ALL,
        )
        .await?
        .succeeded::<Github>()?
        .parsed::<Converted, Github>()?;
    let bot: Bot = HTTP
        .ask(
            request(Method::GET, format!("/users/{}%5Bbot%5D", converted.slug)),
            Read::ALL,
        )
        .await?
        .succeeded::<Github>()?
        .parsed::<Bot, Github>()?;
    let app = App {
        id: converted.id,
        slug: converted.slug,
        client_id: converted.client_id,
        host: host.to_owned(),
        owner: converted.owner.login,
        organization: converted.owner.kind == "Organization",
        bot_id: bot.id,
        bot_login: bot.login,
    };
    Ok((app, converted.pem))
}
