//! Every agent learns about Sikemux's tools a different way: a config file
//! named on the command line, a dotted override, an extension, its own home
//! directory, or for a chat the protocol itself. These are those ways; each
//! agent's `AgentOps` picks its own.
//!
//! An agent's existing MCP servers are never disturbed. Each integration either
//! adds to what the person already configured or works on a private copy of the
//! agent's home, so an agent launched outside Sikemux is unaffected.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::vars;

use crate::acp::schema::v1::{EnvVariable, McpServer, McpServerStdio};
use crate::agent_catalog::AgentKind;
use crate::cli::protocol::{is_tool_agent_id, TOOLS_OFFER_ENV};
use crate::launch::{homes, skills};

const CONFIG_TIMEOUT: Duration = Duration::from_secs(5);
const TOOLS_MCP_FLAG: &str = "--tools-mcp";
/// The tools' server name, and the one for agents whose config keys take no hyphen.
pub(crate) const TOOLS_SERVER: &str = "sikemux-tools";
const TOOLS_KEY: &str = "sikemux_tools";

/// Where an agent's Sikemux tools run from, and where Sikemux keeps what it
/// gives each agent.
pub struct Tools {
    /// The Sikemux program, run as the tools' MCP server.
    pub executable: PathBuf,
    /// Where the tools reach the core.
    pub endpoint: PathBuf,
    /// The data folder, which holds the skills library and the agents'
    /// private homes; without one agents get only the tools.
    pub data_dir: Option<PathBuf>,
}

/// What a terminal agent starts with: arguments in front of the ones its
/// launch already has, and environment for its process.
#[derive(Debug, Default)]
pub struct TerminalTools {
    pub args_prefix: Vec<String>,
    pub environment: Vec<(String, String)>,
}

impl TerminalTools {
    /// Folds these arguments into `args`, and hands back the environment.
    /// Sikemux's go first: an agent that takes a subcommand wants its options
    /// before it.
    pub fn apply(mut self, args: &mut Vec<String>) -> Vec<(String, String)> {
        self.args_prefix.append(args);
        *args = self.args_prefix;
        self.environment
    }
}

pub(crate) struct McpLaunch {
    command: String,
    args: Vec<String>,
}

/// One terminal agent's launch while its tools are set up: what the tools' server
/// is started with, and the environment its process gets.
pub(crate) struct TerminalSetup<'a> {
    pub(crate) tools: &'a Tools,
    pub(crate) agent_id: &'a str,
    pub(crate) program: &'a str,
    pub(crate) launch: McpLaunch,
    pub(crate) environment: Vec<(String, String)>,
    /// The data folder with the skills library ready in it.
    pub(crate) data_dir: Option<&'a Path>,
    pub(crate) library: Option<PathBuf>,
}

