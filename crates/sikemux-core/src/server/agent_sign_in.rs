//! Signing the person in and out of their accounts with Claude Code, Codex and
//! Antigravity on this host, through each CLI's own login. The page a sign-in opens
//! goes to every client, so the person can finish it from any device.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::agent_sign_in::Page;
pub use sikemux_calls::agent_sign_in::{Account, Code, SigningIn};
pub use sikemux_calls::agent_sign_in::{AfterSignIn, LauncherParams};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::command::AsyncCommand;
use sikemux_process::sync::Locked;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot, watch};

use super::changed::Topic;
use crate::agent_accounts::{antigravity, cli, folders::link_shared};
use crate::agent_catalog::AgentKind;

use super::{Core, CoreError, CoreResult};

const SIGN_OUT_TIMEOUT: Duration = Duration::from_secs(12);
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SIGN_IN_TAIL: usize = 12;
const MAX_PATH_CHARS: usize = 4_096;

pub(crate) trait AccountExt: Sized {
    fn check(&self) -> CoreResult<()>;

    fn key(&self) -> String;

    async fn command(&self) -> CoreResult<AsyncCommand>;

    fn added_antigravity(&self) -> CoreResult<Option<antigravity::Added>>;
}

impl AccountExt for Account {
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

    async fn command(&self) -> CoreResult<AsyncCommand> {
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

    fn added_antigravity(&self) -> CoreResult<Option<antigravity::Added>> {
        match self.agent {
            AgentKind::Antigravity => Ok(antigravity::added(self.config_path.as_deref())?),
            _ => Ok(None),
        }
    }
}

/// A sign-in running for one account, which the person can answer or stop.
struct SignIn {
    id: uuid::Uuid,
    input: Option<ChildStdin>,
    stop: Option<oneshot::Sender<()>>,
    page: Option<Page>,
    by_code: bool,
    /// How the sign-in ended, once it has.
    ended: Ended,
}

type Ended = watch::Receiver<Option<Result<(), String>>>;

/// Takes a sign-in off the list as it ends, unless another took its place.
struct Entry {
    key: String,
    id: uuid::Uuid,
}

impl Drop for Entry {
    fn drop(&mut self) {
        let mut sign_ins = sign_ins();
        if (sign_ins.get(&self.key)).is_some_and(|own| own.id == self.id) {
            sign_ins.remove(&self.key);
        }
    }
}

fn sign_ins() -> MutexGuard<'static, HashMap<String, SignIn>> {
    static SIGN_INS: OnceLock<Mutex<HashMap<String, SignIn>>> = OnceLock::new();
    SIGN_INS.get_or_init(Mutex::default).locked()
}

fn first_url(line: &str) -> Option<&str> {
    let start = line.find("https://")?;
    let rest = line.get(start..)?;
    let url = rest
        .split(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'' || ch == '>')
        .next()
        .unwrap_or(rest);
    Some(url.trim_end_matches(['.', ',', ')']))
}

pub(super) fn pages() -> Vec<Page> {
    sign_ins()
        .values()
        .filter_map(|sign_in| sign_in.page.clone())
        .collect()
}

fn pages_changed(core: &Core) {
    core.changed(What::Read(Topic::AgentSignIn));
}

/// The chats waiting for an account's sign-in, by the account's id.
fn waiting() -> MutexGuard<'static, HashMap<String, Vec<String>>> {
    static WAITING: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    WAITING.get_or_init(Mutex::default).locked()
}

/// Moves the chat onto the account as soon as a sign-in to it finishes, from whichever client.
pub(super) fn move_after_sign_in(core: &Core, after: AfterSignIn) -> CoreResult<()> {
    core.workspaces().agent_or_gone(&after.agent_id)?;
    let mut waiting = waiting();
    let chats = waiting.entry(after.account_id).or_default();
    if !chats.contains(&after.agent_id) {
        chats.push(after.agent_id);
    }
    Ok(())
}

/// Moves the chats waiting for the account in `config_path` onto it, and asks what it offers.
fn signed_in(core: &Arc<Core>, account: &Account) {
    let book = core
        .workspaces()
        .accounts()
        .unwrap_or_default()
        .with_built_ins();
    let profiles = book.profiles.into_iter().filter(|profile| {
        profile.provider == account.agent.as_str() && profile.config_path == account.config_path
    });
    for profile in profiles {
        tokio::spawn(read_options(core.clone(), profile.clone()));
        let Some(chats) = waiting().remove(&profile.id) else {
            continue;
        };
        let target = crate::agent_accounts::chat_account(&profile);
        for agent_id in chats {
            let (core, target) = (core.clone(), target.clone());
            tokio::spawn(async move {
                if let Err(error) = super::chat::move_chat(&core, &agent_id, target).await {
                    eprintln!(
                        "sikemux core: {agent_id} stayed on its account after sign-in: {error}"
                    );
                }
            });
        }
    }
}

