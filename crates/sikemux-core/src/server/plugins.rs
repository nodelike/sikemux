//! The app's built-in plugins. The binary that runs the core provides them, so
//! they run here alone, with the person's own sign-ins: the window reaches them
//! through its calls, agents through their tools, and the board acts on code
//! hosts and trackers while the app is closed. Without them, as in tests, every
//! call says they are missing.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::SignInStatus;
pub use sikemux_plugin_api::{Category, CodeHostSpec, Listed, Manifest};

use crate::cli::protocol::HarnessRequest;

use super::changed::Topic;
use super::connection::ClientConn;
use super::{Core, CoreError, CoreResult};

/// How a plugin call failed, as the window shows it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    pub plugin: String,
    pub category: Category,
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
    pub fn new(plugin: &str, category: Category, message: impl Into<String>) -> Self {
        Self {
            plugin: plugin.to_owned(),
            category,
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

/// A plugin tool whose every call the person approves first: `check` answers
/// what they are asked, and `method` runs once they approve.
pub struct Gated {
    pub plugin: String,
    pub method: String,
    pub check: String,
}

pub trait Plugins: Send + Sync {
    fn manifests(&self) -> Vec<&Manifest>;
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
    /// The tool an agent knows by `name`, when the person approves each call to it.
    fn gated(&self, name: &str) -> Option<Gated>;
    fn call_agent_tool<'a>(&'a self, name: &'a str, arguments: Value) -> Answer<'a, Value>;
}

/// The plugins the binary that runs the core provides. They live as long as
/// the process does.
#[derive(Clone, Copy)]
pub struct Provided(pub &'static dyn Plugins);

impl std::fmt::Debug for Provided {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Provided")
    }
}

fn plugins(core: &Core, plugin: &str) -> Result<&'static dyn Plugins, Failure> {
    present(core.plugins, plugin)
}

fn present(plugins: Option<Provided>, plugin: &str) -> Result<&'static dyn Plugins, Failure> {
    plugins.map(|provided| provided.0).ok_or_else(|| {
        Failure::new(
            plugin,
            Category::NotInstalled,
            format!("Sikemux's {plugin} plugin is not available to its background process"),
        )
    })
}

/// What the plugins set for an agent, terminal or task started in `cwd`,
/// asked of the project that holds it.
pub(crate) fn agent_environment(core: &Core, cwd: &Path) -> BTreeMap<String, String> {
    let Some(Provided(plugins)) = core.plugins else {
        return BTreeMap::new();
    };
    let project = super::workspace::project_of(cwd, &core.workspaces.project_paths());
    plugins.agent_environment(Path::new(&project))
}

pub(crate) async fn call(
    core: &Core,
    plugin: &str,
    method: &str,
    params: Value,
) -> Result<Value, Failure> {
    call_on(core.plugins, plugin, method, params).await
}

/// Calls `method` of `plugin` among `plugins`, for what holds them without the core.
pub(crate) async fn call_on(
    plugins: Option<Provided>,
    plugin: &str,
    method: &str,
    params: Value,
) -> Result<Value, Failure> {
    present(plugins, plugin)?.call(plugin, method, params).await
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
    let outcome = match plugins(core, &call.plugin) {
        Ok(plugins) => {
            plugins
                .call(&call.plugin, &call.method, call.params.clone())
                .await
        }
        Err(failure) => Err(failure),
    };
    if let (true, Some(core)) = (outcome.is_ok(), core.arc()) {
        wrote(&core, &call.plugin, &call.method, &call.params);
    }
    tell_agents_if_theirs(core, &call.plugin);
    sign_in_moved(core, &call.plugin, &call.method);
    match outcome {
        Ok(value) => Outcome::Value(value),
        Err(failure) => Outcome::Failed(failure),
    }
}

pub(crate) fn tell_agents_if_theirs(core: &Core, plugin: &str) {
    if core
        .plugins
        .is_some_and(|Provided(plugins)| plugins.offers_agent_tools(plugin))
    {
        core.harness.plugins_changed();
    }
}

pub(crate) fn manifests(core: &Core) -> Vec<&'static Manifest> {
    core.plugins
        .map(|Provided(plugins)| plugins.manifests())
        .unwrap_or_default()
}

pub(crate) fn manifest(core: &Core, plugin: &str) -> Option<&'static Manifest> {
    manifests(core)
        .into_iter()
        .find(|manifest| manifest.id == plugin)
}

