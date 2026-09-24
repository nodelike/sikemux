//! Every agent host learns about the browser tools a different way: a config
//! file named on the command line, a dotted override, an extension, or its own
//! home directory. This is that per-host knowledge in one place.
//!
//! A host's existing MCP servers are never disturbed. Each integration either
//! adds to what the user already configured or works on a private copy of the
//! host's home, so an agent launched outside Sikemux is unaffected.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use agent_client_protocol::schema::v1::{EnvVariable, McpServer, McpServerStdio};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::{browser_state_dir, validate_agent_id, BrowserManager, BrowserMcpLaunch};
use crate::error::{AppError, AppResult};

const CONFIG_TIMEOUT: Duration = Duration::from_secs(5);

/// The hosts that can be told about the browser tools. Anything else launches
/// untouched.
pub const SUPPORTED_AGENTS: &[&str] =
    &["claude", "codex", "grok", "hermes", "omp", "opencode", "pi"];

pub fn is_supported(agent_type: &str) -> bool {
    SUPPORTED_AGENTS.contains(&agent_type)
}

/// What the agent needs to start with: arguments in front of the ones the pane
/// already built, and environment for its process.
pub struct BrowserAgentIntegration {
    pub args_prefix: Vec<String>,
    pub environment: Vec<(String, String)>,
}

impl BrowserAgentIntegration {
    /// Fold these arguments into the launch the pane built, and hand back the
    /// environment. Sikemux's go first: a host that takes a subcommand wants
    /// its options before it.
    pub fn apply(mut self, args: &mut Vec<String>) -> Vec<(String, String)> {
        self.args_prefix.append(args);
        *args = self.args_prefix;
        self.environment
    }
}

impl BrowserManager {
    pub async fn agent_integration(
        &self,
        app: &AppHandle,
        agent_id: &str,
        agent_type: &str,
        agent_program: &str,
    ) -> AppResult<BrowserAgentIntegration> {
        validate_agent_id(agent_id)?;
        let launch = self.mcp_launch(app)?;
        let mut environment = base_environment(agent_id)?;
        let args_prefix = match agent_type {
            "claude" => claude_browser_args(app, &launch)?,
            "codex" => codex_browser_args(&launch)?,
            "pi" | "omp" => {
                environment.push(("SIKEMUX_TOOLS_MCP_COMMAND".into(), launch.command.clone()));
                environment.push((
                    "SIKEMUX_TOOLS_MCP_ARGS".into(),
                    serde_json::to_string(&launch.args)?,
                ));
                vec![
                    "--extension".into(),
                    pi_browser_extension(app)?.to_string_lossy().into_owned(),
                ]
            }
            "hermes" => {
                let home = prepare_hermes_home(app, agent_id, agent_program, &launch).await?;
                environment.push(("HERMES_HOME".into(), home.to_string_lossy().into_owned()));
                Vec::new()
            }
            "grok" => {
                let home = prepare_grok_home(app, agent_id, agent_program, &launch).await?;
                environment.push(("GROK_HOME".into(), home.to_string_lossy().into_owned()));
                Vec::new()
            }
            "opencode" => {
                environment.push((
                    "OPENCODE_CONFIG_CONTENT".into(),
                    opencode_browser_config(inherited_opencode_config().as_deref(), &launch)?,
                ));
                Vec::new()
            }
            _ => return Err(AppError::BadArg("unsupported browser agent type")),
        };
        Ok(BrowserAgentIntegration {
            args_prefix,
            environment,
        })
    }
}

/// An ACP agent is handed its servers over the protocol itself, so this path
/// has no config file and no flag. It is a separate transport from the
/// terminal one above and needs telling too.
pub fn acp_browser_server(app: &AppHandle, agent_id: &str) -> AppResult<McpServer> {
    validate_agent_id(agent_id)?;
    let launch = app.state::<BrowserManager>().mcp_launch(app)?;
    let mut server = McpServerStdio::new("sikemux-tools", absolute_command(&launch.command)?);
    server.args = launch.args;
    server.env = base_environment(agent_id)?
        .into_iter()
        .map(|(name, value)| EnvVariable::new(name, value))
        .collect();
    Ok(McpServer::Stdio(server))
}

