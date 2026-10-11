//! The person's accounts with Claude Code and Codex.
//!
//! Each account is a directory the agent's CLI keeps its sign-in in, named by
//! `CLAUDE_CONFIG_DIR` or `CODEX_HOME`. Signing in and out runs the CLI's own
//! login and logout commands, and who is signed in comes from the CLI too:
//! Sikemux never reads or stores a credential.
//!
//! An account Sikemux adds links its chats, settings and tools to the default
//! directory, so a chat can move between accounts and only the sign-in
//! differs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::oneshot;

use super::config::agent_config_root;
use super::executable::{apply_login_environment, apply_process_config, expand_user_path};
use super::status::forget_status;
use super::usage::{codex_app_server, forget_agent_usage};
use super::AgentKind;

/// Marks a directory as one Sikemux made, whose shared entries it may link.
const MARKER: &str = ".sikemux-account";
const STATUS_TIMEOUT: Duration = Duration::from_secs(12);
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SIGN_IN_TAIL: usize = 12;

/// What a Claude account shares with the default one. Anything else, such as
/// the sign-in and the CLI's own caches, stays with the account.
const CLAUDE_SHARED: &[&str] = &[
    "projects",
    "settings.json",
    "CLAUDE.md",
    "agents",
    "commands",
    "skills",
    "plugins",
    "hooks",
    "output-styles",
    "todos",
    "plans",
    "file-history",
    "history.jsonl",
    "keybindings.json",
    "session-env",
    "shell-snapshots",
];

/// What a Codex account keeps to itself. Everything else is shared.
const CODEX_OWN: &[&str] = &[
    "auth.json",
    "models_cache.json",
    "log",
    "memories",
    "tmp",
    MARKER,
];

fn shares(agent: AgentKind, name: &str) -> bool {
    match agent {
        AgentKind::Claude => CLAUDE_SHARED.contains(&name),
        AgentKind::Codex => !CODEX_OWN.contains(&name),
        _ => false,
    }
}

fn default_root(agent: AgentKind) -> Option<PathBuf> {
    let home = crate::system::user_home();
    match agent {
        AgentKind::Claude => Some(home.join(".claude")),
        AgentKind::Codex => Some(home.join(".codex")),
        _ => None,
    }
}

/// The folder an account keeps its chats in. Accounts that share one can
/// take over each other's chats.
fn sessions_root(agent: AgentKind, config_path: Option<&str>) -> Option<PathBuf> {
    let root = agent_config_root(agent.as_str(), config_path)?;
    let folder = match agent {
        AgentKind::Claude => "projects",
        AgentKind::Codex => "sessions",
        _ => return None,
    };
    let path = root.join(folder);
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

pub(crate) fn sessions_shared(provider: &str, one: Option<&str>, other: Option<&str>) -> bool {
    let Some(agent) = kind(provider) else {
        return false;
    };
    sessions_root(agent, one).is_some_and(|root| sessions_root(agent, other) == Some(root))
}

pub(crate) fn kind(provider: &str) -> Option<AgentKind> {
    match provider {
        "claude" => Some(AgentKind::Claude),
        "codex" => Some(AgentKind::Codex),
        _ => None,
    }
}

/// Links the default directory's shared entries into an account Sikemux
/// made. Entries already there are left alone, so a file the account wrote
/// itself is never replaced.
pub(crate) fn link_shared(agent: AgentKind, config_path: Option<&str>) {
    let (Some(account), Some(shared)) = (
        config_path.and_then(|path| agent_config_root(agent.as_str(), Some(path))),
        default_root(agent),
    ) else {
        return;
    };
    link_into(agent, &account, &shared);
}

fn link_into(agent: AgentKind, account: &Path, shared: &Path) {
    if !account.join(MARKER).is_file() || account == shared {
        return;
    }
    let sessions = match agent {
        AgentKind::Claude => "projects",
        _ => "sessions",
    };
    let _ = std::fs::create_dir_all(shared.join(sessions));
    let Ok(entries) = std::fs::read_dir(shared) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let target = account.join(name);
        if !shares(agent, name) || target.symlink_metadata().is_ok() {
            continue;
        }
        #[cfg(unix)]
        if let Err(error) = std::os::unix::fs::symlink(entry.path(), &target) {
            eprintln!("Could not share {name} with the account in {account:?}: {error}");
        }
    }
}