/// `plugin` ran `method`; when its manifest names it as a write, what it
/// shows is read again at once, and again once the service has settled.
pub(crate) fn wrote(core: &Arc<Core>, plugin: &str, method: &str, params: &Value) {
    if !manifest(core, plugin)
        .is_some_and(|manifest| manifest.writes.iter().any(|write| write == method))
    {
        return;
    }
    super::plugin_views::wrote(core, plugin);
    super::codehost::wrote(core, plugin, params);
}

#[derive(Deserialize)]
pub(crate) struct Disabled {
    ids: Vec<String>,
}

pub(crate) fn set_disabled(core: &Core, disabled: Disabled) {
    if let Some(Provided(plugins)) = core.plugins {
        plugins.set_disabled(disabled.ids);
        core.harness.plugins_changed();
        core.changes.changed(Topic::Plugins, None, None);
        core.view_changed();
    }
}

#[derive(Deserialize)]
pub(crate) struct SignIn {
    stream: u64,
    plugin: String,
}

/// The plugin's browser sign-in as the client's stream, which sends the
/// address to open and ends once the person has signed in.
pub(crate) fn sign_in(
    core: &Arc<Core>,
    connection: Arc<ClientConn>,
    sign_in: SignIn,
) -> CoreResult<Value> {
    stream(
        core,
        connection,
        StreamCall {
            stream: sign_in.stream,
            call: WindowCall {
                plugin: sign_in.plugin,
                method: "signInWithBrowser".into(),
                params: Value::Null,
            },
        },
    )
}

#[derive(Deserialize)]
pub(crate) struct Landed {
    /// The address the browser came back to, whole or as its path and query.
    target: String,
}

/// Finishes a browser sign-in whose browser came back on another machine,
/// such as one the person opened from a host reached over SSH.
pub(crate) fn sign_in_callback(core: &Core, landed: Landed) -> CoreResult<()> {
    if landed.target.len() > 8 * 1024 {
        return Err("that is not a sign-in address".into());
    }
    plugins(core, "sign-in")
        .map_err(|failure| CoreError::from(failure.message))?
        .relay_sign_in(&landed.target)
        .map_err(CoreError::from)
}

/// Every plugin built in, each with whether its sign-in works, asked of its
/// service now.
pub(crate) async fn listed(core: &Core) -> Vec<Listed> {
    let disabled = disabled(core);
    let asked = manifests(core).into_iter().map(|manifest| {
        let id = manifest.id.clone();
        let name = manifest.name.clone();
        let enabled = !disabled.contains(&id);
        async move {
            let signed_in = if enabled {
                match call(core, &id, "status", Value::Null).await {
                    Ok(status) => Some(
                        serde_json::from_value::<SignInStatus<IgnoredAny>>(status)
                            .is_ok_and(|status| status.configured && !status.auth_failed),
                    ),
                    Err(failure) if failure.category == Category::UnknownMethod => None,
                    Err(_) => Some(false),
                }
            } else {
                None
            };
            Listed {
                id,
                name,
                enabled,
                signed_in,
                token_form: manifest.token_form.clone(),
            }
        }
    });
    futures_util::future::join_all(asked).await
}

#[derive(Deserialize)]
pub(crate) struct LinkParams {
    url: String,
}

/// The plugin whose own view shows what a web address names, and where in that view, as
/// the first enabled plugin's `resolveLink` answers; null when none does.
pub(crate) async fn link(core: &Core, params: LinkParams) -> Value {
    let disabled = disabled(core);
    let url = params.url;
    let asked = manifests(core).into_iter().filter_map(|manifest| {
        let id = manifest.id.clone();
        let url = url.clone();
        (!disabled.contains(&id)).then_some(async move {
            let at = call(core, &id, "resolveLink", json!({ "url": url }))
                .await
                .ok()?;
            (!at.is_null()).then(|| json!({ "plugin": id, "at": at }))
        })
    });
    futures_util::future::join_all(asked)
        .await
        .into_iter()
        .flatten()
        .next()
        .unwrap_or(Value::Null)
}