/// The protocol asks for an absolute path, and a development launch runs
/// through a program found on PATH.
fn absolute_command(command: &str) -> AppResult<PathBuf> {
    let path = PathBuf::from(command);
    if path.is_absolute() {
        return Ok(path);
    }
    crate::system::find_executable(command)
        .ok_or_else(|| AppError::Other(format!("{command} is not on PATH")))
}

/// What the MCP sidecar reads once the host spawns it. The sidecar inherits
/// this through the agent process, so it never appears in a config file.
fn base_environment(agent_id: &str) -> AppResult<Vec<(String, String)>> {
    Ok(vec![
        ("SIKEMUX_TOOLS_AGENT_ID".into(), agent_id.to_owned()),
        (
            "SIKEMUX_CLI_ENDPOINT".into(),
            crate::cli_server::cli_endpoint_path()
                .ok_or_else(|| AppError::Other("CLI endpoint unavailable".into()))?
                .to_string_lossy()
                .into_owned(),
        ),
    ])
}

fn mcp_server_document(launch: &BrowserMcpLaunch) -> Value {
    json!({
        "type": "stdio",
        "command": launch.command,
        "args": launch.args,
    })
}

/// Claude Code merges `--mcp-config` with the servers the user already has,
/// so this adds the browser without hiding anything else. The flag takes a
/// list, so it is written as one `=` token: given a separate one it would
/// swallow whatever plain word came next.
fn claude_browser_args(app: &AppHandle, launch: &BrowserMcpLaunch) -> AppResult<Vec<String>> {
    let directory = browser_state_dir(app)?;
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("claude-mcp.json");
    let document = json!({ "mcpServers": { "sikemux-tools": mcp_server_document(launch) } });
    let temporary = directory.join(format!(".claude-mcp-{}.json", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&document)?)?;
    std::fs::rename(temporary, &path)?;
    Ok(vec![format!("--mcp-config={}", path.to_string_lossy())])
}

fn codex_browser_args(launch: &BrowserMcpLaunch) -> AppResult<Vec<String>> {
    let command = serde_json::to_string(&launch.command)?;
    let args = serde_json::to_string(&launch.args)?;
    Ok(vec![
        "-c".into(),
        format!("mcp_servers.sikemux_tools.command={command}"),
        "-c".into(),
        format!("mcp_servers.sikemux_tools.args={args}"),
    ])
}

fn inherited_opencode_config() -> Option<String> {
    crate::system::login_shell_environment()
        .get("OPENCODE_CONFIG_CONTENT")
        .cloned()
        .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok())
}

fn opencode_browser_config(existing: Option<&str>, launch: &BrowserMcpLaunch) -> AppResult<String> {
    let mut config = match existing.map(str::trim).filter(|value| !value.is_empty()) {
        Some(existing) => serde_json::from_str::<Value>(existing)
            .map_err(|_| AppError::BadArg("invalid OPENCODE_CONFIG_CONTENT"))?,
        None => json!({}),
    };
    let root = config
        .as_object_mut()
        .ok_or(AppError::BadArg("invalid OPENCODE_CONFIG_CONTENT"))?;
    let mcp = root
        .entry("mcp")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or(AppError::BadArg("invalid OpenCode MCP config"))?;
    let mut command = vec![launch.command.clone()];
    command.extend(launch.args.iter().cloned());
    mcp.insert(
        "sikemux_tools".into(),
        json!({ "type": "local", "command": command, "enabled": true }),
    );
    serde_json::to_string(&config).map_err(AppError::from)
}

fn pi_browser_extension(app: &AppHandle) -> AppResult<PathBuf> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let path = resource_dir.join("sikemux_pi_tools.ts");
        if path.is_file() {
            return Ok(path);
        }
    }
    #[cfg(debug_assertions)]
    {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("sikemux_pi_tools.ts");
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(AppError::Other(
        "bundled Pi browser extension is missing".into(),
    ))
}

async fn prepare_hermes_home(
    app: &AppHandle,
    agent_id: &str,
    agent_program: &str,
    launch: &BrowserMcpLaunch,
) -> AppResult<PathBuf> {
    let home = private_home(
        app,
        "hermes",
        agent_id,
        &agent_home("HERMES_HOME", ".hermes"),
        "config.yaml",
        "{}\n",
    )?;
    let server = json!({
        "command": launch.command,
        "args": launch.args,
        "enabled": true,
    })
    .to_string();
    run_config_command(
        agent_program,
        &[
            "config",
            "set",
            "mcp_servers.sikemux_tools",
            &server,
            "--force",
        ],
        ("HERMES_HOME", &home),
        "Hermes",
    )
    .await?;
    Ok(home)
}