fn slug(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug: String = slug.chars().take(32).collect();
    if slug.is_empty() {
        "account".into()
    } else {
        slug
    }
}

/// What a new Claude account starts with from the default one: its tool
/// servers and which projects it trusts. Claude keeps both beside the
/// sign-in, so they cannot be linked.
fn claude_seed(default: &Value) -> Value {
    let mut seed = Map::new();
    seed.insert("hasCompletedOnboarding".into(), json!(true));
    for key in ["mcpServers", "theme", "lastOnboardingVersion"] {
        if let Some(value) = default.get(key) {
            seed.insert(key.into(), value.clone());
        }
    }
    if let Some(projects) = default.get("projects").and_then(Value::as_object) {
        let kept: Map<String, Value> = projects
            .iter()
            .map(|(path, project)| {
                let mut kept = Map::new();
                for key in [
                    "allowedTools",
                    "mcpServers",
                    "enabledMcpjsonServers",
                    "disabledMcpjsonServers",
                    "hasTrustDialogAccepted",
                ] {
                    if let Some(value) = project.get(key) {
                        kept.insert(key.into(), value.clone());
                    }
                }
                (path.clone(), Value::Object(kept))
            })
            .collect();
        seed.insert("projects".into(), Value::Object(kept));
    }
    Value::Object(seed)
}

