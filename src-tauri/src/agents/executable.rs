use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sikemux_pty::user_shell::login_shell_environment;
use tokio::process::Command;

use super::config::{agent_config_root, configured_default_effort, configured_default_model};
use super::models::model_catalog_error_detail;
use super::{allowed_agent_path, AgentDef, AgentInfo, AgentProfileRequest, AGENT_DEFS};

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

pub(super) fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        return crate::system::user_home();
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return crate::system::user_home().join(rest);
    }
    PathBuf::from(value)
}

pub(super) fn apply_process_config(command: &mut Command, agent: &str, config_path: Option<&str>) {
    let Some(root) = agent_config_root(agent, config_path) else {
        return;
    };
    if config_path.is_none() {
        return;
    }
    match agent {
        "codex" => {
            command.env("CODEX_HOME", root);
        }
        "claude" => {
            command.env("CLAUDE_CONFIG_DIR", root);
        }
        _ => {}
    }
}

pub(super) fn apply_login_environment(command: &mut Command) {
    for (key, value) in login_shell_environment() {
        command.env(key, value);
    }
}

fn push_agent_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_file() && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn automatic_agent_candidates(def: &AgentDef) -> Vec<PathBuf> {
    let mut candidates = crate::system::find_executables_matching(def.command, |candidate| {
        allowed_agent_path(def.kind, candidate)
    });
    let home = crate::system::user_home();
    for candidate in [
        home.join(".local/bin").join(def.command),
        home.join("Library/pnpm").join(def.command),
        home.join(".npm/bin").join(def.command),
        home.join(".bun/bin").join(def.command),
    ] {
        push_agent_candidate(&mut candidates, candidate);
    }
    if def.kind == "claude" {
        push_agent_candidate(&mut candidates, home.join(".claude/local/claude"));
        push_agent_candidate(&mut candidates, home.join(".claude/bin/claude"));
    }
    if def.kind == "opencode" {
        push_agent_candidate(&mut candidates, home.join(".opencode/bin/opencode"));
    }
    if def.kind == "grok" {
        push_agent_candidate(&mut candidates, home.join(".grok/bin/grok"));
    }
    #[cfg(target_os = "macos")]
    if def.kind == "codex" {
        push_agent_candidate(
            &mut candidates,
            PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        );
    }
    candidates
}

fn explicit_agent_candidates(value: &str) -> Vec<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }
    if value.contains('/') || value.contains('\\') || value.starts_with('~') {
        return vec![expand_user_path(value)];
    }
    crate::system::find_executables_matching(value, |_| true)
}

/// Arguments that answer "does this CLI run?" without side effects. `--version`
/// suits most agents, but Hermes bundles a `git fetch` update check into it
/// (bounded by its own 10s network timeout, cached for six hours), so every
/// cache miss outran our probe and reported an installed CLI as missing.
/// `--help` exercises the same interpreter and venv without the network.
fn agent_probe_args(agent: &str) -> &'static [&'static str] {
    match agent {
        "hermes" => &["--help"],
        _ => &["--version"],
    }
}

