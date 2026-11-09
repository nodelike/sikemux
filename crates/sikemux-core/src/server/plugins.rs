//! The app's built-in plugins. The binary that runs the core provides them, so
//! they run here alone, with the person's own sign-ins: the window reaches them
//! through its calls, agents through their tools, and the board acts on code
//! hosts and trackers while the app is closed. Without them, as in tests, every
//! call says they are missing.

use super::connection::blocking;
use crate::git::remote::remote_urls;
use crate::server::changed::What;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub use sikemux_plugin_api::FailureCategory;
use sikemux_plugin_api::SignInStatus;
use sikemux_view::plugins::Listed;

mod manifest;
pub use manifest::{AgentTool, CodeHostSpec, Compare, Manifest};

use crate::cli::protocol::HarnessRequest;

use super::changed::Topic;
use super::connection::ClientConn;
use super::{Core, CoreError, CoreResult};

/// How a plugin call failed, as the window shows it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginFailure {
    pub plugin: String,
    pub category: FailureCategory,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl From<PluginFailure> for String {
    fn from(failure: PluginFailure) -> Self {
        failure.message
    }
}

impl PluginFailure {
    pub fn new(plugin: &str, category: FailureCategory, message: impl Into<String>) -> Self {
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
pub enum PluginOutcome {
    Value(Value),
    Failed(PluginFailure),
}

impl PluginOutcome {
    /// The value inside the answer, read as a `T`, or what its failure said.
    pub fn read<T: serde::de::DeserializeOwned>(self) -> Result<T, String> {
        match self {
            PluginOutcome::Value(value) => {
                serde_json::from_value(value).map_err(|error| error.to_string())
            }
            PluginOutcome::Failed(failure) => Err(failure.message),
        }
    }
}

/// One line of a plugin stream on the tool endpoint.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StreamEvent {
    Item { value: Value },
    End,
    Error { error: PluginFailure },
}

pub type Answer<'a, T> = Pin<Box<dyn Future<Output = Result<T, PluginFailure>> + Send + 'a>>;
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

fn plugins(core: &Core, plugin: &str) -> Result<&'static dyn Plugins, PluginFailure> {
    present(core.config.plugins, plugin)
}

fn present(plugins: Option<Provided>, plugin: &str) -> Result<&'static dyn Plugins, PluginFailure> {
    plugins.map(|provided| provided.0).ok_or_else(|| {
        PluginFailure::new(
            plugin,
            FailureCategory::NotInstalled,
            format!("Sikemux's {plugin} plugin is not available to its background process"),
        )
    })
}

/// What the plugins set for an agent, terminal or task started in `cwd`,
/// asked of the project that holds it.
pub(crate) fn agent_environment(core: &Core, cwd: &Path) -> BTreeMap<String, String> {
    let Some(Provided(plugins)) = core.config.plugins else {
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
) -> Result<Value, PluginFailure> {
    call_on(core.config.plugins, plugin, method, params).await
}

/// Calls `method` of `plugin` among `plugins`, for what holds them without the core.
pub(crate) async fn call_on(
    plugins: Option<Provided>,
    plugin: &str,
    method: &str,
    params: Value,
) -> Result<Value, PluginFailure> {
    present(plugins, plugin)?.call(plugin, method, params).await
}

/// A plugin's own method and what it takes, which only the plugin knows the shape of.
#[derive(Serialize, Deserialize)]
pub struct PluginCall {
    pub plugin: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A call from a plugin's pane. A pane signing in or out changes what its
/// plugin offers agents, so they are told to list their tools again.
pub(crate) async fn window_call(core: &Core, call: PluginCall) -> PluginOutcome {
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
        Ok(value) => PluginOutcome::Value(value),
        Err(failure) => PluginOutcome::Failed(failure),
    }
}

pub(crate) fn tell_agents_if_theirs(core: &Core, plugin: &str) {
    if core
        .config
        .plugins
        .is_some_and(|Provided(plugins)| plugins.offers_agent_tools(plugin))
    {
        core.harness.plugins_changed();
    }
}

pub(crate) fn manifests(core: &Core) -> Vec<&'static Manifest> {
    core.config
        .plugins
        .map(|Provided(plugins)| plugins.manifests())
        .unwrap_or_default()
}

pub(crate) fn manifest(core: &Core, plugin: &str) -> Option<&'static Manifest> {
    manifests(core)
        .into_iter()
        .find(|manifest| manifest.id == plugin)
}

/// `plugin` ran `method`; when that is a code host's write, what the core keeps of the host
/// is read again at once, and again once the service has settled.
pub(crate) fn wrote(core: &Arc<Core>, plugin: &str, method: &str, params: &Value) {
    if !manifest(core, plugin).is_some_and(|manifest| {
        manifest.code_host.is_some() && sikemux_plugin_api::codehost::writes(method)
    }) {
        return;
    }
    if method == "createPull" {
        super::board::proof::pull_opened(core);
    }
    super::codehost::wrote(core, plugin, params);
}

#[derive(Serialize, Deserialize)]
pub struct Disabled {
    pub ids: Vec<String>,
}