impl TerminalSetup<'_> {
    /// Claude Code merges `--mcp-config` with the servers the person already
    /// has, so this adds the tools without hiding anything else.
    pub(crate) fn claude_args(&self) -> Result<Vec<String>, String> {
        let mut args = self.tools.claude_args(&self.launch)?;
        args.extend(skills::skill_args(self.data_dir, skills::claude_args));
        Ok(args)
    }

    pub(crate) fn codex_args(&self) -> Result<Vec<String>, String> {
        codex_args(&self.launch)
    }

    /// Pi and OMP load the tools through Sikemux's extension, which reads the
    /// server from the environment, beside the skill arguments `skills` makes.
    pub(crate) fn extension_args(
        &mut self,
        skills: impl FnOnce(Option<&Path>) -> Vec<String>,
    ) -> Result<Vec<String>, String> {
        self.environment
            .push((vars::TOOLS_MCP_COMMAND.into(), self.launch.command.clone()));
        self.environment.push((
            vars::TOOLS_MCP_ARGS.into(),
            serde_json::to_string(&self.launch.args).map_err(|error| error.to_string())?,
        ));
        let mut args = vec![
            "--extension".into(),
            self.tools.pi_extension()?.to_string_lossy().into_owned(),
        ];
        args.extend(skills(self.data_dir));
        Ok(args)
    }

    pub(crate) async fn hermes_home(&mut self) -> Result<Vec<String>, String> {
        let home = self
            .tools
            .hermes_home(
                "hermes",
                self.agent_id,
                self.program,
                Some(&self.launch),
                self.library.as_deref(),
            )
            .await?;
        self.environment.push(home);
        Ok(Vec::new())
    }

    pub(crate) async fn grok_home(&mut self) -> Result<Vec<String>, String> {
        let home = self
            .tools
            .grok_home(
                "grok",
                self.agent_id,
                self.program,
                Some(&self.launch),
                self.library.as_deref(),
            )
            .await?;
        self.environment.push(home);
        Ok(Vec::new())
    }

    pub(crate) fn muse_args(&self) -> Result<Vec<String>, String> {
        muse_args(&self.launch, &self.environment)
    }

    pub(crate) fn opencode_config(&mut self) -> Result<Vec<String>, String> {
        let config = opencode_config(
            skills::inherited_opencode_config().as_deref(),
            &self.launch,
            self.library
                .clone()
                .map(|library| (skills::opencode_user_paths(), library)),
        )?;
        self.environment
            .push(("OPENCODE_CONFIG_CONTENT".into(), config));
        Ok(Vec::new())
    }
}

impl Tools {
    fn mcp(&self) -> McpLaunch {
        McpLaunch {
            command: self.executable.to_string_lossy().into_owned(),
            args: vec![TOOLS_MCP_FLAG.into()],
        }
    }

    fn state_dir(&self) -> Result<PathBuf, String> {
        let data_dir = self
            .data_dir
            .as_ref()
            .ok_or("Sikemux keeps no data folder here")?;
        Ok(data_dir.join("browser"))
    }

    /// The data folder with the skills library ready in it. Without one,
    /// agents launch without Sikemux skills rather than pointed at a missing folder.
    fn skills_dir(&self) -> Option<&Path> {
        skills::ready(self.data_dir.as_deref()?)
    }

    /// What `agent_type` running `agent_program` in a terminal needs to reach
    /// the tools. Agents Sikemux does not know launch untouched.
    pub async fn terminal(
        &self,
        agent_id: &str,
        agent_type: &str,
        agent_program: &str,
    ) -> Result<TerminalTools, String> {
        let Some(kind) = AgentKind::from_name(agent_type) else {
            return Ok(TerminalTools::default());
        };
        valid_agent_id(agent_id)?;
        let data_dir = self.skills_dir();
        let mut setup = TerminalSetup {
            tools: self,
            agent_id,
            program: agent_program,
            launch: self.mcp(),
            environment: self.base_environment(agent_id),
            data_dir,
            library: data_dir.map(crate::skills::library),
        };
        let args_prefix = kind.ops().terminal_tools(&mut setup).await?;
        Ok(TerminalTools {
            args_prefix,
            environment: setup.environment,
        })
    }

    /// The tools as a chat is handed them, over the protocol itself, with the
    /// tools only `offer` names.
    pub fn chat_server(&self, agent_id: &str, offer: Option<&str>) -> Result<Value, String> {
        valid_agent_id(agent_id)?;
        let mut server = McpServerStdio::new(TOOLS_SERVER, absolute_command(&self.executable)?);
        server.args = vec![TOOLS_MCP_FLAG.into()];
        server.env = self
            .base_environment(agent_id)
            .into_iter()
            .chain(offer.map(|offer| (TOOLS_OFFER_ENV.to_owned(), offer.to_owned())))
            .map(|(name, value)| EnvVariable::new(name, value))
            .collect();
        serde_json::to_value(McpServer::Stdio(server)).map_err(|error| error.to_string())
    }