/// Asks `profile`'s agent what it offers, so a chat that takes the account has its models.
async fn read_options(core: Arc<Core>, profile: crate::agent_accounts::Profile) {
    let id = super::workspace::LauncherId::on(&profile.provider, &profile.id).to_string();
    let launch = async {
        let launcher = core.workspaces().launcher(&id)?;
        let program = launcher.agent.program().await?;
        let folder = match profile.config_path.as_deref() {
            Some(path) => sikemux_process::paths::account_dir(path),
            None => std::env::temp_dir(),
        };
        let owner = core.clone();
        super::connection::blocking(move || {
            owner
                .workspaces()
                .chat_launch(&launcher, program, &folder, None, None)
                .map_err(CoreError::from)
        })
        .await
    };
    match launch.await {
        Ok(launch) => super::chat::ask_offers(&core, id, launch),
        Err(error) => eprintln!("sikemux core: {id} could not be asked what it offers: {error}"),
    }
}

/// Signs the person in to the account a launcher starts agents on, as
/// [`sign_in`] does.
pub(super) async fn sign_in_launcher(core: &Arc<Core>, params: LauncherParams) -> CoreResult<()> {
    let launcher = super::workspace::LauncherId::parse(&params.launcher);
    let agent = AgentKind::from_name(launcher.kind)
        .ok_or_else(|| CoreError::from("that agent is not one Sikemux knows"))?;
    let profile =
        (launcher.account).and_then(|id| core.workspaces().accounts()?.profile(id).cloned());
    let account = Account {
        agent,
        executable_path: profile
            .as_ref()
            .and_then(|profile| profile.executable_path.clone()),
        config_path: profile.and_then(|profile| profile.config_path),
    };
    let by_code = false;
    sign_in(core, SigningIn { account, by_code }).await
}

/// Runs the agent's own sign-in for one account and answers once it ends.
/// The CLI opens a browser on the host itself; the page it opens is offered
/// to every client, for when the person is elsewhere.
pub(super) async fn sign_in(core: &Arc<Core>, signing: SigningIn) -> CoreResult<()> {
    let SigningIn { account, by_code } = signing;
    account.check()?;
    let added = account.added_antigravity()?;
    // agy's server would go on with a sign-in it already keeps, even one Google has ended.
    let by_code = by_code || added.as_ref().is_some_and(antigravity::Added::signed_in);
    // Its page gives a code only the sign-in that opened it takes, so asking again carries on
    // with that one.
    let same = match added {
        Some(_) => sign_ins()
            .get(&account.key())
            .filter(|sign_in| sign_in.stop.is_some() && (sign_in.by_code || !by_code))
            .map(|sign_in| sign_in.ended.clone()),
        None => None,
    };
    if let Some(mut same) = same {
        pages_changed(core);
        let ended = same.wait_for(Option::is_some).await;
        return match ended.ok().and_then(|ended| ended.clone()) {
            Some(ended) => Ok(ended?),
            None => Err("Sign-in stopped".into()),
        };
    }
    let (end, ended) = watch::channel(None);
    let answer = run(core, &account, by_code, ended).await;
    let _ = end.send(Some(
        (answer.as_ref().copied()).map_err(|error| error.message.clone()),
    ));
    answer
}

