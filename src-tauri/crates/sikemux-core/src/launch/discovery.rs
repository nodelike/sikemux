//! Finds the program each agent runs as, and checks it starts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sikemux_process::paths::expand_home;
use sikemux_process::sync::Locked;
use sikemux_process::user_environment;
use sikemux_pty::user_shell::login_shell_environment;
use tokio::process::Command;

use crate::agent_catalog::{AgentEntry, AGENTS};

/* Two seconds is what a person waits for an agent binary to name its version
before the probe gives up. A test spawns the same real process while hundreds
of its siblings run beside it, so the shipped budget fails there for being
busy rather than for being broken. A test that means to catch a hanging probe
says its own timeout. */
#[cfg(not(test))]
const AGENT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const AGENT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_UPDATE_RETRY_TIMEOUT: Duration = Duration::from_secs(8);
const AGENT_UPDATE_SETTLE_DELAY: Duration = Duration::from_millis(350);

fn healthy_agent_executables() -> &'static Mutex<HashMap<String, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn push_agent_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_file() && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn automatic_candidates(def: &AgentEntry) -> Vec<PathBuf> {
    let mut candidates = user_environment::find_executables(def.command);
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
    let mut command = Command::from(user_environment::command(executable));
    command
        .args(def.probe)
        .envs(login_shell_environment())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| "version check timed out".to_string())?
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

#[derive(Clone)]
struct AgentProbe {
    checked_at: std::time::Instant,
    modified: std::time::SystemTime,
    size: u64,
    version: String,
}

async fn probe_agent_executable(def: &AgentEntry, executable: &Path) -> Result<String, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, AgentProbe>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let metadata = fs::metadata(executable).ok();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok());
    let size = metadata.as_ref().map_or(0, |metadata| metadata.len());
    if let Some(cached) = cache.locked().get(executable).cloned() {
        if cached.checked_at.elapsed() < Duration::from_secs(60)
            && Some(cached.modified) == modified
            && cached.size == size
        {
            return Ok(cached.version);
        }
    }
    let version = probe_agent_executable_with_timeout(def, executable, AGENT_PROBE_TIMEOUT).await?;
    if let Some(modified) = modified {
        let mut cache = cache.locked();
        if cache.len() >= 128 {
            cache.retain(|_, probe| probe.checked_at.elapsed() < Duration::from_secs(60));
        }
        cache.insert(
            executable.to_path_buf(),
            AgentProbe {
                checked_at: std::time::Instant::now(),
                modified,
                size,
                version: version.clone(),
            },
        );
    }
    Ok(version)
}

pub async fn first_healthy(
    def: &AgentEntry,
    candidates: Vec<PathBuf>,
) -> (Option<PathBuf>, Vec<String>) {
    let mut failures = Vec::new();
    let cached = healthy_agent_executables().locked().get(def.kind).cloned();
    let mut temporarily_unverified = None;
    for candidate in candidates {
        match probe_agent_executable(def, &candidate).await {
            Ok(_) => {
                healthy_agent_executables()
                    .locked()
                    .insert(def.kind.to_string(), candidate.clone());
                return (Some(candidate), failures);
            }
            Err(error) if error == "version check timed out" => {
                tokio::time::sleep(AGENT_UPDATE_SETTLE_DELAY).await;
                match probe_agent_executable_with_timeout(
                    def,
                    &candidate,
                    AGENT_UPDATE_RETRY_TIMEOUT,
                )
                .await
                {
                    Ok(_) => {
                        healthy_agent_executables()
                            .locked()
                            .insert(def.kind.to_string(), candidate.clone());
                        return (Some(candidate), failures);
                    }
                    Err(retry_error) => {
                        failures.push(format!(
                            "{}: {retry_error} after update retry",
                            candidate.display()
                        ));
                        if cached.as_ref() == Some(&candidate) {
                            temporarily_unverified = Some(candidate);
                        }
                    }
                }
            }
            Err(error) => failures.push(format!("{}: {error}", candidate.display())),
        }
    }
    (temporarily_unverified, failures)
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
