//! Finds the program each agent runs as, and checks it starts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use sikemux_process::paths::expand_home;
use sikemux_process::sync::Locked;
use sikemux_process::user_environment;
use sikemux_pty::user_shell::login_shell_environment;
use tokio::process::Command;
use tokio::sync::{Notify, Semaphore, SemaphorePermit};

use crate::agent_catalog::{AgentEntry, AGENTS};
use crate::flights::{shared, Flights};

/* Two seconds is what a person waits for an agent binary to name its version
before the probe gives up. A test spawns the same real process while hundreds
of its siblings run beside it, so the shipped budget fails there for being
busy rather than for being broken. A test that means to catch a hanging probe
says its own timeout. */
#[cfg(not(test))]
const AGENT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const AGENT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_RETRY_TIMEOUT: Duration = Duration::from_secs(8);
/// How long after a check timed out it is tried again, each time it times out again.
const RETRY_AFTER: [Duration; 2] = [Duration::from_secs(15), Duration::from_secs(60)];
/// How many agent programs are started at once to answer a question, so
/// listing every agent never starts them all together.
const PROBE_CONCURRENCY: usize = 4;
const MAX_PROBED: usize = 128;
const TIMED_OUT: &str = "version check timed out";

fn push_agent_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_file() && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn automatic_candidates(def: &AgentEntry) -> Vec<PathBuf> {
    let mut candidates = user_environment::find_executables(def.command);
    if user_environment::search_path_only() {
        return candidates;
    }
    let home = sikemux_process::paths::home().unwrap_or_default();
    for candidate in [
        home.join(".local/bin").join(def.command),
        home.join("Library/pnpm").join(def.command),
        home.join(".npm/bin").join(def.command),
        home.join(".bun/bin").join(def.command),
    ] {
        push_agent_candidate(&mut candidates, candidate);
    }
    for path in def.install_paths {
        push_agent_candidate(&mut candidates, expand_home(path));
    }
    candidates
}

fn explicit_candidates(value: &str) -> Vec<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }
    if value.contains('/') || value.contains('\\') || value.starts_with('~') {
        return vec![expand_home(value)];
    }
    user_environment::find_executables(value)
}

/// Hermes's `--version` waits on a networked update check, so the catalog
/// probes it with `--help`.
async fn probe_agent_executable_with_timeout(
    def: &AgentEntry,
    executable: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::from(user_environment::agent_command(executable));
    command
        .args(def.probe)
        .envs(login_shell_environment())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| TIMED_OUT.to_string())?
        .map_err(|error| format!("could not start: {error}"))?;
    if !output.status.success() {
        let detail = error_detail(&output.stderr)
            .or_else(|| error_detail(&output.stdout))
            .unwrap_or_else(|| format!("exited with {}", output.status));
        return Err(detail);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string())
}

/// A turn to start an agent's program to ask it something; held until it answers.
pub(crate) async fn probe_turn() -> Option<SemaphorePermit<'static>> {
    static TURNS: Semaphore = Semaphore::const_new(PROBE_CONCURRENCY);
    TURNS.acquire().await.ok()
}

/// A program file as it is now: when it last changed, and its size.
type FileStamp = (SystemTime, u64);

/// What a program answered, kept until its file changes. A check that timed
/// out is no answer: it is tried again in the background, and until then
/// the program's last good answer stands in for it.
struct Probed {
    stamp: Option<FileStamp>,
    answer: Result<String, String>,
    good: Option<String>,
}

impl Probed {
    fn timed_out(&self) -> bool {
        matches!(&self.answer, Err(error) if error == TIMED_OUT)
    }

    fn shown(&self) -> Result<String, String> {
        match &self.good {
            Some(good) if self.timed_out() => Ok(good.clone()),
            _ => self.answer.clone(),
        }
    }
}

fn probed() -> &'static Mutex<HashMap<PathBuf, Probed>> {
    static PROBED: OnceLock<Mutex<HashMap<PathBuf, Probed>>> = OnceLock::new();
    PROBED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn stamp(executable: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(executable).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Has the programs that failed or timed out asked again on their next look:
/// what they need may have been installed since.
pub fn forget_failures() {
    for probed in probed().locked().values_mut() {
        if probed.answer.is_err() {
            probed.stamp = None;
        }
    }
}

/// Wakes once a program whose check timed out has answered in the background.
pub fn answered_late() -> &'static Notify {
    static ANSWERED: Notify = Notify::const_new();
    &ANSWERED
}

