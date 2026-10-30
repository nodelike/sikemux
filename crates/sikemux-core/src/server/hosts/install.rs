//! "Install Sikemux", "Update" and "Sign in" on an SSH host (ADR 0018, ADR 0019): the
//! installer, or `sikemux login`, runs over the person's own SSH in a terminal of that host's
//! strip, so a password ssh or sudo asks for is typed there and never passes through the core.
//! Only the sign-in code shown on that terminal's own screen, on the line login prints it on,
//! is approved for the account; the host is then noted as installed from the alias.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::sync::watch;

use super::super::changed::Topic;
use super::super::{session, spare, Core, CoreResult};
use super::account::{self, SignInCodeParams};
use crate::protocol::{BuildChannel, SessionId};

/// Long enough to type a password, download the core and sign in.
const INSTALL_WAIT: Duration = Duration::from_secs(15 * 60);
/// The letters a sign-in code is made of, which the accounts server chooses.
const CODE_LETTERS: &str = "BCDFGHJKLMNPQRSTVWXZ";
/// What `sikemux login` prints in front of the code.
const CODE_LINE: &str = "enter the code";
/// How long, and how often, to look again for a code printed after the words in front of it.
const CODE_SETTLE: Duration = Duration::from_millis(200);
const CODE_SETTLE_TRIES: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Work {
    Install,
    /// Updates a host that runs an older release, then restarts its core; a host signed out
    /// signs in on the way.
    Update,
    SignIn,
    /// Starts the stopped core of a host Sikemux is already on.
    Start,
}

/// What is being done to an SSH host from here, kept until the next set-up of that host so a
/// caller that asks late still hears how it ended.
pub(crate) struct Setup {
    work: Work,
    /// Which set-up of the host this is, so a late end never settles a newer one.
    run: u64,
    ended: watch::Sender<Option<Result<(), String>>>,
}

/// A set-up as the host's row shows it: running, or why it stopped.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetupRow {
    work: Work,
    problem: Option<String>,
}