/// A plugin's call may have signed it in or out, or picked another account,
/// so clients read the plugins again.
pub(crate) fn sign_in_moved(core: &Core, plugin: &str, method: &str) {
    let account_moved = manifest(core, plugin).is_some_and(|manifest| {
        manifest
            .account_methods
            .iter()
            .any(|account| account == method)
    });
    if account_moved {
        core.changes.changed(Topic::Plugins, None, None);
        super::plugin_views::signed(core, plugin);
    }
}

/// The plugins the person switched off, which every client shows.
pub(crate) fn disabled(core: &Core) -> Vec<String> {
    disabled_in(core.plugins)
}

pub(crate) fn disabled_in(plugins: Option<Provided>) -> Vec<String> {
    plugins
        .map(|Provided(plugins)| plugins.disabled())
        .unwrap_or_default()
}

#[derive(Deserialize)]
pub(crate) struct StreamCall {
    /// The client's number for the stream its items arrive as.
    stream: u64,
    #[serde(flatten)]
    call: WindowCall,
}

/// Runs a plugin's stream, its items arriving as the client's stream on
/// `connection`. A stream that signs the plugin in or out changes what it
/// offers agents and what the rail shows, so both hear of it when it ends.
pub(crate) fn stream(
    core: &Arc<Core>,
    connection: Arc<ClientConn>,
    call: StreamCall,
) -> CoreResult<Value> {
    let StreamCall {
        stream: id,
        call: WindowCall {
            plugin,
            method,
            params,
        },
    } = call;
    let stream = core.streams.open(connection, id)?;
    let core = core.clone();
    tokio::spawn(async move {
        let items = stream.clone();
        let sink: Sink = Box::new(move |value| items.push(value));
        let ended = match plugins(&core, &plugin) {
            Ok(plugins) => plugins.stream(&plugin, &method, params, sink).await,
            Err(failure) => Err(failure),
        };
        match ended {
            Ok(()) => stream.finish(None),
            Err(failure) => stream.fail(serde_json::to_value(failure).unwrap_or_default()),
        }
        tell_agents_if_theirs(&core, &plugin);
        sign_in_moved(&core, &plugin, &method);
    });
    Ok(json!({ "stream": id }))
}

/// The tools an agent working in `project` is offered.
pub(crate) async fn agent_tools(core: &Core, project: &str) -> Result<Value, String> {
    let Some(Provided(plugins)) = core.plugins else {
        return Ok(Value::Array(Vec::new()));
    };
    let project = PathBuf::from(project);
    let remotes = tokio::task::spawn_blocking(move || remote_urls(&project))
        .await
        .map_err(|error| error.to_string())?;
    Ok(Value::Array(plugins.agent_tools(&remotes).await))
}

/// Runs the tool an agent called, or puts the call to the person when they
/// approve each one.
pub(crate) async fn call_agent_tool(
    core: &Arc<Core>,
    request: &HarnessRequest,
) -> Result<Value, String> {
    let params = &request.params;
    let name = params
        .get("tool")
        .and_then(Value::as_str)
        .ok_or("plugins.call needs the tool's name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let Some(Provided(plugins)) = core.plugins else {
        return Err("plugins are not loaded".into());
    };
    if let Some(gated) = plugins.gated(name) {
        return super::approvals::propose(core, request, gated, arguments).await;
    }
    let answer = plugins
        .call_agent_tool(name, arguments)
        .await
        .map_err(|failure| explain(&failure))?;
    super::plugin_views::tool_ran(core, name);
    Ok(answer)
}

/// An agent cannot sign in on the person's behalf, so a signed-out plugin says
/// who can fix it and where.
pub(crate) fn explain(failure: &Failure) -> String {
    let signed_out = matches!(failure.category, Category::Unconfigured | Category::Auth)
        || matches!(failure.status, Some(401 | 403));
    if signed_out {
        format!(
            "{} is not signed in ({}). Ask the person to sign in from its pane in Sikemux, then try again; do not switch to another tool or MCP server for it.",
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

    fn failure(category: Category, status: Option<u16>) -> Failure {
        Failure {
            status,
            ..Failure::new("sikemux.signoz", category, "signoz: not configured")
        }
    }

    #[test]
    fn a_signed_out_plugin_tells_the_agent_who_can_fix_it() {
        for signed_out in [
            failure(Category::Unconfigured, None),
            failure(Category::Http, Some(401)),
        ] {
            assert!(explain(&signed_out).contains("Ask the person to sign in"));
        }
        assert_eq!(
            explain(&failure(Category::BadParams, None)),
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