    /// Gives a chat of an agent that takes the skills library in a home of its own
    /// a private home holding it, since the tools reach it over the protocol.
    pub async fn chat_home(
        &self,
        agent_id: &str,
        provider: &str,
        program: &Path,
        env: &mut BTreeMap<String, String>,
    ) {
        let Some(library) = self.skills_dir().map(crate::skills::library) else {
            return;
        };
        let program = program.to_string_lossy();
        let Some(home) = AgentKind::from_name(provider)
            .and_then(|kind| kind.ops().chat_home(self, agent_id, &program, &library))
        else {
            return;
        };
        match home.await {
            Ok((variable, home)) => {
                env.insert(variable, home);
            }
            Err(error) => {
                eprintln!("Sikemux skills are unavailable to this {provider} chat: {error}")
            }
        }
    }

    /// What the MCP server reads once the agent starts it. It inherits this
    /// through the agent's process, so it never appears in a config file.
    fn base_environment(&self, agent_id: &str) -> Vec<(String, String)> {
        vec![
            (vars::TOOLS_AGENT_ID.into(), agent_id.to_owned()),
            (
                vars::CLI_ENDPOINT.into(),
                self.endpoint.to_string_lossy().into_owned(),
            ),
        ]
    }

    /// Claude Code's `--mcp-config` takes a list, so it is written as one `=` token:
    /// given a separate one it would swallow whatever plain word came next.
    fn claude_args(&self, launch: &McpLaunch) -> Result<Vec<String>, String> {
        let directory = self.state_dir()?;
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let path = directory.join("claude-mcp.json");
        let document = json!({ "mcpServers": { TOOLS_SERVER: mcp_server_document(launch) } });
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        sikemux_process::files::write_atomic(&path, &bytes, false)
            .map_err(|error| error.to_string())?;
        Ok(vec![format!("--mcp-config={}", path.to_string_lossy())])
    }

    /// Pi's extension ships beside the Sikemux program: in the app bundle's
    /// resources, or the source tree's in a development build.
    fn pi_extension(&self) -> Result<PathBuf, String> {
        const NAME: &str = "sikemux_pi_tools.ts";
        let beside = self
            .executable
            .parent()
            .map(|folder| [folder.join("../Resources").join(NAME), folder.join(NAME)]);
        let development = cfg!(debug_assertions).then(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join(NAME)
        });
        beside
            .into_iter()
            .flatten()
            .chain(development)
            .find(|path| path.is_file())
            .ok_or_else(|| "the Pi tools extension is missing".to_owned())
    }

    /// A private Hermes home for `agent_id`, kept under `host`, given the tools
    /// when `tools` names them and the skills `library`.
    pub(crate) async fn hermes_home(
        &self,
        host: &str,
        agent_id: &str,
        program: &str,
        tools: Option<&McpLaunch>,
        library: Option<&Path>,
    ) -> Result<(String, String), String> {
        let (variable, home) = self.private_home(AgentKind::Hermes, host, agent_id)?;
        let set = |key: &str, value: &str| {
            let args = ["config", "set", key, value, "--force"].map(str::to_owned);
            run_config_command(program, args.to_vec(), (variable, home.clone()), "Hermes")
        };
        if let Some(launch) = tools {
            let server = json!({
                "command": launch.command,
                "args": launch.args,
                "enabled": true,
            })
            .to_string();
            set(&format!("mcp_servers.{TOOLS_KEY}"), &server).await?;
        }
        if let Some(library) = library {
            let yaml = std::fs::read_to_string(home.join("config.yaml")).unwrap_or_default();
            let dirs = hermes_external_dirs(&yaml, library);
            if let Err(error) = set("skills.external_dirs", &dirs).await {
                eprintln!("Sikemux skills are unavailable to Hermes: {error}");
            }
        }
        Ok((variable.to_owned(), home.to_string_lossy().into_owned()))
    }

    /// A private Grok home for `agent_id`, kept under `host`, given the tools
    /// when `tools` names them and the skills `library`.
    pub(crate) async fn grok_home(
        &self,
        host: &str,
        agent_id: &str,
        program: &str,
        tools: Option<&McpLaunch>,
        library: Option<&Path>,
    ) -> Result<(String, String), String> {
        let (variable, home) = self.private_home(AgentKind::Grok, host, agent_id)?;
        if let Some(launch) = tools {
            let mut args = ["mcp", "add", TOOLS_KEY, "--"].map(str::to_owned).to_vec();
            args.push(launch.command.clone());
            args.extend(launch.args.iter().cloned());
            run_config_command(program, args, (variable, home.clone()), "Grok").await?;
        }
        if let Some(library) = library {
            let path = home.join("config.toml");
            let config = std::fs::read_to_string(&path).unwrap_or_default();
            match grok_config(&config, library) {
                Ok(config) => std::fs::write(&path, config).map_err(|error| error.to_string())?,
                Err(error) => eprintln!("Sikemux skills are unavailable to Grok: {error}"),
            }
        }
        Ok((variable.to_owned(), home.to_string_lossy().into_owned()))
    }

    /// A private copy of `kind`'s home for `agent_id`, which Sikemux can add a
    /// server to without touching the person's own config, and the variable
    /// that names it.
    fn private_home(
        &self,
        kind: AgentKind,
        host: &str,
        agent_id: &str,
    ) -> Result<(&'static str, PathBuf), String> {
        let (home, _) = homes::private(kind).ok_or("the agent keeps no private home")?;
        let destination = self.state_dir()?.join(host).join(agent_id);
        homes::private_home(kind, &destination, None)?;
        Ok((home.variable, destination))
    }
}

