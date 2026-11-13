//! "Install Sikemux", "Update", "Start" and "Sign in" on an SSH host (ADR 0018, ADR 0019), over
//! the person's own SSH. Install runs in a terminal of that host's strip, so a password sudo asks
//! for is typed there and never passes through the core; the rest run without one. A set-up
//! ends when its remote command ends. Signing in runs `login --json` there, whose code the core
//! reads from the pipe and approves for the account; the host is then noted as reached at the
//! alias, and asked afresh what it runs.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use crate::server::serving::ServeCore;
pub use sikemux_calls::hosts::SetupRow;
pub use sikemux_calls::hosts::{AliasParams, SetupEnded};
pub use sikemux_calls::hosts::{InstallParams, SetUpWork};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::sync::{mpsc, oneshot, watch};

use super::super::account::devices::{approve_host_sign_in, SignInCodeParams};
use super::super::changed::Topic;
use super::super::{session, spare, Core, CoreResult};
use super::probe::{self, Why};
use sikemux_wire::protocol::SessionId;

/// Long enough to download a release and hand the core over to it.
const UPDATE_WAIT: Duration = Duration::from_secs(10 * 60);
/// Long enough to sign in once the code is approved.
const SIGN_IN_WAIT: Duration = Duration::from_secs(3 * 60);
const START_WAIT: Duration = Duration::from_secs(60);
/// Printed before `login --json`, so what a shell's startup files print is not read as its answer.
const LOGIN_MARK: &str = "sikemux-login";

/// What is being done to an SSH host from here, kept until the next set-up of that host so a
/// caller that asks late still hears how it ended.
pub(crate) struct Setup {
    work: SetUpWork,
    /// Which set-up of the host this is, so a late end never settles a newer one.
    run: u64,
    ended: watch::Sender<Option<Done>>,
}

pub(crate) trait SetupRowExt: Sized {
    /// Connecting while it runs; out of reach once it has stopped short.
    fn reach(&self) -> super::listed::Reach;
}

impl SetupRowExt for SetupRow {
    fn reach(&self) -> super::listed::Reach {
        match self.problem {
            None => super::listed::Reach::Connecting,
            Some(_) => super::listed::Reach::Away,
        }
    }
}

/// How a set-up ended: the name of the host it added to the account, or why it stopped short.
type Done = Result<Option<String>, String>;

/// An alias from ~/.ssh/config, never something ssh would read as an option.
fn checked_alias(alias: &str) -> CoreResult<&str> {
    let plain = !alias.is_empty()
        && !alias.starts_with('-')
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'));
    if plain {
        Ok(alias)
    } else {
        Err("that is not an SSH host from ~/.ssh/config".into())
    }
}

/// Starts `work` on the SSH host `alias`, and answers with the terminal it runs in, when it
/// needs one. `hostSetup` waits for the end, and the host's row says how it went.
pub(crate) async fn install_host(core: &Arc<Core>, params: InstallParams) -> CoreResult<Value> {
    let alias = checked_alias(&params.alias)?.to_owned();
    let signs_in = matches!(params.work, SetUpWork::Install | SetUpWork::SignIn);
    if signs_in && core.remote().owner().is_none() {
        return Err(format!(
            "this host is not signed in to your Sikemux account, so it cannot add {alias} to it"
        )
        .into());
    }
    let run = {
        let mut setups = core.hosts().setups.locked();
        if setups
            .get(&alias)
            .is_some_and(|setup| setup.ended.borrow().is_none())
        {
            return Err(format!("{alias} is already being set up").into());
        }
        let run = setups.get(&alias).map_or(0, |setup| setup.run + 1);
        setups.insert(
            alias.clone(),
            Setup {
                work: params.work,
                run,
                ended: watch::channel(None).0,
            },
        );
        run
    };
    core.changed(What::Read(Topic::Hosts));
    let terminal = match params.work {
        SetUpWork::Install => {
            let startup = format!("ssh -t -- {alias} {}", single_quoted(&installer()));
            let started = async {
                let id = spare::setup_shell(core, &alias, startup).await?;
                let tab = super::super::layout::show_ssh_terminal(core, alias.clone(), id)?;
                CoreResult::Ok((id, tab))
            }
            .await;
            match started {
                Ok(started) => Some(started),
                Err(error) => {
                    settle(core, &alias, run, Err(error.to_string()));
                    return Err(error);
                }
            }
        }
        _ => None,
    };
    let (core, work, id) = (
        core.clone(),
        params.work,
        terminal.as_ref().map(|(id, _)| *id),
    );
    tokio::spawn(async move {
        let ended = set_up(&core, &alias, work, id).await;
        settle(&core, &alias, run, ended);
        probe::ask(&core, &alias, Why::Changed);
    });
    Ok(match terminal {
        Some((id, tab)) => json!({ "terminal": id, "tab": tab }),
        None => json!({ "terminal": null }),
    })
}

