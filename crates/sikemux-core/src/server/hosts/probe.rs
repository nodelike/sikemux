//! What runs on an SSH host, asked over the person's own SSH (ADR 0018, ADR 0019): whether it
//! runs Sikemux, on this host's account or another, and which host it is, so the host shows as
//! one row and every client offers the one thing that sets it up. One `sh` script goes in on
//! ssh's stdin: it reads the runtime file the host's core keeps, else asks the core's command at
//! the places Sikemux installs it. The question never prompts: with no key the SSH config
//! already offers, it answers that ssh needs signing in to.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
pub use sikemux_calls::hosts::{Found, Unanswered};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sikemux_process::sync::Locked;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use super::super::changed::Topic;
use super::super::Core;

/// Long enough to connect and ask.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// How soon an alias is asked again when a terminal on it reconnects.
const ASKED_RECENTLY: Duration = Duration::from_secs(15);
/// How soon an alias is asked again when the hosts are shown.
const SHOWN_RECENTLY: Duration = Duration::from_secs(5 * 60);
/// How much of what ssh printed to stderr a failure keeps.
const STDERR_TAIL: usize = 400;

/// Sets `c` to the core's command at the places Sikemux installs it, then on PATH. The core's
/// own name goes first: on a package install `sikemux` on PATH is the window.
const FIND_CORE: &str = r#"c=
for x in "$HOME/.config/sikemux/bin/sikemux" /usr/local/lib/sikemux/sikemux-host /usr/lib/sikemux/sikemux-host "/Applications/Sikemux.app/Contents/Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host" "$HOME/Applications/Sikemux.app/Contents/Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host" "$HOME/.local/bin/sikemux" "$(command -v sikemux-host 2>/dev/null)" "$(command -v sikemux 2>/dev/null)"; do
  if [ -n "$x" ] && [ -x "$x" ]; then c=$x; break; fi
done
"#;

const RUNTIME_MARK: &str = "sikemux-runtime";
const STATUS_MARK: &str = "sikemux-status";
const NONE_MARK: &str = "sikemux-none";

fn probe_script() -> String {
    format!(
        r#"r="$HOME/.config/sikemux/core.sock.runtime.json"
if [ -r "$r" ] && [ -S "$HOME/.config/sikemux/core.sock" ]; then
  pid=$(sed -n 's/.*"pid"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$r" | head -n 1)
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    case "$(ps -o stat= -o comm= -p "$pid" 2>/dev/null)" in
      Z*) ;;
      *sikemux*) printf '\n{RUNTIME_MARK}\n'; cat "$r"; printf '\n'; exit 0 ;;
    esac
  fi
fi
{FIND_CORE}if [ -n "$c" ]; then
  printf '\n{STATUS_MARK}\n'
  "$c" status --json </dev/null 2>/dev/null
  printf '\n'
  exit 0
fi
printf '\n{NONE_MARK}\n'
"#
    )
}

/// A script that runs the core's command with `args`, or fails saying Sikemux is not there.
pub(super) fn core_command(args: &str) -> String {
    format!(
        "{FIND_CORE}[ -n \"$c\" ] || {{ echo 'Sikemux is not installed there' >&2; exit 127; }}\n{}\n",
        args.replace("$core", "\"$c\"")
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Found(Found),
    Unanswered(Unanswered),
}

/// The host a probe reached, when it said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reached {
    pub(crate) key: String,
}

#[derive(Clone)]
pub(crate) struct Probe {
    pub(crate) answer: Answer,
    pub(crate) reached: Option<Reached>,
    at: Instant,
}

#[derive(Default)]
pub(crate) struct Probes {
    answers: HashMap<String, Probe>,
    asking: HashSet<String>,
}

/// Why an alias is asked, which sets how recently asked is recent enough.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// A set-up ended or the host's links changed: always ask.
    Changed,
    /// A terminal on it connected.
    Connected,
    /// The hosts are shown.
    Shown,
}

/// Asks the SSH host `alias` what it runs, unless that is being asked now or was answered
/// recently enough for `why`.
pub(crate) fn ask(core: &Arc<Core>, alias: &str, why: Why) {
    {
        let mut probes = core.hosts().probes.locked();
        if probes.asking.contains(alias) {
            return;
        }
        let recent = match why {
            Why::Changed => Duration::ZERO,
            Why::Connected => ASKED_RECENTLY,
            Why::Shown => SHOWN_RECENTLY,
        };
        if probes
            .answers
            .get(alias)
            .is_some_and(|probe| probe.at.elapsed() < recent)
        {
            return;
        }
        probes.asking.insert(alias.to_owned());
    }
    core.changed(What::Read(Topic::Hosts));
    let (core, alias) = (core.clone(), alias.to_owned());
    tokio::spawn(async move {
        let probe = probe(&core, &alias).await;
        let same = match (&probe.answer, &probe.reached) {
            (Answer::Found(Found::Same), Some(reached)) => Some(reached.key.clone()),
            _ => None,
        };
        let mut probes = core.hosts().probes.locked();
        probes.asking.remove(&alias);
        probes.answers.insert(alias.clone(), probe);
        drop(probes);
        if let Some(key) = same {
            super::listed::note_reached(&core, key, alias);
        }
        core.changed(What::Read(Topic::Hosts));
    });
}