fn valid_agent_id(agent_id: &str) -> Result<(), String> {
    if is_tool_agent_id(agent_id) {
        Ok(())
    } else {
        Err("invalid agent id".into())
    }
}

/// The protocol asks for an absolute path, and a development launch runs
/// through a program found on PATH.
fn absolute_command(command: &Path) -> Result<PathBuf, String> {
    if command.is_absolute() {
        return Ok(command.to_path_buf());
    }
    let name = command.to_string_lossy();
    sikemux_process::user_environment::find_executable(&name)
        .ok_or_else(|| format!("{name} is not on PATH"))
}

fn mcp_server_document(launch: &McpLaunch) -> Value {
    json!({
        "type": "stdio",
        "command": launch.command,
        "args": launch.args,
    })
}

fn codex_args(launch: &McpLaunch) -> Result<Vec<String>, String> {
    let command = serde_json::to_string(&launch.command).map_err(|error| error.to_string())?;
    let args = serde_json::to_string(&launch.args).map_err(|error| error.to_string())?;
    Ok(vec![
        "-c".into(),
        format!("mcp_servers.{TOOLS_KEY}.command={command}"),
        "-c".into(),
        format!("mcp_servers.{TOOLS_KEY}.args={args}"),
    ])
}

/// Muse adds one named server from a `-c` override to the ones it already
/// has. It starts tool servers with only the environment it is given, so the
/// server's is named in the server itself.
fn muse_args(launch: &McpLaunch, environment: &[(String, String)]) -> Result<Vec<String>, String> {
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
    let server = serde_json::to_string(&server).map_err(|error| error.to_string())?;
    Ok(vec![
        "-c".into(),
        format!("mcpServers.{TOOLS_SERVER}={server}"),
    ])
}

fn opencode_config(
    existing: Option<&str>,
    launch: &McpLaunch,
    skills: Option<(Vec<String>, PathBuf)>,
) -> Result<String, String> {
    let mut config = match existing.map(str::trim).filter(|value| !value.is_empty()) {
        Some(existing) => serde_json::from_str::<Value>(existing)
            .map_err(|_| "invalid OPENCODE_CONFIG_CONTENT".to_owned())?,
        None => json!({}),
    };
    let root = config
        .as_object_mut()
        .ok_or("invalid OPENCODE_CONFIG_CONTENT")?;
    let mcp = root
        .entry("mcp")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("invalid OpenCode MCP config")?;
    let mut command = vec![launch.command.clone()];
    command.extend(launch.args.iter().cloned());
    mcp.insert(
        TOOLS_KEY.into(),
        json!({ "type": "local", "command": command, "enabled": true }),
    );
    if let Some((own_paths, library)) = skills {
        skills::add_opencode_paths(&mut config, &own_paths, &library);
    }
    serde_json::to_string(&config).map_err(|error| error.to_string())
}