pub(crate) async fn host_setup(core: &Arc<Core>, params: AliasParams) -> CoreResult<SetupEnded> {
    let mut ended = core
        .hosts()
        .setups
        .locked()
        .get(&params.alias)
        .map(|setup| setup.ended.subscribe())
        .ok_or("nothing has been set up on that host")?;
    let ended = ended
        .wait_for(Option::is_some)
        .await
        .map_err(|_| "the set-up was dropped")?
        .clone();
    Ok(match ended {
        Some(Err(problem)) => SetupEnded {
            problem: Some(problem),
            added: None,
        },
        Some(Ok(added)) => SetupEnded {
            problem: None,
            added,
        },
        None => SetupEnded {
            problem: None,
            added: None,
        },
    })
}

/// Takes a set-up that stopped short off its host's row.
pub(crate) fn dismiss_host_setup(core: &Core, params: AliasParams) -> CoreResult<()> {
    let mut setups = core.hosts().setups.locked();
    if setups
        .get(&params.alias)
        .is_some_and(|setup| setup.ended.borrow().is_none())
    {
        return Err("that set-up is still running".into());
    }
    setups.remove(&params.alias);
    drop(setups);
    core.changed(What::Read(Topic::Hosts));
    Ok(())
}

/// The set-ups that are running or stopped short, by alias, for the hosts' rows.
pub(crate) fn rows(core: &Core) -> Vec<(String, SetupRow)> {
    core.hosts()
        .setups
        .locked()
        .iter()
        .filter_map(|(alias, setup)| {
            let problem = match &*setup.ended.borrow() {
                None => None,
                Some(Err(problem)) => Some(problem.clone()),
                Some(Ok(_)) => return None,
            };
            Some((
                alias.clone(),
                SetupRow {
                    work: setup.work,
                    problem,
                },
            ))
        })
        .collect()
}

/// Ends set-up `run` of `alias`, unless a newer one has begun.
fn settle(core: &Core, alias: &str, run: u64, ended: Done) {
    if let Some(setup) = core
        .hosts()
        .setups
        .locked()
        .get_mut(alias)
        .filter(|setup| setup.run == run)
    {
        setup.ended.send_replace(Some(ended));
    }
    core.changed(What::Read(Topic::Hosts));
}

async fn set_up(
    core: &Arc<Core>,
    alias: &str,
    work: SetUpWork,
    terminal: Option<SessionId>,
) -> Done {
    match work {
        SetUpWork::Install => {
            let session = terminal
                .and_then(|id| core.session(id))
                .ok_or("the set-up terminal ended")?;
            let exit = session::ended(&session).await;
            match exit.and_then(|exit| exit.code) {
                Some(0) => sign_in(core, alias).await,
                Some(code) => Err(format!(
                    "the installer exited with code {code}; its terminal shows why"
                )),
                None => Err("the set-up terminal was closed before it finished".into()),
            }
        }
        SetUpWork::SignIn => sign_in(core, alias).await,
        SetUpWork::Update => {
            let ran = probe::run(
                alias,
                &probe::core_command("$core restart && $core update"),
                UPDATE_WAIT,
                |_| {},
            )
            .await;
            ended(&ran).map(|()| None)
        }
        SetUpWork::Start => {
            let ran = probe::run(
                alias,
                &probe::core_command("$core start"),
                START_WAIT,
                |_| {},
            )
            .await;
            ended(&ran).map(|()| None)
        }
    }
}