type Probe = (PathBuf, Option<FileStamp>);

/// Asks the program at `executable` and keeps its answer, once however many
/// ask together. A first look that times out is tried again later.
async fn ask(
    def: &'static AgentEntry,
    executable: &Path,
    stamp: Option<FileStamp>,
    timeout: Duration,
) -> Result<String, String> {
    static ASKING: LazyLock<Flights<Probe, Result<String, String>>> =
        LazyLock::new(Flights::default);
    shared(&ASKING, (executable.to_path_buf(), stamp), async {
        let answer = {
            let _turn = probe_turn().await;
            probe_agent_executable_with_timeout(def, executable, timeout).await
        };
        let Some(stamp) = stamp else {
            return answer;
        };
        let mut probed = probed().locked();
        let good = answer
            .clone()
            .ok()
            .or_else(|| probed.get(executable).and_then(|kept| kept.good.clone()));
        if probed.len() >= MAX_PROBED {
            probed.clear();
        }
        let kept = Probed {
            stamp: Some(stamp),
            answer,
            good,
        };
        if kept.timed_out() && timeout == AGENT_PROBE_TIMEOUT {
            retry_later(def, executable.to_path_buf(), stamp);
        }
        let shown = kept.shown();
        probed.insert(executable.to_path_buf(), kept);
        shown
    })
    .await
}

/// Tries a check that timed out again, off any caller's path, since a busy
/// machine or an updater swapping the program says nothing about whether it works.
fn retry_later(def: &'static AgentEntry, executable: PathBuf, stamp: FileStamp) {
    let still_timed_out = move |executable: &Path| {
        probed()
            .locked()
            .get(executable)
            .is_some_and(|kept| kept.stamp == Some(stamp) && kept.timed_out())
    };
    tokio::spawn(async move {
        for wait in RETRY_AFTER {
            tokio::time::sleep(wait).await;
            if !still_timed_out(&executable) {
                return;
            }
            let _ = ask(def, &executable, Some(stamp), AGENT_RETRY_TIMEOUT).await;
            if !still_timed_out(&executable) {
                answered_late().notify_one();
                return;
            }
        }
    });
}

/// What the program at `executable` answers, asked again only once its file changes.
async fn probe_agent_executable(
    def: &'static AgentEntry,
    executable: &Path,
) -> Result<String, String> {
    let stamp = stamp(executable);
    if let Some(kept) = probed().locked().get(executable) {
        if stamp.is_some() && kept.stamp == stamp {
            return kept.shown();
        }
    }
    ask(def, executable, stamp, AGENT_PROBE_TIMEOUT).await
}

pub async fn first_healthy(
    def: &'static AgentEntry,
    candidates: Vec<PathBuf>,
) -> (Option<PathBuf>, Vec<String>) {
    let mut failures = Vec::new();
    for candidate in candidates {
        match probe_agent_executable(def, &candidate).await {
            Ok(_) => return (Some(candidate), failures),
            Err(error) => failures.push(format!("{}: {error}", candidate.display())),
        }
    }
    (None, failures)
}

/// Where to look for `def`'s program: the path or name `explicit` gives, else
/// the user's PATH and the folders its installers use.
pub fn candidates(def: &AgentEntry, explicit: Option<&str>) -> Vec<PathBuf> {
    explicit
        .map(explicit_candidates)
        .unwrap_or_else(|| automatic_candidates(def))
}

/// The first working program for `agent`: the one `explicit` names, else
/// the first found where its installers put it.
pub async fn resolve(agent: &str, explicit: Option<&str>) -> Result<PathBuf, String> {
    let def = AGENTS
        .iter()
        .find(|def| def.kind == agent)
        .ok_or("Unknown agent provider")?;
    let candidates = candidates(def, explicit);
    let (resolved, failures) = first_healthy(def, candidates).await;
    resolved.ok_or_else(|| {
        if failures.is_empty() {
            format!("{} executable was not found", def.label)
        } else {
            failures.join("; ")
        }
    })
}

const ERROR_DETAIL_LIMIT: usize = 240;