async fn run_config_command(
    program: &str,
    args: Vec<String>,
    home: (&str, PathBuf),
    label: &str,
) -> Result<(), String> {
    let output = tokio::time::timeout(
        CONFIG_TIMEOUT,
        tokio::process::Command::from(sikemux_process::user_environment::agent_command(program))
            .args(args)
            .env(home.0, home.1)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| format!("{label} tools config timed out"))?
    .map_err(|error| format!("{label} tools config failed: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(512)
        .collect::<String>();
    Err(format!("{label} tools config failed: {}", detail.trim()))
}

/// The `skills.external_dirs` a Hermes `config.yaml` already lists, with the
/// library added, ready for `hermes config set`.
fn hermes_external_dirs(config_yaml: &str, library: &Path) -> String {
    let mut dirs = yaml_list(config_yaml, "skills", "external_dirs");
    let library = library.to_string_lossy().into_owned();
    if !dirs.contains(&library) {
        dirs.push(library);
    }
    json!(dirs).to_string()
}

/// Reads `parent: { key: [...] }` from YAML in its block or flow form. Hermes
/// writes this file itself, so these two forms cover it.
fn yaml_list(text: &str, parent: &str, key: &str) -> Vec<String> {
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim_end() != format!("{parent}:"));
    if lines.next().is_none() {
        return Vec::new();
    }
    let mut inside = lines.take_while(|line| line.is_empty() || line.starts_with(' '));
    let Some(line) = inside.find(|line| line.trim_start().starts_with(&format!("{key}:"))) else {
        return Vec::new();
    };
    let rest = line
        .trim_start()
        .trim_start_matches(&format!("{key}:"))
        .trim();
    let unquote = |item: &str| item.trim().trim_matches('"').trim_matches('\'').to_string();
    if let Some(flow) = rest
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return flow
            .split(',')
            .map(unquote)
            .filter(|item| !item.is_empty())
            .collect();
    }
    inside
        .map(str::trim)
        .take_while(|line| line.starts_with("- "))
        .filter_map(|line| line.strip_prefix("- "))
        .map(unquote)
        .collect()
}