fn ended(ran: &probe::Ran) -> Result<(), String> {
    match ran.failure() {
        None => Ok(()),
        Some(
            probe::Unanswered::Unreachable { detail }
            | probe::Unanswered::SshSignIn { detail }
            | probe::Unanswered::Failed { detail },
        ) => Err(detail),
        Some(probe::Unanswered::Checking) => Err("it did not answer".into()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginLine {
    code: Option<String>,
    signed_in: Option<bool>,
    account: Option<String>,
}

/// What a `login` from before `login --json` prints in front of its code.
const OLD_LOGIN_PROMPT: &str = "enter the code";

/// Runs `login --json` on `alias`, approving the one code it prints for this host's account,
/// and answers with the name the host was added under. The run ends at once when the approval
/// fails, when it prints a second code, or when it prompts as a release before `login --json`
/// does, polling for a code nobody here approves.
async fn sign_in(core: &Arc<Core>, alias: &str) -> Done {
    let (lines, mut said) = mpsc::unbounded_channel::<String>();
    let (stop, stopped) = oneshot::channel::<()>();
    let script = probe::core_command(&format!("echo {LOGIN_MARK}; $core login --json"));
    let running = probe::run_until(
        alias,
        &script,
        SIGN_IN_WAIT,
        move |line| {
            let _ = lines.send(line.to_owned());
        },
        async {
            let _ = stopped.await;
        },
    );
    let reading = async {
        let mut stop = Some(stop);
        let mut added: Option<String> = None;
        let mut approved = false;
        let mut outcome: Option<Result<(), String>> = None;
        let mut marked = false;
        while let Some(line) = said.recv().await {
            let line = line.trim();
            if !marked {
                marked = line == LOGIN_MARK;
                continue;
            }
            let read = line
                .starts_with('{')
                .then(|| serde_json::from_str::<LoginLine>(line).ok())
                .flatten();
            let failed = match read {
                None if line.contains(OLD_LOGIN_PROMPT) => Some(format!(
                    "the Sikemux on {alias} is too old to be signed in from here; update it, then sign in"
                )),
                None => None,
                Some(LoginLine {
                    code: Some(_), ..
                }) if approved => Some(format!(
                    "{alias} printed a second sign-in code, which was not approved"
                )),
                Some(LoginLine {
                    code: Some(code), ..
                }) => {
                    approved = true;
                    match approve_host_sign_in(core, SignInCodeParams::new(code)).await {
                        Ok(host) => {
                            added = Some(host.name);
                            super::listed::note_installed(core, host.key, alias.to_owned());
                            None
                        }
                        Err(error) => Some(error.to_string()),
                    }
                }
                Some(LoginLine {
                    signed_in: Some(true),
                    account,
                    ..
                }) => {
                    outcome = Some(match account {
                        Some(account) if Some(&account) != core.remote().owner().as_ref() => {
                            Err(format!("{alias} is signed in to another Sikemux account"))
                        }
                        _ => Ok(()),
                    });
                    None
                }
                Some(_) => None,
            };
            if let Some(failed) = failed {
                outcome = Some(Err(failed));
                if let Some(stop) = stop.take() {
                    let _ = stop.send(());
                }
                break;
            }
        }
        (outcome, added)
    };
    let (ran, (outcome, added)) = tokio::join!(running, reading);
    match outcome {
        Some(Err(failed)) => Err(failed),
        Some(Ok(())) => ended(&ran).map(|()| added),
        None => ended(&ran).and(Err("it ended without saying it signed in".into())),
    }
}

/// The installer for this core's channel, as an SSH host runs it. It leaves signing in to the
/// core, which approves the code itself.
fn installer() -> String {
    let channel = match super::super::host::info().channel {
        BuildChannel::Stable => "stable",
        _ => "nightly",
    };
    format!(
        "curl -fsSL https://sikemux.com/install | SIKEMUX_CHANNEL={channel} SIKEMUX_SIGN_IN=later sh -s -- --host --yes"
    )
}

use sikemux_process::single_quoted;
use sikemux_wire::protocol::BuildChannel;