async fn prepare_grok_home(
    app: &AppHandle,
    agent_id: &str,
    agent_program: &str,
    launch: &BrowserMcpLaunch,
) -> AppResult<PathBuf> {
    let home = private_home(
        app,
        "grok",
        agent_id,
        &agent_home("GROK_HOME", ".grok"),
        "config.toml",
        "",
    )?;
    let mut args = vec!["mcp", "add", "sikemux_tools", "--", launch.command.as_str()];
    args.extend(launch.args.iter().map(String::as_str));
    run_config_command(agent_program, &args, ("GROK_HOME", &home), "Grok").await?;
    Ok(home)
}

fn agent_home(variable: &str, default_name: &str) -> PathBuf {
    crate::system::login_shell_environment()
        .get(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::system::user_home().join(default_name))
}

/// A private copy of the host's home: every entry is linked back to the real
/// one, except the config file, which is copied so Sikemux can add a server to
/// it without touching the user's own.
fn private_home(
    app: &AppHandle,
    host: &str,
    agent_id: &str,
    source: &Path,
    config_name: &str,
    empty_config: &str,
) -> AppResult<PathBuf> {
    let destination = browser_state_dir(app)?.join(host).join(agent_id);
    std::fs::create_dir_all(&destination)?;
    match std::fs::read_dir(source) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if entry.file_name() == config_name {
                    continue;
                }
                link_entry(&entry.path(), &destination.join(entry.file_name()))?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let config = destination.join(config_name);
    let source_config = source.join(config_name);
    if source_config.is_file() {
        std::fs::copy(source_config, &config)?;
    } else if !config.is_file() {
        std::fs::write(&config, empty_config)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(destination)
}

fn link_entry(source: &Path, destination: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(destination).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination)
    }
    #[cfg(windows)]
    {
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(source, destination)
        } else {
            std::os::windows::fs::symlink_file(source, destination)
                .or_else(|_| std::fs::hard_link(source, destination))
                .or_else(|_| std::fs::copy(source, destination).map(|_| ()))
        }
    }
}

