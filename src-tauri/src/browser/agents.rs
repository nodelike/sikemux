//! Every agent host learns about the browser tools a different way: a config
//! file named on the command line, a dotted override, an extension, or its own
//! home directory. This is that per-host knowledge in one place.
//!
//! A host's existing MCP servers are never disturbed. Each integration either
//! adds to what the user already configured or works on a private copy of the
//! host's home, so an agent launched outside Sikemux is unaffected.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_core::acp::schema::v1::{EnvVariable, McpServer, McpServerStdio};
use sikemux_core::launch::{homes, skills};
use tauri::{AppHandle, Manager};

use super::agent_skills;
use super::{browser_state_dir, validate_agent_id, BrowserManager, BrowserMcpLaunch};
use crate::error::{AppError, AppResult};

const CONFIG_TIMEOUT: Duration = Duration::from_secs(5);

/// The hosts that can be told about the browser tools. Anything else launches
/// untouched.
pub fn is_supported(agent_type: &str) -> bool {
    crate::agents::AgentKind::from_name(agent_type).is_some()
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
        let launch = self.mcp_launch()?;
        let mut environment = base_environment(agent_id)?;
        let data_dir = skills_data_dir(app);
        let data_dir = data_dir.as_deref();
        let args_prefix = match agent_type {
            "claude" => {
                let mut args = claude_browser_args(app, &launch)?;
                args.extend(skills::skill_args(data_dir, skills::claude_args));
                args
            }
            "codex" => codex_browser_args(&launch)?,
            "pi" | "omp" => {
                environment.push(("SIKEMUX_TOOLS_MCP_COMMAND".into(), launch.command.clone()));
                environment.push((
                    "SIKEMUX_TOOLS_MCP_ARGS".into(),
                    serde_json::to_string(&launch.args)?,
                ));
                let mut args = vec![
                    "--extension".into(),
                    pi_browser_extension(app)?.to_string_lossy().into_owned(),
                ];
                args.extend(if agent_type == "pi" {
                    skills::skill_args(data_dir, |dir| Ok(skills::pi_args(dir)))
                } else {
                    skills::skill_args(data_dir, skills::omp_args)
                });
                args
            }
            "hermes" => {
                let home =
                    prepare_hermes_home(app, agent_id, agent_program, &launch, data_dir).await?;
                environment.push(("HERMES_HOME".into(), home.to_string_lossy().into_owned()));
                Vec::new()
            }
            "grok" => {
                let home =
                    prepare_grok_home(app, agent_id, agent_program, &launch, data_dir).await?;
                environment.push(("GROK_HOME".into(), home.to_string_lossy().into_owned()));
                Vec::new()
            }
            "muse" => muse_browser_args(&launch, &environment)?,
            "opencode" => {
                environment.push((
                    "OPENCODE_CONFIG_CONTENT".into(),
                    opencode_browser_config(
                        skills::inherited_opencode_config().as_deref(),
                        &launch,
                        data_dir.map(|dir| {
                            (
                                skills::opencode_user_paths(),
                                sikemux_core::skills::library(dir),
                            )
                        }),
                    )?,
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
    let launch = app.state::<BrowserManager>().mcp_launch()?;
    let mut server = McpServerStdio::new("sikemux-tools", absolute_command(&launch.command)?);
    server.args = launch.args;
    server.env = base_environment(agent_id)?
        .into_iter()
        .map(|(name, value)| EnvVariable::new(name, value))
        .collect();
    Ok(McpServer::Stdio(server))
}

/// Gives a Hermes or Grok chat a private home holding the skills library,
/// since the tools reach them over the protocol rather than their config.
pub async fn chat_home(
    app: &AppHandle,
    agent_id: &str,
    provider: &str,
    program: &Path,
    env: &mut BTreeMap<String, String>,
) {
    let Some(data_dir) = skills_data_dir(app) else {
        return;
    };
    let library = sikemux_core::skills::library(&data_dir);
    match provider {
        "hermes" => {
            match chat_hermes_home(app, agent_id, &program.to_string_lossy(), &library).await {
                Ok(home) => {
                    env.insert("HERMES_HOME".into(), home.to_string_lossy().into_owned());
                }
                Err(error) => {
                    eprintln!("Sikemux skills are unavailable to this Hermes chat: {error}")
                }
            }
        }
        "grok" => match chat_grok_home(app, agent_id, &library) {
            Ok(home) => {
                env.insert("GROK_HOME".into(), home.to_string_lossy().into_owned());
            }
            Err(error) => eprintln!("Sikemux skills are unavailable to this Grok chat: {error}"),
        },
        _ => {}
    }
}

async fn chat_hermes_home(
    app: &AppHandle,
    agent_id: &str,
    program: &str,
    library: &Path,
) -> AppResult<PathBuf> {
    let real = crate::agents::config::hermes_root()
        .ok_or_else(|| AppError::Other("the home folder is unavailable".into()))?;
    homes::share_session_store(&real, "state.db", false)?;
    let home = private_home(app, "hermes-chat", agent_id, &real, "config.yaml", "{}\n")?;
    let config = std::fs::read_to_string(home.join("config.yaml")).unwrap_or_default();
    let dirs = agent_skills::hermes_external_dirs(&config, library);
    run_config_command(
        program,
        &["config", "set", "skills.external_dirs", &dirs, "--force"],
        ("HERMES_HOME", &home),
        "Hermes",
    )
    .await?;
    Ok(home)
}

fn chat_grok_home(app: &AppHandle, agent_id: &str, library: &Path) -> AppResult<PathBuf> {
    let real = crate::agents::config::grok_root()
        .ok_or_else(|| AppError::Other("the home folder is unavailable".into()))?;
    homes::share_session_store(&real, "sessions", true)?;
    let home = private_home(app, "grok-chat", agent_id, &real, "config.toml", "")?;
    let path = home.join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap_or_default();
    let merged = agent_skills::grok_config(&config, library).map_err(AppError::Other)?;
    std::fs::write(&path, merged)?;
    Ok(home)
}

/// The app data folder with the skills library ready in it. Without one,
/// agents launch without Sikemux skills rather than pointed at a missing folder.
pub fn skills_data_dir(app: &AppHandle) -> Option<PathBuf> {
    let data_dir = app.path().app_data_dir().ok()?;
    skills::ready(&data_dir)?;
    Some(data_dir)
}

/// The protocol asks for an absolute path, and a development launch runs
/// through a program found on PATH.
fn absolute_command(command: &str) -> AppResult<PathBuf> {
    let path = PathBuf::from(command);
    if path.is_absolute() {
        return Ok(path);
    }
    sikemux_process::user_environment::find_executable(command)
        .ok_or_else(|| AppError::Other(format!("{command} is not on PATH")))
}

/// What the MCP sidecar reads once the host spawns it. The sidecar inherits
/// this through the agent process, so it never appears in a config file.
fn base_environment(agent_id: &str) -> AppResult<Vec<(String, String)>> {
    let environment = vec![
        ("SIKEMUX_TOOLS_AGENT_ID".into(), agent_id.to_owned()),
        (
            "SIKEMUX_CLI_ENDPOINT".into(),
            crate::cli_paths::cli_endpoint_path()
                .ok_or_else(|| AppError::Other("CLI endpoint unavailable".into()))?
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    Ok(environment)
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
    sikemux_process::files::write_atomic(&path, &serde_json::to_vec_pretty(&document)?, false)?;
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

/// Muse adds one named server from a `-c` override to the ones it already
/// has. It starts tool servers with only the environment it is given, so the
/// sidecar's is named in the server itself.
fn muse_browser_args(
    launch: &BrowserMcpLaunch,
    environment: &[(String, String)],
) -> AppResult<Vec<String>> {
    let server = json!({
        "transport": "stdio",
        "command": launch.command,
        "args": launch.args,
        "env": environment
            .iter()
            .map(|(name, value)| (name.clone(), Value::from(value.as_str())))
            .collect::<serde_json::Map<String, Value>>(),
        "mode": "optional",
    });
    Ok(vec![
        "-c".into(),
        format!(
            "mcpServers.sikemux-tools={}",
            serde_json::to_string(&server)?
        ),
    ])
}

/// Skills arguments for one launch. A library that cannot be set up costs the
/// agent its Sikemux skills, never its launch.
fn opencode_browser_config(
    existing: Option<&str>,
    launch: &BrowserMcpLaunch,
    skills: Option<(Vec<String>, PathBuf)>,
) -> AppResult<String> {
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
    if let Some((own_paths, library)) = skills {
        skills::add_opencode_paths(&mut config, &own_paths, &library);
    }
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
    data_dir: Option<&Path>,
) -> AppResult<PathBuf> {
    let home = private_home(
        app,
        "hermes",
        agent_id,
        &crate::agents::config::hermes_root()
            .ok_or_else(|| AppError::Other("the home folder is unavailable".into()))?,
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
    if let Some(data_dir) = data_dir {
        let config = std::fs::read_to_string(home.join("config.yaml")).unwrap_or_default();
        let dirs =
            agent_skills::hermes_external_dirs(&config, &sikemux_core::skills::library(data_dir));
        if let Err(error) = run_config_command(
            agent_program,
            &["config", "set", "skills.external_dirs", &dirs, "--force"],
            ("HERMES_HOME", &home),
            "Hermes",
        )
        .await
        {
            eprintln!("Sikemux skills are unavailable to Hermes: {error}");
        }
    }
    Ok(home)
}

async fn prepare_grok_home(
    app: &AppHandle,
    agent_id: &str,
    agent_program: &str,
    launch: &BrowserMcpLaunch,
    data_dir: Option<&Path>,
) -> AppResult<PathBuf> {
    let home = private_home(
        app,
        "grok",
        agent_id,
        &crate::agents::config::grok_root()
            .ok_or_else(|| AppError::Other("the home folder is unavailable".into()))?,
        "config.toml",
        "",
    )?;
    let mut args = vec!["mcp", "add", "sikemux_tools", "--", launch.command.as_str()];
    args.extend(launch.args.iter().map(String::as_str));
    run_config_command(agent_program, &args, ("GROK_HOME", &home), "Grok").await?;
    if let Some(data_dir) = data_dir {
        let path = home.join("config.toml");
        let config = std::fs::read_to_string(&path).unwrap_or_default();
        match agent_skills::grok_config(&config, &sikemux_core::skills::library(data_dir)) {
            Ok(config) => std::fs::write(&path, config)?,
            Err(error) => eprintln!("Sikemux skills are unavailable to Grok: {error}"),
        }
    }
    Ok(home)
}

/// A private copy of the host's home for `agent_id`, which Sikemux can add a
/// server to without touching the user's own config.
fn private_home(
    app: &AppHandle,
    host: &str,
    agent_id: &str,
    source: &Path,
    config_name: &str,
    empty_config: &str,
) -> AppResult<PathBuf> {
    let destination = browser_state_dir(app)?.join(host).join(agent_id);
    homes::private_home(&destination, source, config_name, empty_config)?;
    Ok(destination)
}

async fn run_config_command(
    program: &str,
    args: &[&str],
    home: (&str, &Path),
    label: &str,
) -> AppResult<()> {
    let output = tokio::time::timeout(
        CONFIG_TIMEOUT,
        tokio::process::Command::from(sikemux_process::user_environment::command(program))
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
            command: "/Apps/Sikemux.app/sikemux-editor".into(),
            args: vec!["--tools-mcp".into()],
        }
    }

    #[test]
    fn opencode_adds_the_skills_library_beside_the_tools() {
        let library = PathBuf::from("/data/skills");
        let config: Value = serde_json::from_str(
            &opencode_browser_config(
                Some(r#"{"skills":{"paths":["/inherited"]}}"#),
                &launch(),
                Some((vec!["/own".into()], library)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            config["skills"]["paths"],
            json!(["/inherited", "/own", "/data/skills"])
        );
        assert!(config["mcp"]["sikemux_tools"].is_object());
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

    /// An ACP adapter tells a stdio server apart by the absence of a type
    /// tag, so the shape matters as much as the values.
    #[test]
    fn an_acp_stdio_server_is_untagged_with_its_environment_spelled_out() {
        let mut stdio = McpServerStdio::new("sikemux-tools", "/apps/sikemux-editor");
        stdio.args = vec!["--tools-mcp".into()];
        stdio.env = vec![EnvVariable::new("SIKEMUX_TOOLS_AGENT_ID", "agent-one")];
        let value = serde_json::to_value(McpServer::Stdio(stdio)).unwrap();
        assert!(value.get("type").is_none(), "{value}");
        assert_eq!(value["name"], "sikemux-tools");
        assert_eq!(value["command"], "/apps/sikemux-editor");
        assert_eq!(value["args"], json!(["--tools-mcp"]));
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
    fn codex_learns_the_server_through_dotted_overrides() {
        assert_eq!(
            codex_browser_args(&launch()).unwrap(),
            vec![
                "-c".to_string(),
                "mcp_servers.sikemux_tools.command=\"/Apps/Sikemux.app/sikemux-editor\""
                    .to_string(),
                "-c".to_string(),
                "mcp_servers.sikemux_tools.args=[\"--tools-mcp\"]".to_string(),
            ]
        );
    }

    #[test]
    fn claude_is_handed_a_stdio_server_named_for_sikemux() {
        let document = json!({ "mcpServers": { "sikemux-tools": mcp_server_document(&launch()) } });
        let server = &document["mcpServers"]["sikemux-tools"];
        assert_eq!(server["type"], "stdio");
        assert_eq!(server["command"], "/Apps/Sikemux.app/sikemux-editor");
        assert_eq!(server["args"], json!(["--tools-mcp"]));
    }

    #[test]
    fn opencode_keeps_the_settings_it_already_had() {
        let existing = r#"{"theme":"dark","mcp":{"other":{"type":"local"}}}"#;
        let merged: Value = serde_json::from_str(
            &opencode_browser_config(Some(existing), &launch(), None).unwrap(),
        )
        .unwrap();
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["mcp"]["other"]["type"], "local");
        assert_eq!(
            merged["mcp"]["sikemux_tools"]["command"],
            json!(["/Apps/Sikemux.app/sikemux-editor", "--tools-mcp"])
        );
        assert_eq!(merged["mcp"]["sikemux_tools"]["enabled"], true);
    }

    #[test]
    fn opencode_starts_from_nothing_and_refuses_junk() {
        let fresh: Value =
            serde_json::from_str(&opencode_browser_config(None, &launch(), None).unwrap()).unwrap();
        assert!(fresh["mcp"]["sikemux_tools"].is_object());
        assert!(opencode_browser_config(Some("   "), &launch(), None).is_ok());
        assert!(opencode_browser_config(Some("not json"), &launch(), None).is_err());
        assert!(opencode_browser_config(Some("[1,2]"), &launch(), None).is_err());
    }
}