async fn probe_agent_executable_with_timeout(
    agent: &str,
    executable: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let probe_args = agent_probe_args(agent);
    #[cfg(windows)]
    let mut command = if matches!(
        executable.extension().and_then(|value| value.to_str()),
        Some(value) if value.eq_ignore_ascii_case("cmd") || value.eq_ignore_ascii_case("bat")
    ) {
        let mut command = Command::from(sikemux_process::user_environment::command("cmd.exe"));
        command
            .args(["/D", "/S", "/C"])
            .arg(executable)
            .args(probe_args);
        command
    } else {
        let mut command = Command::from(sikemux_process::user_environment::command(executable));
        command.args(probe_args);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::from(sikemux_process::user_environment::command(executable));
        command.args(probe_args);
        command
    };
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    apply_login_environment(&mut command);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| "version check timed out".to_string())?
        .map_err(|error| format!("could not start: {error}"))?;
    if !output.status.success() {
        let detail = model_catalog_error_detail(&output.stderr)
            .or_else(|| model_catalog_error_detail(&output.stdout))
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

async fn probe_agent_executable(agent: &str, executable: &Path) -> Result<String, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, AgentProbe>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let metadata = fs::metadata(executable).ok();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok());
    let size = metadata.as_ref().map_or(0, |metadata| metadata.len());
    if let Some(cached) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(executable).cloned())
    {
        if cached.checked_at.elapsed() < Duration::from_secs(60)
            && Some(cached.modified) == modified
            && cached.size == size
        {
            return Ok(cached.version);
        }
    }
    let version =
        probe_agent_executable_with_timeout(agent, executable, AGENT_PROBE_TIMEOUT).await?;
    if let (Some(modified), Ok(mut cache)) = (modified, cache.lock()) {
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

async fn first_healthy_agent_candidate(
    agent: &str,
    candidates: Vec<PathBuf>,
) -> (Option<PathBuf>, Vec<String>) {
    let mut failures = Vec::new();
    let cached = healthy_agent_executables()
        .lock()
        .ok()
        .and_then(|cache| cache.get(agent).cloned());
    let mut temporarily_unverified = None;
    for candidate in candidates {
        match probe_agent_executable(agent, &candidate).await {
            Ok(_) => {
                if let Ok(mut cache) = healthy_agent_executables().lock() {
                    cache.insert(agent.to_string(), candidate.clone());
                }
                return (Some(candidate), failures);
            }
            Err(error) if error == "version check timed out" => {
                tokio::time::sleep(AGENT_UPDATE_SETTLE_DELAY).await;
                match probe_agent_executable_with_timeout(
                    agent,
                    &candidate,
                    AGENT_UPDATE_RETRY_TIMEOUT,
                )
                .await
                {
                    Ok(_) => {
                        if let Ok(mut cache) = healthy_agent_executables().lock() {
                            cache.insert(agent.to_string(), candidate.clone());
                        }
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

pub(crate) async fn resolve_agent_executable(
    agent: &str,
    explicit: Option<&str>,
) -> Result<PathBuf, String> {
    let def = AGENT_DEFS
        .iter()
        .find(|def| def.kind == agent)
        .ok_or("Unknown agent provider")?;
    let candidates = explicit
        .map(explicit_agent_candidates)
        .unwrap_or_else(|| automatic_agent_candidates(def));
    let (resolved, failures) = first_healthy_agent_candidate(agent, candidates).await;
    resolved.ok_or_else(|| {
        if failures.is_empty() {
            format!("{} executable was not found", def.label)
        } else {
            failures.join("; ")
        }
    })
}

/// Agent CLIs that are installed for the current user. The app's PATH is fixed
/// from the login shell during boot, so this matches what spawned PTYs can run.
#[tauri::command]
pub async fn available_agents(profiles: Vec<AgentProfileRequest>) -> Vec<AgentInfo> {
    let available = futures::future::join_all(AGENT_DEFS.iter().map(|def| async {
        let profile = profiles
            .iter()
            .find(|profile| profile.kind.as_str() == def.kind);
        let explicit = profile.and_then(|profile| profile.executable_path.as_deref());
        let candidates = explicit
            .map(explicit_agent_candidates)
            .unwrap_or_else(|| automatic_agent_candidates(def));
        if candidates.is_empty() && profile.is_none() {
            return None;
        }

        let (resolved, failures) = first_healthy_agent_candidate(def.kind, candidates).await;
        let config_path = profile.and_then(|profile| profile.config_path.clone());
        let warning = resolved.as_ref().and_then(|path| {
            (!failures.is_empty())
                .then(|| format!("Skipped {}; using {}", failures.join("; "), path.display()))
        });
        let error = resolved.is_none().then(|| {
            if failures.is_empty() {
                format!("{} executable was not found", def.label)
            } else {
                failures.join("; ")
            }
        });
        Some(AgentInfo {
            kind: def.kind,
            label: def.label,
            command: resolved
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .or_else(|| explicit.map(str::to_string))
                .unwrap_or_else(|| def.command.to_string()),
            available: resolved.is_some(),
            error,
            warning,
            profile_id: profile.and_then(|profile| profile.profile_id.clone()),
            config_path: config_path.clone(),
            default_model: configured_default_model(def.kind, config_path.as_deref()),
            default_effort: configured_default_effort(def.kind, config_path.as_deref()),
        })
    }))
    .await;
    available.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::{
        first_healthy_agent_candidate, probe_agent_executable, probe_agent_executable_with_timeout,
    };
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

        assert!(probe_agent_executable("codex", &healthy).await.is_ok());
        assert!(probe_agent_executable("codex", &broken)
            .await
            .unwrap_err()
            .contains("saved launcher missing"));

        let (selected, failures) = first_healthy_agent_candidate(
            "codex",
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
            probe_agent_executable("codex", &executable).await.unwrap(),
            "version-one"
        );
        assert_eq!(
            probe_agent_executable("codex", &executable).await.unwrap(),
            "version-one"
        );
        assert_eq!(fs::read_to_string(&count).unwrap(), "x\n");
        fs::write(&executable, "#!/bin/sh\necho version-two-changed\n").unwrap();
        assert_eq!(
            probe_agent_executable("codex", &executable).await.unwrap(),
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
                "claude",
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
            probe_agent_executable("hermes", &executable).await.unwrap(),
            "usage: hermes"
        );
        assert!(
            probe_agent_executable_with_timeout(
                "claude",
                &executable,
                std::time::Duration::from_millis(20)
            )
            .await
            .is_err(),
            "other agents keep probing --version"
        );
    }
}