async fn run_config_command(
    program: &str,
    args: &[&str],
    home: (&str, &Path),
    label: &str,
) -> AppResult<()> {
    let output = tokio::time::timeout(
        CONFIG_TIMEOUT,
        tokio::process::Command::new(program)
            .args(args)
            .env(home.0, home.1)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| AppError::Other(format!("{label} browser config timed out")))?
    .map_err(|error| AppError::Other(format!("{label} browser config failed: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(512)
        .collect::<String>();
    Err(AppError::Other(format!(
        "{label} browser config failed: {}",
        detail.trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> BrowserMcpLaunch {
        BrowserMcpLaunch {
            command: "/Apps/Sikemux.app/sikemux-tools-mcp".into(),
            args: vec!["--stdio".into()],
        }
    }

    #[test]
    fn sikemux_options_land_before_the_launch_the_pane_built() {
        let integration = BrowserAgentIntegration {
            args_prefix: vec!["--mcp-config=/state/claude-mcp.json".into()],
            environment: vec![("SIKEMUX_TOOLS_AGENT_ID".into(), "agent-one".into())],
        };
        let mut args = vec![
            "--model".to_string(),
            "opus".into(),
            "--resume".into(),
            "abc".into(),
        ];
        let environment = integration.apply(&mut args);
        assert_eq!(
            args,
            vec![
                "--mcp-config=/state/claude-mcp.json",
                "--model",
                "opus",
                "--resume",
                "abc"
            ]
        );
        assert_eq!(environment.len(), 1);
    }

    #[test]
    fn a_host_launched_with_no_arguments_still_gets_told() {
        let integration = BrowserAgentIntegration {
            args_prefix: vec![
                "-c".into(),
                "mcp_servers.sikemux_tools.command=\"x\"".into(),
            ],
            environment: Vec::new(),
        };
        let mut args = Vec::new();
        integration.apply(&mut args);
        assert_eq!(args.len(), 2);
    }

    /// An ACP adapter tells a stdio server apart by the absence of a type
    /// tag, so the shape matters as much as the values.
    #[test]
    fn an_acp_stdio_server_is_untagged_with_its_environment_spelled_out() {
        let mut stdio = McpServerStdio::new("sikemux-tools", "/apps/sikemux-tools-mcp");
        stdio.args = vec!["--stdio".into()];
        stdio.env = vec![EnvVariable::new("SIKEMUX_TOOLS_AGENT_ID", "agent-one")];
        let value = serde_json::to_value(McpServer::Stdio(stdio)).unwrap();
        assert!(value.get("type").is_none(), "{value}");
        assert_eq!(value["name"], "sikemux-tools");
        assert_eq!(value["command"], "/apps/sikemux-tools-mcp");
        assert_eq!(value["args"], json!(["--stdio"]));
        assert_eq!(value["env"][0]["name"], "SIKEMUX_TOOLS_AGENT_ID");
        assert_eq!(value["env"][0]["value"], "agent-one");
    }

    #[test]
    fn an_acp_session_is_given_an_absolute_command_even_in_development() {
        assert_eq!(
            absolute_command("/bin/sh").unwrap(),
            PathBuf::from("/bin/sh")
        );
        let found = absolute_command("sh").unwrap();
        assert!(found.is_absolute(), "{found:?} should be absolute");
        assert!(absolute_command("sikemux-no-such-program").is_err());
    }

    #[test]
    fn every_supported_host_is_one_the_pane_can_launch() {
        for agent_type in SUPPORTED_AGENTS {
            assert!(is_supported(agent_type));
        }
        assert!(!is_supported("shell"));
        assert!(!is_supported(""));
    }

    #[test]
    fn codex_learns_the_server_through_dotted_overrides() {
        assert_eq!(
            codex_browser_args(&launch()).unwrap(),
            vec![
                "-c".to_string(),
                "mcp_servers.sikemux_tools.command=\"/Apps/Sikemux.app/sikemux-tools-mcp\""
                    .to_string(),
                "-c".to_string(),
                "mcp_servers.sikemux_tools.args=[\"--stdio\"]".to_string(),
            ]
        );
    }

    #[test]
    fn claude_config_rides_on_one_token_so_the_next_argument_survives() {
        let path = Path::new("/state/claude-mcp.json");
        assert_eq!(
            vec![format!("--mcp-config={}", path.to_string_lossy())],
            vec!["--mcp-config=/state/claude-mcp.json".to_string()]
        );
    }

    #[test]
    fn claude_is_handed_a_stdio_server_named_for_sikemux() {
        let document = json!({ "mcpServers": { "sikemux-tools": mcp_server_document(&launch()) } });
        let server = &document["mcpServers"]["sikemux-tools"];
        assert_eq!(server["type"], "stdio");
        assert_eq!(server["command"], "/Apps/Sikemux.app/sikemux-tools-mcp");
        assert_eq!(server["args"], json!(["--stdio"]));
    }

    #[test]
    fn opencode_keeps_the_settings_it_already_had() {
        let existing = r#"{"theme":"dark","mcp":{"other":{"type":"local"}}}"#;
        let merged: Value =
            serde_json::from_str(&opencode_browser_config(Some(existing), &launch()).unwrap())
                .unwrap();
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["mcp"]["other"]["type"], "local");
        assert_eq!(
            merged["mcp"]["sikemux_tools"]["command"],
            json!(["/Apps/Sikemux.app/sikemux-tools-mcp", "--stdio"])
        );
        assert_eq!(merged["mcp"]["sikemux_tools"]["enabled"], true);
    }

    #[test]
    fn opencode_starts_from_nothing_and_refuses_junk() {
        let fresh: Value =
            serde_json::from_str(&opencode_browser_config(None, &launch()).unwrap()).unwrap();
        assert!(fresh["mcp"]["sikemux_tools"].is_object());
        assert!(opencode_browser_config(Some("   "), &launch()).is_ok());
        assert!(opencode_browser_config(Some("not json"), &launch()).is_err());
        assert!(opencode_browser_config(Some("[1,2]"), &launch()).is_err());
    }
}