/// Grok's copied `config.toml` with the library added to `[skills] paths`.
fn grok_config(config_toml: &str, library: &Path) -> Result<String, String> {
    let mut config: toml::Table = config_toml.parse().map_err(|error| format!("{error}"))?;
    let skills = config
        .entry("skills")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let skills = skills
        .as_table_mut()
        .ok_or("[skills] in config.toml is not a table")?;
    let paths = skills
        .entry("paths")
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    let paths = paths
        .as_array_mut()
        .ok_or("skills.paths in config.toml is not a list")?;
    let library = toml::Value::String(library.to_string_lossy().into_owned());
    if !paths.contains(&library) {
        paths.push(library);
    }
    toml::to_string(&config).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> McpLaunch {
        McpLaunch {
            command: "/Apps/Sikemux.app/sikemux-host".into(),
            args: vec!["--tools-mcp".into()],
        }
    }

    #[test]
    fn opencode_adds_the_skills_library_beside_the_tools() {
        let library = PathBuf::from("/data/skills");
        let config: Value = serde_json::from_str(
            &opencode_config(
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
        let integration = TerminalTools {
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
        let mut stdio = McpServerStdio::new("sikemux-tools", "/apps/sikemux-host");
        stdio.args = vec!["--tools-mcp".into()];
        stdio.env = vec![EnvVariable::new("SIKEMUX_TOOLS_AGENT_ID", "agent-one")];
        let value = serde_json::to_value(McpServer::Stdio(stdio)).unwrap();
        assert!(value.get("type").is_none(), "{value}");
        assert_eq!(value["name"], "sikemux-tools");
        assert_eq!(value["command"], "/apps/sikemux-host");
        assert_eq!(value["args"], json!(["--tools-mcp"]));
        assert_eq!(value["env"][0]["name"], "SIKEMUX_TOOLS_AGENT_ID");
        assert_eq!(value["env"][0]["value"], "agent-one");
    }

    #[test]
    fn an_acp_session_is_given_an_absolute_command_even_in_development() {
        assert_eq!(
            absolute_command(Path::new("/bin/sh")).unwrap(),
            PathBuf::from("/bin/sh")
        );
        let found = absolute_command(Path::new("sh")).unwrap();
        assert!(found.is_absolute(), "{found:?} should be absolute");
        assert!(absolute_command(Path::new("sikemux-no-such-program")).is_err());
    }

    #[test]
    fn codex_learns_the_server_through_dotted_overrides() {
        assert_eq!(
            codex_args(&launch()).unwrap(),
            vec![
                "-c".to_string(),
                "mcp_servers.sikemux_tools.command=\"/Apps/Sikemux.app/sikemux-host\"".to_string(),
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
        assert_eq!(server["command"], "/Apps/Sikemux.app/sikemux-host");
        assert_eq!(server["args"], json!(["--tools-mcp"]));
    }

    #[test]
    fn opencode_keeps_the_settings_it_already_had() {
        let existing = r#"{"theme":"dark","mcp":{"other":{"type":"local"}}}"#;
        let merged: Value =
            serde_json::from_str(&opencode_config(Some(existing), &launch(), None).unwrap())
                .unwrap();
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["mcp"]["other"]["type"], "local");
        assert_eq!(
            merged["mcp"]["sikemux_tools"]["command"],
            json!(["/Apps/Sikemux.app/sikemux-host", "--tools-mcp"])
        );
        assert_eq!(merged["mcp"]["sikemux_tools"]["enabled"], true);
    }

    #[test]
    fn opencode_starts_from_nothing_and_refuses_junk() {
        let fresh: Value =
            serde_json::from_str(&opencode_config(None, &launch(), None).unwrap()).unwrap();
        assert!(fresh["mcp"]["sikemux_tools"].is_object());
        assert!(opencode_config(Some("   "), &launch(), None).is_ok());
        assert!(opencode_config(Some("not json"), &launch(), None).is_err());
        assert!(opencode_config(Some("[1,2]"), &launch(), None).is_err());
    }

    const LIBRARY: &str = "/Users/me/Library/Application Support/sikemux/skills";

    #[test]
    fn hermes_keeps_external_dirs_in_either_yaml_form() {
        let block = "model: x\nskills:\n  enabled: true\n  external_dirs:\n    - ~/team-skills\n    - \"/opt/skills\"\nmcp_servers: {}\n";
        assert_eq!(
            hermes_external_dirs(block, Path::new(LIBRARY)),
            json!(["~/team-skills", "/opt/skills", LIBRARY]).to_string()
        );
        let flow = "skills:\n  external_dirs: [~/a, '/b']\n";
        assert_eq!(
            hermes_external_dirs(flow, Path::new(LIBRARY)),
            json!(["~/a", "/b", LIBRARY]).to_string()
        );
        assert_eq!(
            hermes_external_dirs("{}\n", Path::new(LIBRARY)),
            json!([LIBRARY]).to_string()
        );
    }

    #[test]
    fn grok_adds_the_library_to_its_skills_paths_and_keeps_the_rest() {
        let config = "model = \"grok-5\"\n\n[skills]\npaths = [\"/own\"]\n\n[mcp_servers.x]\ncommand = \"x\"\n";
        let merged: toml::Table = grok_config(config, Path::new(LIBRARY))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(merged["model"].as_str(), Some("grok-5"));
        assert_eq!(merged["skills"]["paths"].as_array().unwrap().len(), 2);
        assert_eq!(merged["mcp_servers"]["x"]["command"].as_str(), Some("x"));
        let empty: toml::Table = grok_config("", Path::new(LIBRARY))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(empty["skills"]["paths"][0].as_str(), Some(LIBRARY));
        assert!(grok_config("skills = 3", Path::new(LIBRARY)).is_err());
    }
}
