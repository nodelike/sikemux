//! The app's built-in plugins. The binary that runs the core provides them, so
//! they run here alone, with the person's own sign-ins: the window reaches them
//! through its calls, agents through their tools, and the board acts on code
//! hosts and trackers while the app is closed. Without them, as in tests, every
//! call says they are missing.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use super::{Core, CoreError, CoreResult};

/// How a plugin call failed, as the window shows it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    pub plugin: String,
    pub category: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        failure.message
    }
}

impl Failure {
    pub fn new(plugin: &str, category: &str, message: impl Into<String>) -> Self {
        Self {
            plugin: plugin.to_owned(),
            category: category.to_owned(),
            message: message.into(),
            status: None,
        }
    }
}

/// What the core answers the window's `plugin` call with.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Value(Value),
    Failed(Failure),
}

/// One line of a plugin stream on the tool endpoint.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StreamEvent {
    Item { value: Value },
    End,
    Error { error: Failure },
}

pub type Answer<'a, T> = Pin<Box<dyn Future<Output = Result<T, Failure>> + Send + 'a>>;
pub type Listing<'a> = Pin<Box<dyn Future<Output = Vec<Value>> + Send + 'a>>;
/// Takes a stream's next item; false once nobody reads them, which ends it.
pub type Sink = Box<dyn Fn(Value) -> bool + Send + Sync>;

pub trait Plugins: Send + Sync {
    fn manifests(&self) -> Vec<Value>;
    fn set_disabled(&self, ids: Vec<String>);
    fn disabled(&self) -> Vec<String>;
    /// Hands a waiting browser sign-in the address the browser came back to on
    /// another machine.
    fn relay_sign_in(&self, landed: &str) -> Result<(), String>;
    fn offers_agent_tools(&self, plugin: &str) -> bool;
    /// What the plugins set for an agent or terminal started in `project`.
    fn agent_environment(&self, project: &Path) -> BTreeMap<String, String>;
    fn call<'a>(&'a self, plugin: &'a str, method: &'a str, params: Value) -> Answer<'a, Value>;
    /// Runs until the stream ends. Dropping the answer stops it.
    fn stream<'a>(
        &'a self,
        plugin: &'a str,
        method: &'a str,
        params: Value,
        sink: Sink,
    ) -> Answer<'a, ()>;
    /// The tools an agent in a repository with these git remotes is offered.
    fn agent_tools<'a>(&'a self, remotes: &'a [String]) -> Listing<'a>;
    fn call_agent_tool<'a>(&'a self, name: &'a str, arguments: Value) -> Answer<'a, Value>;
}

static PLUGINS: OnceLock<Box<dyn Plugins>> = OnceLock::new();

/// Gives the core its plugins. Call once, before the core starts.
pub fn provide(plugins: Box<dyn Plugins>) {
    let _ = PLUGINS.set(plugins);
}

fn plugins(plugin: &str) -> Result<&'static dyn Plugins, Failure> {
    PLUGINS.get().map(|plugins| &**plugins).ok_or_else(|| {
        Failure::new(
            plugin,
            "not-installed",
            format!("Sikemux's {plugin} plugin is not available to its background process"),
        )
    })
}

/// What the plugins set for an agent, terminal or task started in `cwd`,
/// asked of the project that holds it.
pub(crate) fn agent_environment(core: &Core, cwd: &Path) -> BTreeMap<String, String> {
    let Some(plugins) = PLUGINS.get() else {
        return BTreeMap::new();
    };
    let project = super::names::project_of(cwd, &core.workspaces.project_paths());
    plugins.agent_environment(Path::new(&project))
}

pub(crate) async fn call(plugin: &str, method: &str, params: Value) -> Result<Value, Failure> {
    plugins(plugin)?.call(plugin, method, params).await
}

#[derive(Deserialize)]
pub(crate) struct WindowCall {
    plugin: String,
    method: String,
    #[serde(default)]
    params: Value,
}