impl SetupRow {
    /// Connecting while it runs; out of reach once it has stopped short.
    pub(crate) fn reach(&self) -> super::listed::Reach {
        match self.problem {
            None => super::listed::Reach::Connecting,
            Some(_) => super::listed::Reach::Away,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct InstallParams {
    alias: String,
    work: Work,
}

#[derive(Deserialize)]
pub(crate) struct AliasParams {
    alias: String,
}

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

/// Starts `work` on the SSH host `alias` in a terminal of its strip, and answers with that
/// terminal. `hostSetup` waits for the end, and the host's row says how it went.
pub(crate) async fn install_host(core: &Arc<Core>, params: InstallParams) -> CoreResult<Value> {
    let alias = checked_alias(&params.alias)?.to_owned();
    let needs_code = matches!(params.work, Work::Install | Work::SignIn);
    if needs_code && core.remote.owner().is_none() {
        return Err(format!(
            "this host is not signed in to your Sikemux account, so it cannot add {alias} to it"
        )
        .into());
    }
    let run = {
        let mut setups = core.hosts.setups.locked();
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
    core.changes.changed(Topic::Hosts, None, None);
    let remote = match params.work {
        Work::Install => installer(),
        Work::Update => format!("{} && {}", installer(), over_ssh("restart")),
        Work::SignIn => over_ssh("login"),
        Work::Start => over_ssh("start"),
    };
    let startup = format!("ssh -t -- {alias} {}", single_quoted(&remote));
    let started = async {
        let id = spare::setup_shell(core, &alias, startup).await?;
        let tab = super::super::layout::show_ssh_terminal(core, alias.clone(), id)?;
        CoreResult::Ok((id, tab))
    }
    .await;
    let (id, tab) = match started {
        Ok(started) => started,
        Err(error) => {
            settle(core, &alias, run, Err(error.to_string()));
            return Err(error);
        }
    };
    tokio::spawn(finish(core.clone(), id, alias, run, needs_code));
    Ok(json!({ "terminal": id, "tab": tab }))
}

/// Waits for the latest set-up of `alias` to end, and answers how it went.
pub(crate) async fn host_setup(core: &Arc<Core>, params: AliasParams) -> CoreResult<Value> {
    let mut ended = core
        .hosts
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
        Some(Err(problem)) => json!({ "problem": problem }),
        _ => json!({ "problem": null }),
    })
}

/// The set-ups that are running or stopped short, by alias, for the hosts' rows.
pub(crate) fn rows(core: &Core) -> Vec<(String, SetupRow)> {
    core.hosts
        .setups
        .locked()
        .iter()
        .filter_map(|(alias, setup)| {
            let problem = match &*setup.ended.borrow() {
                None => None,
                Some(Err(problem)) => Some(problem.clone()),
                Some(Ok(())) => return None,
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
fn settle(core: &Core, alias: &str, run: u64, ended: Result<(), String>) {
    if let Some(setup) = core
        .hosts
        .setups
        .locked()
        .get_mut(alias)
        .filter(|setup| setup.run == run)
    {
        setup.ended.send_replace(Some(ended));
    }
    core.changes.changed(Topic::Hosts, None, None);
}

async fn finish(core: Arc<Core>, id: SessionId, alias: String, run: u64, needs_code: bool) {
    let ended = approve_shown_code(&core, id, &alias, needs_code).await;
    settle(&core, &alias, run, ended);
    super::probe::ask(&core, &alias, true);
}

async fn approve_shown_code(
    core: &Arc<Core>,
    id: SessionId,
    alias: &str,
    needs_code: bool,
) -> Result<(), String> {
    let session = core.session(id).ok_or("the set-up terminal ended")?;
    let line = tokio::time::timeout(
        INSTALL_WAIT,
        session::line_shown(&session, CODE_LINE.to_owned()),
    )
    .await
    .map_err(|_| "setting it up took too long".to_owned())?
    .unwrap_or_default();
    let mut code = line.as_deref().and_then(sign_in_code);
    // The code can reach the screen a moment after the words in front of it.
    for _ in 0..CODE_SETTLE_TRIES {
        if code.is_some() || line.is_none() {
            break;
        }
        tokio::time::sleep(CODE_SETTLE).await;
        let shown = session::line_shown(&session, CODE_LINE.to_owned())
            .await
            .unwrap_or_default();
        code = shown.as_deref().and_then(sign_in_code);
    }
    session.end_setup();
    let Some(code) = code else {
        return if needs_code {
            Err("it ended without a sign-in code; a host already signed in to your account shows under Hosts as it is".into())
        } else {
            Ok(())
        };
    };
    let host = account::approve_host_sign_in(core, SignInCodeParams::new(code))
        .await
        .map_err(|error| error.to_string())?;
    super::listed::note_installed(core, host.key, alias.to_owned());
    Ok(())
}

/// The sign-in code in a line `sikemux login` printed, if it is that line.
fn sign_in_code(line: &str) -> Option<String> {
    if !line.contains(CODE_LINE) {
        return None;
    }
    line.split(|c: char| !c.is_ascii_uppercase() && c != '-')
        .find(|word| {
            word.len() == 9
                && word.char_indices().all(|(at, c)| {
                    if at == 4 {
                        c == '-'
                    } else {
                        CODE_LETTERS.contains(c)
                    }
                })
        })
        .map(str::to_owned)
}

/// The installer for this core's channel, as an SSH host runs it.
fn installer() -> String {
    let channel = match super::super::host::info().channel {
        BuildChannel::Stable => "stable",
        _ => "nightly",
    };
    format!(
        "curl -fsSL https://sikemux.com/install | SIKEMUX_CHANNEL={channel} sh -s -- --host --yes"
    )
}

/// `sikemux` with `args`, run on an SSH host through its login shell's PATH and where the
/// installer puts it when that PATH does not reach it.
pub(super) fn over_ssh(args: &str) -> String {
    format!(r#"exec "$SHELL" -lc 'PATH="$PATH:$HOME/.local/bin" sikemux {args}'"#)
}

fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
