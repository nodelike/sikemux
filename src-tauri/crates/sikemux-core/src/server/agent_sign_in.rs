//! Signing the person in and out of their accounts with Claude Code and
//! Codex on this host, through each CLI's own login. The page a sign-in opens
//! goes to every client, so the person can finish it from any device.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::oneshot;

use super::changed::Topic;
use crate::agent_accounts::{cli, folders::link_shared};
use crate::agent_catalog::AgentKind;

use super::{Core, CoreError, CoreResult};

const SIGN_OUT_TIMEOUT: Duration = Duration::from_secs(12);
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SIGN_IN_TAIL: usize = 12;
const MAX_PATH_CHARS: usize = 4_096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    agent: AgentKind,
    executable_path: Option<String>,
    config_path: Option<String>,
}

impl Account {
    fn check(&self) -> CoreResult<()> {
        let long = |path: &Option<String>| {
            path.as_ref()
                .is_some_and(|path| path.len() > MAX_PATH_CHARS)
        };
        if long(&self.executable_path) || long(&self.config_path) {
            return Err("that path is longer than Sikemux takes".into());
        }
        Ok(())
    }

    fn key(&self) -> String {
        format!(
            "{}\0{}",
            self.agent.as_str(),
            self.config_path.as_deref().unwrap_or("")
        )
    }

    async fn command(&self) -> CoreResult<Command> {
        let program: PathBuf =
            crate::launch::discovery::resolve(self.agent.as_str(), self.executable_path.as_deref())
                .await?;
        Ok(cli(
            self.agent.as_str(),
            &program,
            self.config_path.as_deref(),
            BTreeMap::new(),
        ))
    }
}

/// The page a running sign-in opened, for a client to open where no browser did.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Page {
    agent: &'static str,
    config_path: Option<String>,
    url: String,
}

/// A sign-in running for one account, which the person can answer or stop.
struct SignIn {
    input: Option<ChildStdin>,
    stop: Option<oneshot::Sender<()>>,
    page: Option<Page>,
}

fn sign_ins() -> MutexGuard<'static, HashMap<String, SignIn>> {
    static SIGN_INS: OnceLock<Mutex<HashMap<String, SignIn>>> = OnceLock::new();
    SIGN_INS.get_or_init(Mutex::default).locked()
}

fn first_url(line: &str) -> Option<&str> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'' || ch == '>')
        .unwrap_or(rest.len());
    Some(rest[..end].trim_end_matches(['.', ',', ')']))
}

/// The pages the sign-ins running now opened.
pub(super) fn pages() -> Vec<Page> {
    sign_ins()
        .values()
        .filter_map(|sign_in| sign_in.page.clone())
        .collect()
}

fn pages_changed(core: &Core) {
    core.changes.changed(Topic::AgentSignIn, None, None);
}

#[derive(Deserialize)]
pub(super) struct LauncherParams {
    /// The launcher whose account to sign in to, such as `claude:work`.
    launcher: String,
}

/// Signs the person in to the account a launcher starts agents on, as
/// [`sign_in`] does.
pub(super) async fn sign_in_launcher(core: &Arc<Core>, params: LauncherParams) -> CoreResult<()> {
    let (provider, profile) = match params.launcher.split_once(':') {
        Some((provider, profile)) => (provider, Some(profile)),
        None => (params.launcher.as_str(), None),
    };
    let agent = AgentKind::from_name(provider)
        .ok_or_else(|| CoreError::from("that agent is not one Sikemux knows"))?;
    let profile = profile.and_then(|id| core.workspaces.accounts()?.profile(id).cloned());
    let account = Account {
        agent,
        executable_path: profile
            .as_ref()
            .and_then(|profile| profile.executable_path.clone()),
        config_path: profile.and_then(|profile| profile.config_path),
    };
    sign_in(core, account).await
}

/// Runs the agent's own sign-in for one account and answers once it ends.
/// The CLI opens a browser on the host itself; the page it opens is offered
/// to every client, for when the person is elsewhere.
pub(super) async fn sign_in(core: &Arc<Core>, account: Account) -> CoreResult<()> {
    account.check()?;
    link_shared(account.agent.as_str(), account.config_path.as_deref());
    let args = account
        .agent
        .ops()
        .account_sign_in()
        .ok_or("Only Claude and Codex keep separate accounts")?;
    let mut command = account.command().await?;
    command.args(args);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| format!("Could not start the {} sign-in", account.agent.as_str()))?;
    let key = account.key();
    let (stop, stopped) = oneshot::channel();
    let previous = sign_ins().insert(
        key.clone(),
        SignIn {
            input: child.stdin.take(),
            stop: Some(stop),
            page: None,
        },
    );
    if let Some(stop) = previous.and_then(|previous| previous.stop) {
        let _ = stop.send(());
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
                            if let Some(running) = sign_ins().get_mut(&key) {
                                running.page = Some(Page {
                                    agent: account.agent.as_str(),
                                    config_path: account.config_path.clone(),
                                    url: url.to_owned(),
                                });
                            }
                            pages_changed(core);
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
    sign_ins().remove(&key);
    pages_changed(core);
    match ended {
        Ok(Some(Ok(status))) if status.success() => Ok(()),
        Ok(Some(_)) => Err(match tail.last() {
            Some(last) => format!("Sign-in did not finish: {last}"),
            None => "Sign-in did not finish".into(),
        }
        .into()),
        Ok(None) => Err("Sign-in stopped".into()),
        Err(_) => Err("Sign-in timed out".into()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Code {
    #[serde(flatten)]
    account: Account,
    code: String,
}

/// Hands a running sign-in the code its page showed, for a CLI that asks
/// for one when the browser could not call it back.
pub(super) async fn sign_in_code(given: Code) -> CoreResult<()> {
    let code = given.code.trim().to_owned();
    if code.is_empty() || code.len() > 4_096 || code.contains(['\n', '\r', '\0']) {
        return Err("That code cannot be right".into());
    }
    let key = given.account.key();
    let mut input = sign_ins()
        .get_mut(&key)
        .and_then(|sign_in| sign_in.input.take())
        .ok_or_else(|| CoreError::from("No sign-in is waiting for a code"))?;
    let written = async {
        input.write_all(format!("{code}\n").as_bytes()).await?;
        input.flush().await
    }
    .await;
    if let Some(sign_in) = sign_ins().get_mut(&key) {
        sign_in.input = Some(input);
    }
    written.map_err(|_| CoreError::from("Could not hand the code to the sign-in"))
}

pub(super) fn cancel(account: Account) {
    let stop = sign_ins()
        .get_mut(&account.key())
        .and_then(|sign_in| sign_in.stop.take());
    if let Some(stop) = stop {
        let _ = stop.send(());
    }
}

pub(super) async fn sign_out(account: Account) -> CoreResult<()> {
    account.check()?;
    let args = account
        .agent
        .ops()
        .account_sign_out()
        .ok_or("Only Claude and Codex keep separate accounts")?;
    let mut command = account.command().await?;
    command.args(args);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(SIGN_OUT_TIMEOUT, command.status())
        .await
        .map_err(|_| CoreError::from("Sign-out took too long"))?
        .map_err(|_| format!("Could not start the {} sign-out", account.agent.as_str()))?;
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