/// A call from a plugin's pane. A pane signing in or out changes what its
/// plugin offers agents, so they are told to list their tools again.
pub(crate) async fn window_call(core: &Core, call: WindowCall) -> Outcome {
    let outcome = match plugins(&call.plugin) {
        Ok(plugins) => plugins.call(&call.plugin, &call.method, call.params).await,
        Err(failure) => Err(failure),
    };
    tell_agents_if_theirs(core, &call.plugin);
    match outcome {
        Ok(value) => Outcome::Value(value),
        Err(failure) => Outcome::Failed(failure),
    }
}

pub(crate) fn tell_agents_if_theirs(core: &Core, plugin: &str) {
    if PLUGINS
        .get()
        .is_some_and(|plugins| plugins.offers_agent_tools(plugin))
    {
        core.harness.plugins_changed();
    }
}

pub(crate) fn manifests() -> Vec<Value> {
    PLUGINS
        .get()
        .map(|plugins| plugins.manifests())
        .unwrap_or_default()
}

#[derive(Deserialize)]
pub(crate) struct Disabled {
    ids: Vec<String>,
}

pub(crate) fn set_disabled(core: &Core, disabled: Disabled) {
    if let Some(plugins) = PLUGINS.get() {
        plugins.set_disabled(disabled.ids);
        core.harness.plugins_changed();
        core.view_changed();
    }
}

#[derive(Deserialize)]
pub(crate) struct SignIn {
    plugin: String,
}

/// A stream ticket for the plugin's browser sign-in, which sends the address
/// to open and ends once the person has signed in.
pub(crate) fn sign_in_ticket(sign_in: SignIn) -> CoreResult<Ticket> {
    stream_ticket(WindowCall {
        plugin: sign_in.plugin,
        method: "signInWithBrowser".into(),
        params: Value::Null,
    })
}

#[derive(Deserialize)]
pub(crate) struct Landed {
    /// The address the browser came back to, whole or as its path and query.
    target: String,
}

/// Finishes a browser sign-in whose browser came back on another machine,
/// such as one the person opened from a host reached over SSH.
pub(crate) fn sign_in_callback(landed: Landed) -> CoreResult<()> {
    if landed.target.len() > 8 * 1024 {
        return Err("that is not a sign-in address".into());
    }
    plugins("sign-in")
        .map_err(|failure| CoreError::from(failure.message))?
        .relay_sign_in(&landed.target)
        .map_err(CoreError::from)
}

/// The plugins the person switched off, which every client shows.
pub(crate) fn disabled() -> Vec<String> {
    PLUGINS
        .get()
        .map(|plugins| plugins.disabled())
        .unwrap_or_default()
}

/// A stream the window asked for and has yet to collect from the tool
/// endpoint, by the ticket it was given.
pub(crate) struct Pending {
    pub(crate) plugin: String,
    pub(crate) method: String,
    pub(crate) params: Value,
}

const TICKET_LIFETIME: Duration = Duration::from_secs(30);
const MAX_TICKETS: usize = 64;

static TICKETS: Mutex<Vec<(String, Instant, Pending)>> = Mutex::new(Vec::new());

#[derive(Serialize)]
pub(crate) struct Ticket {
    ticket: String,
}

/// Holds a stream for the window to collect over the tool endpoint, which
/// only it may ask for, though anyone holding the endpoint's token may connect.
pub(crate) fn stream_ticket(call: WindowCall) -> CoreResult<Ticket> {
    let mut tickets = TICKETS.locked();
    tickets.retain(|(_, made, _)| made.elapsed() < TICKET_LIFETIME);
    if tickets.len() >= MAX_TICKETS {
        return Err(CoreError::from(
            "too many plugin streams are starting at once",
        ));
    }
    let ticket = crate::cli::auth::new_nonce();
    tickets.push((
        ticket.clone(),
        Instant::now(),
        Pending {
            plugin: call.plugin,
            method: call.method,
            params: call.params,
        },
    ));
    Ok(Ticket { ticket })
}

