//! Signing the person in and out of their accounts with Claude Code and
//! Codex, through each CLI's own login. The core keeps the accounts and says
//! who each is signed in as; these commands pass the app's asks on to it.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use sikemux_core::agent_accounts::{cli, folders::link_shared, METHODS};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::oneshot;

use super::AgentKind;
use crate::error::{AppError, AppResult};
use crate::pty::PtyManager;

const SIGN_OUT_TIMEOUT: Duration = Duration::from_secs(12);
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SIGN_IN_TAIL: usize = 12;

/// The core's calls about the person's agent accounts.
#[tauri::command]
pub async fn agent_accounts_call(
    manager: State<'_, PtyManager>,
    method: String,
    params: Value,
) -> AppResult<Value> {
    if !METHODS.contains(&method.as_str()) {
        return Err(AppError::Other(format!("{method} is not an account call")));
    }
    let client = manager.client().await?;
    crate::agent_names::call(&client, method, params).await
}

async fn agent_command(
    agent: AgentKind,
    executable_path: Option<&str>,
    config_path: Option<&str>,
) -> Result<Command, String> {
    let program: PathBuf =
        sikemux_core::launch::discovery::resolve(agent.as_str(), executable_path).await?;
    Ok(cli(agent.as_str(), &program, config_path, BTreeMap::new()))
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
    link_shared(agent.as_str(), config_path.as_deref());
    let mut command =
        agent_command(agent, executable_path.as_deref(), config_path.as_deref()).await?;
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
    let mut command =
        agent_command(agent, executable_path.as_deref(), config_path.as_deref()).await?;
    match agent {
        AgentKind::Claude => command.args(["auth", "logout"]),
        AgentKind::Codex => command.arg("logout"),
        _ => return Err("Only Claude and Codex keep separate accounts".into()),
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(SIGN_OUT_TIMEOUT, command.status())
        .await
        .map_err(|_| "Sign-out took too long".to_string())?
        .map_err(|_| format!("Could not start the {} sign-out", agent.as_str()))?;
    if status.success() {
        Ok(())
    } else {
        Err("The agent could not sign out".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_page_is_read_from_the_cli_output() {
        assert_eq!(
            first_url("If your browser didn't open automatically, copy this URL manually: https://claude.ai/oauth/authorize?code=true&x=1."),
            Some("https://claude.ai/oauth/authorize?code=true&x=1")
        );
        assert_eq!(first_url("Starting local login server"), None);
    }
}