async fn run(core: &Arc<Core>, account: &Account, by_code: bool, ended: Ended) -> CoreResult<()> {
    link_shared(account.agent.as_str(), account.config_path.as_deref());
    let added = account.added_antigravity()?;
    let server = match &added {
        Some(added) if !by_code => Some(added.server()?),
        _ => None,
    };
    let args = match &server {
        Some(server) => [vec!["acp".to_owned()], server.args()].concat(),
        None => account
            .agent
            .ops()
            .account_sign_in(account.config_path.as_deref())
            .ok_or(crate::agent_accounts::NO_ACCOUNTS)?,
    };
    let mut command = account.command().await?;
    command.args(args);
    command
        .stdin(sikemux_process::command::Stdio::piped())
        .stdout(sikemux_process::command::Stdio::piped())
        .stderr(sikemux_process::command::Stdio::piped())
        .kill_on_drop(true);
    let store = added.as_ref().map(antigravity::Added::signing_in);
    let mut child = command
        .spawn()
        .map_err(|_| format!("Could not start the {} sign-in", account.agent.as_str()))?;
    let mut input = child.stdin.take();
    let mut asking = server.as_ref().and_then(|_| input.take());
    let key = account.key();
    let id = uuid::Uuid::new_v4();
    let (stop, stopped) = oneshot::channel();
    let previous = sign_ins().insert(
        key.clone(),
        SignIn {
            id,
            input,
            stop: Some(stop),
            page: None,
            by_code,
            ended,
        },
    );
    let entry = Entry {
        key: key.clone(),
        id,
    };
    if let Some(stop) = previous.and_then(|previous| previous.stop) {
        let _ = stop.send(());
    }

    let (lines_tx, mut lines) = mpsc::unbounded_channel::<String>();
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
    // Removing the account while its sign-in was starting found none to stop.
    account.added_antigravity()?;
    let watch = async {
        tokio::select! {
            signed_in = async {
                if let (Some(server), Some(asking)) = (&server, &mut asking) {
                    return in_browser(server, asking, &mut lines).await;
                }
                while let Some(line) = lines.recv().await {
                    if !shown_url {
                        if let Some(url) = first_url(&line) {
                            if let Some(store) = &store {
                                if !store.confirmed().await {
                                    return Err(CoreError::from(antigravity::WRONG_STORE));
                                }
                            }
                            shown_url = true;
                            let mut sign_ins = sign_ins();
                            if let Some(own) = (sign_ins.get_mut(&key)).filter(|own| own.id == id) {
                                own.page = Some(Page {
                                    agent: account.agent.as_str().to_owned(),
                                    config_path: account.config_path.clone(),
                                    url: url.to_owned(),
                                });
                            }
                            drop(sign_ins);
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
                Ok(child.wait().await.is_ok_and(|status| status.success()))
            } => Some(signed_in),
            _ = stopped => None,
        }
    };
    let ended = tokio::time::timeout(SIGN_IN_TIMEOUT, watch).await;
    let _ = child.kill().await;
    drop(entry);
    pages_changed(core);
    match ended {
        Ok(Some(Err(refused))) => Err(refused),
        Ok(Some(Ok(true))) => {
            if let Some(added) = added {
                if let Err(removed) = account.added_antigravity() {
                    let _ = added.forget_sign_in();
                    return Err(removed);
                }
                if !added.signed_in() {
                    return Err("Sign-in did not finish".into());
                }
                added
                    .keep_private()
                    .map_err(|_| "Could not keep this account's sign-in private")?;
            }
            signed_in(core, account);
            Ok(())
        }
        // What agy prints can hold its sign-in link.
        Ok(Some(_)) if added.is_some() => Err("Sign-in did not finish".into()),
        Ok(Some(_)) => Err(match tail.last() {
            Some(last) => format!("Sign-in did not finish: {last}"),
            None => "Sign-in did not finish".into(),
        }
        .into()),
        Ok(None) => Err("Sign-in stopped".into()),
        Err(_) => Err("Sign-in timed out".into()),
    }
}

/// Signs in through agy's agent server, which opens the page itself and answers once the page
/// has come back to it.
async fn in_browser(
    server: &antigravity::OnAccount,
    input: &mut ChildStdin,
    lines: &mut mpsc::UnboundedReceiver<String>,
) -> CoreResult<bool> {
    const STARTED: u64 = 1;
    const SIGNED_IN: u64 = 2;
    let hello = json!({ "protocolVersion": 1, "clientCapabilities": {} });
    ask(input, STARTED, "initialize", hello).await?;
    while let Some(line) = lines.recv().await {
        let Ok(said) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if said.get("method").is_some() {
            continue;
        }
        match said.get("id").and_then(Value::as_u64) {
            Some(STARTED) if offers_browser(&said) => {
                let method = json!({ "methodId": antigravity::BROWSER_SIGN_IN });
                ask(input, SIGNED_IN, "authenticate", method).await?;
                if !server.confirmed().await {
                    return Err(antigravity::WRONG_STORE.into());
                }
            }
            Some(SIGNED_IN) => return Ok(said.get("error").is_none() && server.signed_in().await),
            Some(STARTED) => return Ok(false),
            _ => {}
        }
    }
    Ok(false)
}

fn offers_browser(said: &Value) -> bool {
    let methods = said
        .pointer("/result/authMethods")
        .and_then(Value::as_array);
    methods.is_some_and(|methods| {
        let mut ids = methods.iter().filter_map(|method| method.get("id"));
        ids.any(|id| id == antigravity::BROWSER_SIGN_IN)
    })
}

async fn ask(input: &mut ChildStdin, id: u64, method: &str, params: Value) -> CoreResult<()> {
    let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let written = input.write_all(format!("{line}\n").as_bytes()).await;
    written.map_err(|_| CoreError::from("Could not start the Antigravity sign-in"))
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

/// [`cancel`], answered once the sign-in has ended.
pub(super) async fn stop(account: Account) {
    let ended = (sign_ins().get(&account.key())).map(|sign_in| sign_in.ended.clone());
    cancel(account);
    if let Some(mut ended) = ended {
        let _ = ended.changed().await;
    }
}

pub(super) async fn sign_out(account: Account) -> CoreResult<()> {
    account.check()?;
    if let Some(added) = account.added_antigravity()? {
        return Ok(added
            .forget_sign_in()
            .map_err(|_| "Could not remove this account's sign-in")?);
    }
    let args = account
        .agent
        .ops()
        .account_sign_out()
        .ok_or(crate::agent_accounts::NO_ACCOUNTS)?;
    let mut command = account.command().await?;
    command.args(args);
    command
        .stdin(sikemux_process::command::Stdio::null())
        .stdout(sikemux_process::command::Stdio::null())
        .stderr(sikemux_process::command::Stdio::null());
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