fn make_account(agent: AgentKind, name: &str, home: &Path) -> Result<String, String> {
    let prefix = match agent {
        AgentKind::Claude => ".claude",
        AgentKind::Codex => ".codex",
        _ => return Err("Only Claude and Codex keep separate accounts".into()),
    };
    let base = format!("{prefix}-{}", slug(name));
    let mut folder = base.clone();
    let mut counter = 2;
    while home.join(&folder).exists() {
        folder = format!("{base}-{counter}");
        counter += 1;
    }
    let directory = home.join(&folder);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    std::fs::write(
        directory.join(MARKER),
        "Made by Sikemux for one account. Links here point at the default account's shared files.\n",
    )
    .map_err(|error| error.to_string())?;
    if matches!(agent, AgentKind::Claude) {
        let default = std::fs::read(home.join(".claude.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or(Value::Null);
        let seed = serde_json::to_vec_pretty(&claude_seed(&default)).map_err(|e| e.to_string())?;
        std::fs::write(directory.join(".claude.json"), seed).map_err(|e| e.to_string())?;
    }
    Ok(format!("~/{folder}"))
}

/// Makes a directory for another account, sharing what the default one
/// has, and returns it as a profile's directory.
#[tauri::command]
pub async fn agent_account_add(agent: AgentKind, name: String) -> Result<String, String> {
    let home = crate::system::user_home();
    let path = make_account(agent, &name, &home)?;
    link_shared(agent, Some(&path));
    Ok(path)
}

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentAccountStatus {
    signed_in: bool,
    /// The person's own name on the account, where the agent keeps one.
    name: Option<String>,
    email: Option<String>,
    plan: Option<String>,
    organization: Option<String>,
    /// `subscription`, `apiKey`, or what else the CLI says it signs in with.
    method: Option<String>,
    sessions: Option<String>,
}

impl AgentAccountStatus {
    pub(super) fn is_signed_in(&self) -> bool {
        self.signed_in
    }

    /// How the account signs in, such as `subscription` or `apiKey`.
    pub(super) fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }
}

fn parse_claude_status(text: &str) -> Option<AgentAccountStatus> {
    let status: Value = serde_json::from_str(text.trim()).ok()?;
    let text_of = |key: &str| {
        status
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let method = text_of("authMethod").filter(|method| method != "none");
    Some(AgentAccountStatus {
        signed_in: status
            .get("loggedIn")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        name: None,
        email: text_of("email"),
        plan: text_of("subscriptionType"),
        organization: text_of("orgName"),
        method: method.map(|method| {
            if method == "claude.ai" {
                "subscription".into()
            } else {
                method
            }
        }),
        sessions: None,
    })
}

/// `auth status` leaves the name out; Claude keeps it in the account's
/// `.claude.json`, which holds no credentials.
fn claude_account_name(config: &Value) -> Option<String> {
    let account = config.get("oauthAccount")?;
    ["displayName", "fullName"]
        .into_iter()
        .filter_map(|key| account.get(key).and_then(Value::as_str))
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Claude puts `.claude.json` inside a chosen config folder, but beside the
/// default `~/.claude` folder rather than in it.
fn claude_config_file(config_path: Option<&str>) -> Option<PathBuf> {
    let root = agent_config_root("claude", config_path)?;
    let inside = root.join(".claude.json");
    if inside.exists() {
        return Some(inside);
    }
    let home = crate::system::user_home();
    (root == home.join(".claude")).then(|| home.join(".claude.json"))
}

fn parse_codex_status(result: &Value) -> AgentAccountStatus {
    let account = result.get("account").filter(|account| !account.is_null());
    let text_of = |key: &str| {
        account
            .and_then(|account| account.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let method = text_of("type").map(|kind| match kind.as_str() {
        "chatgpt" => "subscription".to_owned(),
        _ => kind,
    });
    AgentAccountStatus {
        signed_in: account.is_some(),
        name: None,
        email: text_of("email"),
        plan: text_of("planType"),
        organization: None,
        method,
        sessions: None,
    }
}

pub(super) fn agent_command(
    agent: AgentKind,
    executable: &Path,
    config_path: Option<&str>,
) -> Command {
    let mut command = Command::from(sikemux_process::user_environment::command(executable));
    apply_login_environment(&mut command);
    apply_process_config(&mut command, agent.as_str(), config_path);
    command.kill_on_drop(true);
    command
}

async fn executable(agent: AgentKind, executable_path: Option<&str>) -> Result<PathBuf, String> {
    super::resolve_agent_executable(agent.as_str(), executable_path).await
}

/// Who an account is signed in as, asked of the agent's own CLI.
#[tauri::command]
pub async fn agent_account_status(
    agent: AgentKind,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<AgentAccountStatus, String> {
    link_shared(agent, config_path.as_deref());
    let program = executable(agent, executable_path.as_deref()).await?;
    let mut status = match agent {
        AgentKind::Claude => {
            let mut command = agent_command(agent, &program, config_path.as_deref());
            command
                .args(["auth", "status", "--json"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            let output = tokio::time::timeout(STATUS_TIMEOUT, command.output())
                .await
                .map_err(|_| "Claude took too long to say who is signed in".to_string())?
                .map_err(|_| "Could not ask Claude who is signed in".to_string())?;
            // A signed-out account still answers, with a failing exit code.
            parse_claude_status(&String::from_utf8_lossy(&output.stdout))
                .ok_or_else(|| "Claude did not say who is signed in".to_string())?
        }
        AgentKind::Codex => parse_codex_status(
            &codex_app_server(&program, config_path.as_deref(), "account/read", "account").await?,
        ),
        _ => return Err("Only Claude and Codex keep separate accounts".into()),
    };
    if matches!(agent, AgentKind::Claude) && status.signed_in {
        status.name = claude_config_file(config_path.as_deref())
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|config| claude_account_name(&config));
    }
    status.sessions = sessions_root(agent, config_path.as_deref())
        .map(|path| path.to_string_lossy().into_owned());
    Ok(status)
}

/// A sign-in running for one account, which the person can answer or stop.
struct SignIn {
    input: Option<ChildStdin>,
    stop: Option<oneshot::Sender<()>>,
}

fn sign_ins() -> &'static Mutex<HashMap<String, SignIn>> {
    static SIGN_INS: OnceLock<Mutex<HashMap<String, SignIn>>> = OnceLock::new();
    SIGN_INS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sign_in_key(agent: AgentKind, config_path: Option<&str>) -> String {
    format!("{}\0{}", agent.as_str(), config_path.unwrap_or(""))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignInProgress<'a> {
    agent: &'a str,
    config_path: Option<&'a str>,
    url: &'a str,
}

fn first_url(line: &str) -> Option<&str> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'' || ch == '>')
        .unwrap_or(rest.len());
    Some(rest[..end].trim_end_matches(['.', ',', ')']))
}

/// Runs the agent's own sign-in for one account and answers once it ends.
/// The CLI opens the browser itself; the page it opens is passed on as
/// `agent_account_sign_in`, for when no browser opened.
#[tauri::command]
pub async fn agent_account_sign_in(
    app: AppHandle,
    agent: AgentKind,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<(), String> {
    link_shared(agent, config_path.as_deref());
    let program = executable(agent, executable_path.as_deref()).await?;
    let mut command = agent_command(agent, &program, config_path.as_deref());
    match agent {
        AgentKind::Claude => command.args(["auth", "login", "--claudeai"]),
        AgentKind::Codex => command.arg("login"),
        _ => return Err("Only Claude and Codex keep separate accounts".into()),
    };
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| format!("Could not start the {} sign-in", agent.as_str()))?;
    let key = sign_in_key(agent, config_path.as_deref());
    let (stop, stopped) = oneshot::channel();
    if let Ok(mut running) = sign_ins().lock() {
        if let Some(previous) = running.insert(
            key.clone(),
            SignIn {
                input: child.stdin.take(),
                stop: Some(stop),
            },
        ) {
            if let Some(stop) = previous.stop {
                let _ = stop.send(());
            }
        }
    }

    let (lines_tx, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    for stream in [
        child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let lines_tx = lines_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stream).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = lines_tx.send(line);
            }
        });
    }
    drop(lines_tx);

    let mut tail: Vec<String> = Vec::new();
    let mut shown_url = false;
    let watch = async {
        tokio::select! {
            status = async {
                while let Some(line) = lines.recv().await {
                    if !shown_url {
                        if let Some(url) = first_url(&line) {
                            shown_url = true;
                            let _ = app.emit_to(
                                "main",
                                "agent_account_sign_in",
                                SignInProgress { agent: agent.as_str(), config_path: config_path.as_deref(), url },
                            );
                        }
                    }
                    let clean = line.trim();
                    if !clean.is_empty() {
                        tail.push(clean.to_owned());
                        if tail.len() > SIGN_IN_TAIL {
                            tail.remove(0);
                        }
                    }
                }
                child.wait().await
            } => Some(status),
            _ = stopped => None,
        }
    };
    let ended = tokio::time::timeout(SIGN_IN_TIMEOUT, watch).await;
    if let Ok(mut running) = sign_ins().lock() {
        running.remove(&key);
    }
    forget_agent_usage(agent, config_path.as_deref());
    forget_status(agent, config_path.as_deref());
    match ended {
        Ok(Some(Ok(status))) if status.success() => Ok(()),
        Ok(Some(_)) => Err(match tail.last() {
            Some(last) => format!("Sign-in did not finish: {last}"),
            None => "Sign-in did not finish".into(),
        }),
        Ok(None) => Err("Sign-in stopped".into()),
        Err(_) => Err("Sign-in timed out".into()),
    }
}

/// Hands a running sign-in the code its page showed, for a CLI that asks
/// for one when the browser could not call it back.
#[tauri::command]
pub async fn agent_account_sign_in_code(
    agent: AgentKind,
    config_path: Option<String>,
    code: String,
) -> Result<(), String> {
    let code = code.trim().to_owned();
    if code.is_empty() || code.len() > 4_096 || code.contains(['\n', '\r', '\0']) {
        return Err("That code cannot be right".into());
    }
    let key = sign_in_key(agent, config_path.as_deref());
    let input = sign_ins()
        .lock()
        .map_err(|_| "Sign-in state is unavailable".to_string())?
        .get_mut(&key)
        .and_then(|sign_in| sign_in.input.take())
        .ok_or_else(|| "No sign-in is waiting for a code".to_string())?;
    let mut input = input;
    input
        .write_all(format!("{code}\n").as_bytes())
        .await
        .map_err(|_| "Could not hand the code to the sign-in".to_string())?;
    input.flush().await.map_err(|error| error.to_string())?;
    if let Ok(mut running) = sign_ins().lock() {
        if let Some(sign_in) = running.get_mut(&key) {
            sign_in.input = Some(input);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn agent_account_sign_in_cancel(agent: AgentKind, config_path: Option<String>) {
    let key = sign_in_key(agent, config_path.as_deref());
    if let Some(stop) = sign_ins().lock().ok().and_then(|mut running| {
        running
            .get_mut(&key)
            .and_then(|sign_in| sign_in.stop.take())
    }) {
        let _ = stop.send(());
    }
}

#[tauri::command]
pub async fn agent_account_sign_out(
    agent: AgentKind,
    executable_path: Option<String>,
    config_path: Option<String>,
) -> Result<(), String> {
    let program = executable(agent, executable_path.as_deref()).await?;
    let mut command = agent_command(agent, &program, config_path.as_deref());
    match agent {
        AgentKind::Claude => command.args(["auth", "logout"]),
        AgentKind::Codex => command.arg("logout"),
        _ => return Err("Only Claude and Codex keep separate accounts".into()),
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(STATUS_TIMEOUT, command.status())
        .await
        .map_err(|_| "Sign-out took too long".to_string())?
        .map_err(|_| format!("Could not start the {} sign-out", agent.as_str()))?;
    forget_agent_usage(agent, config_path.as_deref());
    forget_status(agent, config_path.as_deref());
    if status.success() {
        Ok(())
    } else {
        Err("The agent could not sign out".into())
    }
}

/// The variables that point an agent at an account's directory.
pub(crate) fn account_environment(
    provider: &str,
    config_path: Option<&str>,
) -> Vec<(String, String)> {
    let Some(variable) = sikemux_core::acp::account::directory_variable(provider) else {
        return Vec::new();
    };
    let Some(path) = config_path.map(str::trim).filter(|path| !path.is_empty()) else {
        // The default account is wherever the person's shell points the agent.
        return sikemux_pty::user_shell::login_shell_environment()
            .get(variable)
            .filter(|path| !path.is_empty())
            .map(|path| vec![(variable.to_owned(), path.clone())])
            .unwrap_or_default();
    };
    let root = agent_config_root(provider, Some(path)).unwrap_or_else(|| expand_user_path(path));
    vec![(variable.to_owned(), root.to_string_lossy().into_owned())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_name_becomes_a_folder_name() {
        assert_eq!(slug("Work"), "work");
        assert_eq!(slug("  My Team / Max  "), "my-team-max");
        assert_eq!(slug("???"), "account");
    }

    #[test]
    fn a_new_claude_account_shares_chats_and_settings_but_not_its_sign_in() {
        let home = tempfile::tempdir().unwrap();
        let shared = home.path().join(".claude");
        std::fs::create_dir_all(shared.join("projects/-tmp")).unwrap();
        std::fs::write(shared.join("settings.json"), "{}").unwrap();
        std::fs::write(shared.join(".credentials.json"), "secret").unwrap();
        std::fs::write(
            home.path().join(".claude.json"),
            r#"{"oauthAccount":{"accountUuid":"me"},"mcpServers":{"x":{}},"projects":{"/p":{"hasTrustDialogAccepted":true,"lastCost":3}}}"#,
        )
        .unwrap();

        let path = make_account(AgentKind::Claude, "Work", home.path()).unwrap();
        assert_eq!(path, "~/.claude-work");
        let account = home.path().join(".claude-work");
        assert!(account.join(MARKER).is_file());
        let seed: Value =
            serde_json::from_slice(&std::fs::read(account.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(seed["mcpServers"], json!({ "x": {} }));
        assert_eq!(
            seed["projects"]["/p"],
            json!({ "hasTrustDialogAccepted": true })
        );
        assert!(seed.get("oauthAccount").is_none());

        assert_eq!(
            make_account(AgentKind::Claude, "Work", home.path()).unwrap(),
            "~/.claude-work-2"
        );
    }

    #[cfg(unix)]
    #[test]
    fn linking_shares_only_what_is_shared_and_keeps_what_is_there() {
        let home = tempfile::tempdir().unwrap();
        let shared = home.path().join(".codex");
        std::fs::create_dir_all(shared.join("sessions")).unwrap();
        std::fs::write(shared.join("config.toml"), "").unwrap();
        std::fs::write(shared.join("auth.json"), "{}").unwrap();
        let account = home.path().join(".codex-work");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("config.toml"), "own = true").unwrap();

        link_into(AgentKind::Codex, &account, &shared);
        assert!(
            !account.join("sessions").exists(),
            "linked into a folder it did not make"
        );

        std::fs::write(account.join(MARKER), "").unwrap();
        link_into(AgentKind::Codex, &account, &shared);
        assert_eq!(
            std::fs::read_link(account.join("sessions")).unwrap(),
            shared.join("sessions")
        );
        assert_eq!(
            std::fs::read_to_string(account.join("config.toml")).unwrap(),
            "own = true"
        );
        assert!(!account.join("auth.json").exists());

        assert!(shares(AgentKind::Claude, "projects"));
        assert!(!shares(AgentKind::Claude, ".credentials.json"));
        assert!(!shares(AgentKind::Claude, "daemon"));
    }

    #[test]
    fn claude_says_who_is_signed_in() {
        let status = parse_claude_status(
            r#"{"loggedIn":true,"authMethod":"claude.ai","email":"me@x.com","orgName":"Me","subscriptionType":"max"}"#,
        )
        .unwrap();
        assert!(status.signed_in);
        assert_eq!(status.email.as_deref(), Some("me@x.com"));
        assert_eq!(status.plan.as_deref(), Some("max"));
        assert_eq!(status.method.as_deref(), Some("subscription"));
        let out = parse_claude_status(r#"{"loggedIn":false,"authMethod":"none"}"#).unwrap();
        assert!(!out.signed_in);
        assert_eq!(out.method, None);
        assert_eq!(parse_claude_status("not json"), None);
    }

    #[test]
    fn claude_keeps_the_account_name_beside_the_sign_in() {
        let config =
            json!({ "oauthAccount": { "displayName": "Kishore", "fullName": "Kishore G" } });
        assert_eq!(claude_account_name(&config).as_deref(), Some("Kishore"));
        let full = json!({ "oauthAccount": { "displayName": " ", "fullName": "Kishore G" } });
        assert_eq!(claude_account_name(&full).as_deref(), Some("Kishore G"));
        assert_eq!(claude_account_name(&json!({ "projects": {} })), None);
    }

    #[test]
    fn codex_says_who_is_signed_in() {
        let status = parse_codex_status(&json!({
            "account": { "type": "chatgpt", "email": "me@x.com", "planType": "pro" },
            "requiresOpenaiAuth": true,
        }));
        assert!(status.signed_in);
        assert_eq!(status.plan.as_deref(), Some("pro"));
        assert_eq!(status.method.as_deref(), Some("subscription"));
        let key = parse_codex_status(&json!({ "account": { "type": "apiKey" } }));
        assert_eq!(key.method.as_deref(), Some("apiKey"));
        assert!(!parse_codex_status(&json!({ "account": null })).signed_in);
    }

    #[test]
    fn the_sign_in_page_is_read_from_the_cli_output() {
        assert_eq!(
            first_url("If your browser didn't open automatically, copy this URL manually: https://claude.ai/oauth/authorize?code=true&x=1."),
            Some("https://claude.ai/oauth/authorize?code=true&x=1")
        );
        assert_eq!(first_url("Starting local login server"), None);
    }
}