/// What `alias` was last found to run, or why it has no answer.
pub(crate) fn answer(core: &Core, alias: &str) -> Answer {
    core.hosts()
        .probes
        .locked()
        .answers
        .get(alias)
        .map_or(Answer::Unanswered(Unanswered::Checking), |probe| {
            probe.answer.clone()
        })
}

/// The host key `alias` was last found to reach.
pub(crate) fn reached(core: &Core, alias: &str) -> Option<String> {
    let probes = core.hosts().probes.locked();
    Some(probes.answers.get(alias)?.reached.as_ref()?.key.clone())
}

async fn probe(core: &Arc<Core>, alias: &str) -> Probe {
    let ran = run(alias, &probe_script(), PROBE_TIMEOUT, |_| {}).await;
    let (answer, reached) = read(&ran, core.remote().owner().as_deref());
    Probe {
        answer,
        reached,
        at: Instant::now(),
    }
}

/// How a script run over ssh ended.
pub(crate) struct Ran {
    /// None when it did not end in time or could not start.
    pub(crate) code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Ran {
    /// ssh's own failure, its stderr, or the script's, by its exit status.
    pub(crate) fn failure(&self) -> Option<Unanswered> {
        let detail = tail(&self.stderr);
        match self.code {
            Some(0) => None,
            None if self.stderr.is_empty() => Some(Unanswered::Unreachable {
                detail: "it did not answer in time".into(),
            }),
            None => Some(Unanswered::Unreachable { detail }),
            Some(255) if needs_ssh_sign_in(&self.stderr) => Some(Unanswered::SshSignIn { detail }),
            Some(255) => Some(Unanswered::Unreachable { detail }),
            Some(code) => Some(Unanswered::Failed {
                detail: if detail.is_empty() {
                    format!("it exited with code {code}")
                } else {
                    detail
                },
            }),
        }
    }
}

/// ssh in batch mode ends with one of these when it needs the person to sign in.
fn needs_ssh_sign_in(stderr: &str) -> bool {
    ["Permission denied", "Host key verification failed"]
        .iter()
        .any(|said| stderr.contains(said))
}

pub(crate) fn tail(text: &str) -> String {
    let text = text.trim();
    let start = text
        .char_indices()
        .rev()
        .nth(STDERR_TAIL)
        .map_or(0, |(at, _)| at);
    text[start..].to_owned()
}

/// Runs `script` on `alias` with `sh`, without a terminal, handing each line it prints to
/// `line` as it comes.
pub(crate) async fn run(
    alias: &str,
    script: &str,
    timeout: Duration,
    line: impl FnMut(&str),
) -> Ran {
    run_until(alias, script, timeout, line, std::future::pending()).await
}

/// [`run`], ended early once `stop` resolves.
pub(crate) async fn run_until(
    alias: &str,
    script: &str,
    timeout: Duration,
    mut line: impl FnMut(&str),
    stop: impl std::future::Future<Output = ()>,
) -> Ran {
    let mut ssh = tokio::process::Command::from(sikemux_process::user_environment::command("ssh"));
    ssh.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ControlMaster=no",
        "-o",
        "ConnectTimeout=10",
        "--",
        alias,
        "sh -s",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    let mut child = match ssh.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ran {
                code: None,
                stdout: String::new(),
                stderr: format!("ssh did not start: {error}"),
            }
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let script = script.to_owned();
        tokio::spawn(async move {
            let _ = stdin.write_all(script.as_bytes()).await;
        });
    }
    let mut stderr = child.stderr.take();
    let stderr = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.read_to_string(&mut text).await;
        }
        text
    });
    let mut stdout = String::new();
    let reading = async {
        if let Some(out) = child.stdout.take() {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(next)) = lines.next_line().await {
                line(&next);
                stdout.push_str(&next);
                stdout.push('\n');
            }
        }
        child.wait().await.ok().and_then(|status| status.code())
    };
    let code = tokio::select! {
        ended = tokio::time::timeout(timeout, reading) => ended.ok().flatten(),
        () = stop => None,
    };
    let _ = child.start_kill();
    let stderr = tokio::time::timeout(Duration::from_secs(1), stderr)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    Ran {
        code,
        stdout,
        stderr,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Said {
    /// Missing from the runtime file, which only a running core keeps.
    running: Option<bool>,
    /// Missing from releases before the probe, and while the core is stopped.
    core_id: Option<String>,
    account: Option<String>,
    /// Only in the runtime file.
    protocol: Option<u32>,
    /// Only from releases whose `login --json` signs a host in from here.
    #[serde(default)]
    login_json: bool,
}

/// What a probe's run says, for a host whose account is `owner`.
fn read(ran: &Ran, owner: Option<&str>) -> (Answer, Option<Reached>) {
    if let Some(failure) = ran.failure() {
        return (Answer::Unanswered(failure), None);
    }
    let marked = |mark: &str| {
        ran.stdout
            .lines()
            .position(|line| line.trim() == mark)
            .map(|at| {
                ran.stdout
                    .lines()
                    .skip(at + 1)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    };
    if marked(NONE_MARK).is_some() {
        return (Answer::Found(Found::Nothing), None);
    }
    let runtime = marked(RUNTIME_MARK).and_then(|after| first_object(&after));
    let from_runtime = runtime.is_some();
    let said = runtime.or_else(|| marked(STATUS_MARK).and_then(|after| first_object(&after)));
    let Some(said) = said else {
        return (
            Answer::Unanswered(Unanswered::Failed {
                detail: "it answered with something other than Sikemux's status".into(),
            }),
            None,
        );
    };
    if said.running == Some(false) {
        return (Answer::Found(Found::Stopped), None);
    }
    let too_old = said
        .protocol
        .is_some_and(|protocol| protocol < sikemux_wire::protocol::OLDEST_PROTOCOL_VERSION);
    let Some(key) = said.core_id.filter(|_| !too_old) else {
        return (Answer::Found(Found::Outdated), None);
    };
    let found = match said.account {
        // A release whose status does not say it has `login --json` cannot be signed in from
        // here until it is updated.
        None if !from_runtime && !said.login_json => Found::Outdated,
        None => Found::SignedOut,
        Some(account) if Some(account.as_str()) == owner => Found::Same,
        Some(_) => Found::Other,
    };
    (Answer::Found(found), Some(Reached { key }))
}

/// The first JSON object in `text`, whatever a login banner put around it.
fn first_object(text: &str) -> Option<Said> {
    let start = text.find('{')?;
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Said>()
        .next()?
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ran(code: Option<i32>, stdout: &str, stderr: &str) -> Ran {
        Ran {
            code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    // temple, on the account and connected, was offered Install: `sikemux` was not on its
    // PATH, the probe's command exited 127, and anything unreadable counted as nothing there.
    #[test]
    fn only_a_confirmed_nothing_offers_install() {
        let missing = ran(Some(127), "", "bash: line 1: sikemux: command not found\n");
        assert!(matches!(
            read(&missing, Some("me")).0,
            Answer::Unanswered(Unanswered::Failed { .. })
        ));
        let garbage = ran(Some(0), "Welcome to temple\n", "");
        assert!(matches!(
            read(&garbage, Some("me")).0,
            Answer::Unanswered(Unanswered::Failed { .. })
        ));
        let refused = ran(
            Some(255),
            "",
            "nodelike@temple: Permission denied (publickey).\n",
        );
        assert!(matches!(
            read(&refused, Some("me")).0,
            Answer::Unanswered(Unanswered::SshSignIn { .. })
        ));
        let away = ran(
            Some(255),
            "",
            "ssh: connect to host temple port 22: No route to host\n",
        );
        assert!(matches!(
            read(&away, Some("me")).0,
            Answer::Unanswered(Unanswered::Unreachable { .. })
        ));
        let none = ran(Some(0), "\nsikemux-none\n", "");
        assert_eq!(read(&none, Some("me")).0, Answer::Found(Found::Nothing));
    }

    #[test]
    fn the_linked_key_is_read_through_a_banner() {
        let status = ran(
            Some(0),
            "Last login: today\n\nsikemux-status\n{\n  \"running\": true,\n  \"coreId\": \"abc\",\n  \"account\": \"me\"\n}\n",
            "",
        );
        assert_eq!(
            read(&status, Some("me")),
            (
                Answer::Found(Found::Same),
                Some(Reached { key: "abc".into() })
            )
        );
        let older = ran(
            Some(0),
            "\nsikemux-status\n{\"running\": true, \"coreId\": \"abc\", \"account\": null}\n",
            "",
        );
        assert_eq!(read(&older, Some("me")).0, Answer::Found(Found::Outdated));
        let newer = ran(
            Some(0),
            "\nsikemux-status\n{\"running\": true, \"coreId\": \"abc\", \"account\": null, \"loginJson\": true}\n",
            "",
        );
        assert_eq!(read(&newer, Some("me")).0, Answer::Found(Found::SignedOut));
        let runtime = ran(
            Some(0),
            "\nsikemux-runtime\n{\"pid\": 7,\"coreId\":\"abc\",\"protocol\":99,\"account\":null}\n",
            "",
        );
        assert_eq!(
            read(&runtime, Some("me")).0,
            Answer::Found(Found::SignedOut)
        );
    }
}