pub(crate) fn set_disabled(core: &Core, disabled: Disabled) {
    if let Some(Provided(plugins)) = core.config.plugins {
        plugins.set_disabled(disabled.ids);
        core.harness.plugins_changed();
        core.changed(What::Read(Topic::Plugins));
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
            call: PluginCall {
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

/// Every plugin built in, each with whether its sign-in works and as whom, asked of its
/// service now.
pub(crate) async fn listed(core: &Core) -> Vec<Listed> {
    let disabled = disabled(core);
    let asked = manifests(core).into_iter().map(|manifest| {
        let offers = |method: &str| manifest.account_methods.iter().any(|named| named == method);
        let mut listed = Listed {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            enabled: !disabled.contains(&manifest.id),
            signed_in: None,
            signed_in_as: None,
            problem: None,
            browser_sign_in: false,
            sign_out: offers("signOut"),
            token_form: manifest.token_form.clone(),
        };
        let browser = offers("signInWithBrowser");
        async move {
            if !listed.enabled {
                return listed;
            }
            match call(core, &listed.id, "status", Value::Null).await {
                Ok(status) => {
                    listed.browser_sign_in = browser && status["browserSignIn"] != false;
                    let status = serde_json::from_value::<SignInStatus<IgnoredAny>>(status).ok();
                    listed.signed_in = Some(
                        status
                            .as_ref()
                            .is_some_and(|status| status.configured && !status.auth_failed),
                    );
                    if let Some(status) = status {
                        listed.signed_in_as = status.signed_in_as;
                        listed.problem = status.message;
                    }
                }
                Err(failure) if failure.category == FailureCategory::UnknownMethod => {}
                Err(failure) => {
                    listed.signed_in = Some(false);
                    listed.problem = Some(failure.message);
                }
            }
            listed
        }
    });
    futures_util::future::join_all(asked).await
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
        core.changed(What::Read(Topic::Plugins));
    }
}

/// The plugins the person switched off, which every client shows.
pub(crate) fn disabled(core: &Core) -> Vec<String> {
    disabled_in(core.config.plugins)
}

pub(crate) fn disabled_in(plugins: Option<Provided>) -> Vec<String> {
    plugins
        .map(|Provided(plugins)| plugins.disabled())
        .unwrap_or_default()
}

#[derive(Serialize, Deserialize)]
pub struct StreamCall {
    /// The client's number for the stream its items arrive as.
    pub stream: u64,
    #[serde(flatten)]
    pub call: PluginCall,
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
        call: PluginCall {
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

pub(crate) async fn agent_tools(core: &Core, project: &str) -> Result<Value, String> {
    let Some(Provided(plugins)) = core.config.plugins else {
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
    let Some(Provided(plugins)) = core.config.plugins else {
        return Err("plugins are not loaded".into());
    };
    let deploy = super::ship::check_tool(core, request, name, &arguments).await?;
    if let Some(gated) = plugins.gated(name) {
        return super::approvals::propose(core, request, gated, arguments).await;
    }
    run_agent_tool(core, name, arguments, deploy).await
}

/// Runs a tool call that may go, following it to its end when it deploys.
pub(crate) async fn run_agent_tool(
    core: &Arc<Core>,
    name: &str,
    arguments: Value,
    deploy: Option<super::ship::Deploy>,
) -> Result<Value, String> {
    let Some(Provided(plugins)) = core.config.plugins else {
        return Err("plugins are not loaded".into());
    };
    let answer = plugins
        .call_agent_tool(name, arguments)
        .await
        .map_err(|failure| explain(&failure));
    if let Some(deploy) = deploy {
        let answered = answer.as_ref().map_err(String::as_str);
        super::ship::tool_ran(core, deploy, name, answered);
    }
    let answer = answer?;
    tool_ran(core, name);
    Ok(answer)
}

/// An agent ran the tool `tool`, which may be one of a plugin's writes.
fn tool_ran(core: &Arc<Core>, tool: &str) {
    let found = manifests(core).into_iter().find_map(|manifest| {
        let tool = manifest.tools.iter().find(|each| each.name == tool)?;
        Some((manifest.id.as_str(), tool.method.as_str()))
    });
    if let Some((plugin, method)) = found {
        wrote(core, plugin, method, &Value::Null);
    }
}

/// An agent cannot sign in on the person's behalf, so a signed-out plugin says
/// who can fix it and where.
pub(crate) fn explain(failure: &PluginFailure) -> String {
    let signed_out = matches!(
        failure.category,
        FailureCategory::Unconfigured | FailureCategory::Auth
    ) || matches!(failure.status, Some(401 | 403));
    if signed_out {
        format!(
            "{} is not signed in ({}). Ask the person to sign in from its pane in Sikemux, then try again; do not switch to another tool or MCP server for it.",
            failure.plugin, failure.message
        )
    } else {
        format!("{}: {}", failure.plugin, failure.message)
    }
}

/// Refuses a program an agent started: it could sign the person's plugin in
/// to an account of its own choosing.
pub(crate) async fn not_an_agent(client: &ClientConn) -> CoreResult<()> {
    let Some(pid) = client.pid else {
        return Ok(());
    };
    match blocking(move || Ok(super::callers::agent_of(pid))).await? {
        Some(_) => {
            Err("an agent cannot sign the person in to a service; the person runs this".into())
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::remote::with_real_ssh_host;

    fn failure(category: FailureCategory, status: Option<u16>) -> PluginFailure {
        PluginFailure {
            status,
            ..PluginFailure::new("sikemux.signoz", category, "signoz: not configured")
        }
    }

    #[test]
    fn a_signed_out_plugin_tells_the_agent_who_can_fix_it() {
        for signed_out in [
            failure(FailureCategory::Unconfigured, None),
            failure(FailureCategory::Http, Some(401)),
        ] {
            assert!(explain(&signed_out).contains("Ask the person to sign in"));
        }
        assert_eq!(
            explain(&failure(FailureCategory::BadParams, None)),
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
            ("api", "https://bitbucket.org/team/api.git"),
            ("web", "https://bitbucket.org/team/web.git"),
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
                "https://bitbucket.org/team/api.git",
                "https://bitbucket.org/team/web.git"
            ]
        );
    }
}