pub(crate) fn redeem(ticket: &str) -> Option<Pending> {
    let mut tickets = TICKETS.locked();
    let at = tickets.iter().position(|(held, made, _)| {
        crate::cli::auth::same_secret(held, ticket) && made.elapsed() < TICKET_LIFETIME
    })?;
    Some(tickets.swap_remove(at).2)
}

pub(crate) async fn stream(pending: &Pending, sink: Sink) -> Result<(), Failure> {
    plugins(&pending.plugin)?
        .stream(
            &pending.plugin,
            &pending.method,
            pending.params.clone(),
            sink,
        )
        .await
}

/// The tools an agent working in `project` is offered.
pub(crate) async fn agent_tools(project: &str) -> Result<Value, String> {
    let Some(plugins) = PLUGINS.get() else {
        return Ok(Value::Array(Vec::new()));
    };
    let project = PathBuf::from(project);
    let remotes = tokio::task::spawn_blocking(move || remote_urls(&project))
        .await
        .map_err(|error| error.to_string())?;
    Ok(Value::Array(plugins.agent_tools(&remotes).await))
}

pub(crate) async fn call_agent_tool(params: &Value) -> Result<Value, String> {
    let name = params
        .get("tool")
        .and_then(Value::as_str)
        .ok_or("plugins.call needs the tool's name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let Some(plugins) = PLUGINS.get() else {
        return Err("plugins are not loaded".into());
    };
    plugins
        .call_agent_tool(name, arguments)
        .await
        .map_err(|failure| explain(&failure))
}

/// An agent cannot sign in on the person's behalf, so a signed-out plugin says
/// who can fix it and where.
fn explain(failure: &Failure) -> String {
    let signed_out = matches!(failure.category.as_str(), "unconfigured" | "auth")
        || matches!(failure.status, Some(401 | 403));
    if signed_out {
        format!(
            "{} is not signed in ({}). Ask the person to sign in from its pane in Sikemux, then try again.",
            failure.plugin, failure.message
        )
    } else {
        format!("{}: {}", failure.plugin, failure.message)
    }
}

const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const GIT_OUTPUT_LIMIT: usize = 1024 * 1024;

/// The URL of every remote of the repository `path` is in. A folder that is
/// not in one but holds repositories gets the remotes of those.
fn remote_urls(path: &Path) -> Vec<String> {
    if let Some(urls) = urls_of(path) {
        return urls;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut urls: Vec<String> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|child| child.join(".git").exists())
        .filter_map(|child| urls_of(&child))
        .flatten()
        .collect();
    urls.sort();
    urls.dedup();
    urls
}

/// None when `repo` is not inside a repository.
fn urls_of(repo: &Path) -> Option<Vec<String>> {
    let listed = crate::git::Git::at(repo)
        .timeout(GIT_TIMEOUT)
        .limit(GIT_OUTPUT_LIMIT)
        .text(&["remote", "-v"])
        .ok()?;
    Some(
        listed
            .lines()
            .filter_map(|line| line.strip_suffix(" (fetch)")?.split_once('\t'))
            .map(|(_, url)| with_real_ssh_host(url, ssh_hostname))
            .collect(),
    )
}

/// A remote such as `git@github-work:org/repo` names a host alias from
/// ~/.ssh/config; this swaps in the host it stands for, so a plugin can tell
/// which service the remote lives on.
fn with_real_ssh_host(remote_url: &str, resolve: impl Fn(&str) -> Option<String>) -> String {
    if let Ok(mut parsed) = url::Url::parse(remote_url) {
        if !matches!(parsed.scheme(), "ssh" | "git+ssh") {
            return remote_url.to_owned();
        }
        let Some(real) = parsed.host_str().and_then(&resolve) else {
            return remote_url.to_owned();
        };
        return match parsed.set_host(Some(&real)) {
            Ok(()) => parsed.into(),
            Err(_) => remote_url.to_owned(),
        };
    }
    let Some((user_host, path)) = remote_url.split_once(':') else {
        return remote_url.to_owned();
    };
    if user_host.contains('/') {
        return remote_url.to_owned();
    }
    let (user, host) = match user_host.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, user_host),
    };
    let Some(real) = resolve(host) else {
        return remote_url.to_owned();
    };
    match user {
        Some(user) => format!("{user}@{real}:{path}"),
        None => format!("{real}:{path}"),
    }
}