/// What a CLI printed when it failed, on one line and cut short.
pub fn error_detail(output: &[u8]) -> Option<String> {
    let said = String::from_utf8_lossy(output);
    let words: Vec<&str> = said
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
        .collect();
    (!words.is_empty()).then(|| crate::text::clipped(&words.join(" "), ERROR_DETAIL_LIMIT))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::{first_healthy, probe_agent_executable, probe_agent_executable_with_timeout};
    use crate::agent_catalog::agent as catalog_agent;

    fn agent(name: &str) -> &'static crate::agent_catalog::AgentEntry {
        catalog_agent(name).unwrap()
    }
    #[cfg(unix)]
    use std::io::Write;

    #[cfg(unix)]
    #[tokio::test]
    async fn executable_probe_rejects_broken_wrappers() {
        use std::os::unix::fs::PermissionsExt;

        let mut healthy = tempfile::NamedTempFile::new().unwrap();
        writeln!(healthy, "#!/bin/sh\nprintf 'codex-cli test\\n'").unwrap();
        let mut permissions = healthy.as_file().metadata().unwrap().permissions();
        permissions.set_mode(0o755);
        healthy.as_file().set_permissions(permissions).unwrap();
        let healthy = healthy.into_temp_path();

        let mut broken = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            broken,
            "#!/bin/sh\nprintf 'saved launcher missing\\n' >&2\nexit 127"
        )
        .unwrap();
        let mut permissions = broken.as_file().metadata().unwrap().permissions();
        permissions.set_mode(0o755);
        broken.as_file().set_permissions(permissions).unwrap();
        let broken = broken.into_temp_path();

        assert!(probe_agent_executable(agent("codex"), &healthy)
            .await
            .is_ok());
        assert!(probe_agent_executable(agent("codex"), &broken)
            .await
            .unwrap_err()
            .contains("saved launcher missing"));

        let (selected, failures) = first_healthy(
            agent("codex"),
            vec![broken.to_path_buf(), healthy.to_path_buf()],
        )
        .await;
        assert_eq!(selected, Some(healthy.to_path_buf()));
        assert_eq!(failures.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executable_probe_reuses_success_and_invalidates_changed_files() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("agent");
        let count = dir.path().join("count");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\necho x >> '{}'\necho version-one\n",
                count.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            probe_agent_executable(agent("codex"), &executable)
                .await
                .unwrap(),
            "version-one"
        );
        assert_eq!(
            probe_agent_executable(agent("codex"), &executable)
                .await
                .unwrap(),
            "version-one"
        );
        assert_eq!(fs::read_to_string(&count).unwrap(), "x\n");
        fs::write(&executable, "#!/bin/sh\necho version-two-changed\n").unwrap();
        assert_eq!(
            probe_agent_executable(agent("codex"), &executable)
                .await
                .unwrap(),
            "version-two-changed"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executable_probe_bounds_temporarily_busy_updaters() {
        use std::os::unix::fs::PermissionsExt;

        let mut busy = tempfile::NamedTempFile::new().unwrap();
        writeln!(busy, "#!/bin/sh\nsleep 1\nprintf 'claude test\\n'").unwrap();
        let mut permissions = busy.as_file().metadata().unwrap().permissions();
        permissions.set_mode(0o755);
        busy.as_file().set_permissions(permissions).unwrap();
        let busy = busy.into_temp_path();

        assert_eq!(
            probe_agent_executable_with_timeout(
                agent("claude"),
                &busy,
                std::time::Duration::from_millis(20)
            )
            .await
            .unwrap_err(),
            "version check timed out"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hermes_probe_skips_the_networked_version_check() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        // Mirrors the real CLI: `--version` blocks on an update check, while
        // `--help` answers immediately.
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("hermes");
        fs::write(
            &executable,
            "#!/bin/sh\ncase \"$1\" in\n--help) printf 'usage: hermes\\n';;\n*) sleep 30;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            probe_agent_executable(agent("hermes"), &executable)
                .await
                .unwrap(),
            "usage: hermes"
        );
        assert!(
            probe_agent_executable_with_timeout(
                agent("claude"),
                &executable,
                std::time::Duration::from_millis(20)
            )
            .await
            .is_err(),
            "other agents keep probing --version"
        );
    }
}