/// `ssh -G` prints the settings ssh would use for a host without connecting.
fn ssh_hostname(host: &str) -> Option<String> {
    if host.starts_with('-') {
        return None;
    }
    let mut command = sikemux_process::user_environment::command("ssh");
    command.args(["-G", host]);
    let output =
        sikemux_process::run(&mut command, None, GIT_TIMEOUT, GIT_OUTPUT_LIMIT, None).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("hostname "))
        .map(str::to_owned)
        .filter(|real| real != host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(category: &str, status: Option<u16>) -> Failure {
        Failure {
            status,
            ..Failure::new("sikemux.signoz", category, "signoz: not configured")
        }
    }

    #[test]
    fn a_signed_out_plugin_tells_the_agent_who_can_fix_it() {
        for signed_out in [failure("unconfigured", None), failure("http", Some(401))] {
            assert!(explain(&signed_out).contains("Ask the person to sign in"));
        }
        assert_eq!(
            explain(&failure("bad-params", None)),
            "sikemux.signoz: signoz: not configured"
        );
    }

    #[test]
    fn an_ssh_host_alias_is_replaced_by_the_host_it_stands_for() {
        let resolve = |host: &str| (host == "github-work").then(|| "github.com".to_owned());
        assert_eq!(
            with_real_ssh_host("git@github-work:org/repo.git", resolve),
            "git@github.com:org/repo.git"
        );
        assert_eq!(
            with_real_ssh_host("ssh://git@github-work/org/repo.git", resolve),
            "ssh://git@github.com/org/repo.git"
        );
        for untouched in [
            "git@bitbucket.org:team/repo.git",
            "https://github-work/org/repo.git",
            "/local/path/repo.git",
        ] {
            assert_eq!(with_real_ssh_host(untouched, resolve), untouched);
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = sikemux_process::user_environment::command("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn remote_urls_are_read_from_anywhere_inside_the_repository() {
        let repo = tempfile::tempdir().expect("tempdir");
        git(repo.path(), &["init", "--quiet"]);
        git(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:nodelike/sikemux.git",
            ],
        );
        git(
            repo.path(),
            &[
                "remote",
                "add",
                "mirror",
                "https://bitbucket.org/team/sikemux.git",
            ],
        );
        let inside = repo.path().join("src");
        std::fs::create_dir(&inside).expect("subdirectory");

        let mut urls = remote_urls(&inside);
        urls.sort();
        assert_eq!(
            urls,
            [
                "git@github.com:nodelike/sikemux.git",
                "https://bitbucket.org/team/sikemux.git"
            ]
        );
        let outside = tempfile::tempdir().expect("tempdir");
        assert!(remote_urls(outside.path()).is_empty());
    }

    #[test]
    fn a_folder_of_repositories_has_the_remotes_of_the_repositories_in_it() {
        let folder = tempfile::tempdir().expect("tempdir");
        for (name, url) in [
            ("api", "git@bitbucket.org:team/api.git"),
            ("web", "git@bitbucket.org:team/web.git"),
        ] {
            let repo = folder.path().join(name);
            std::fs::create_dir(&repo).expect("repository folder");
            git(&repo, &["init", "--quiet"]);
            git(&repo, &["remote", "add", "origin", url]);
        }
        std::fs::create_dir(folder.path().join("notes")).expect("plain folder");

        assert_eq!(
            remote_urls(folder.path()),
            [
                "git@bitbucket.org:team/api.git",
                "git@bitbucket.org:team/web.git"
            ]
        );
    }
}
