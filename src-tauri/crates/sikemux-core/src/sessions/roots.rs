//! Where each agent CLI keeps its settings and saved sessions.

use std::path::PathBuf;

use sikemux_process::paths::home;
use sikemux_pty::launch::account_dir;
use sikemux_pty::user_shell::user_variable;

pub fn environment_path(key: &str) -> Option<PathBuf> {
    user_variable(key).map(PathBuf::from)
}

/// Where Hermes keeps its config and sessions.
pub fn hermes_root() -> Option<PathBuf> {
    environment_path("HERMES_HOME").or_else(|| Some(home()?.join(".hermes")))
}

/// Where Pi keeps its settings and sessions.
pub fn pi_agent_root() -> Option<PathBuf> {
    environment_path("PI_CODING_AGENT_DIR").or_else(|| Some(home()?.join(".pi/agent")))
}

fn omp_profile_name() -> Option<String> {
    let raw = user_variable("OMP_PROFILE").or_else(|| user_variable("PI_PROFILE"))?;
    let name = raw.trim();
    (!name.is_empty()
        && name != "default"
        && name != "."
        && name != ".."
        && name.len() <= 64
        && !name.contains(['/', '\\']))
    .then(|| name.to_string())
}

fn omp_config_root() -> Option<PathBuf> {
    let home = home()?;
    let config = user_variable("PI_CONFIG_DIR").unwrap_or_else(|| ".omp".to_string());
    Some(home.join(config.trim_start_matches(['/', '\\'])))
}

pub fn omp_agent_root() -> Option<PathBuf> {
    if let Some(profile) = omp_profile_name() {
        return Some(
            omp_config_root()?
                .join("profiles")
                .join(profile)
                .join("agent"),
        );
    }
    environment_path("PI_CODING_AGENT_DIR").or_else(|| Some(omp_config_root()?.join("agent")))
}

pub fn omp_session_dirs() -> Vec<PathBuf> {
    if let Some(path) = environment_path("PI_CODING_AGENT_SESSION_DIR") {
        return vec![path];
    }
    let mut dirs = Vec::new();
    if let Some(root) = omp_agent_root() {
        dirs.push(root.join("sessions"));
    }
    if let Some(xdg) = environment_path("XDG_DATA_HOME") {
        let root = xdg.join("omp");
        let sessions = omp_profile_name()
            .map(|profile| root.join("profiles").join(profile).join("sessions"))
            .unwrap_or_else(|| root.join("sessions"));
        if sessions.exists() && !dirs.contains(&sessions) {
            dirs.push(sessions);
        }
    }
    dirs
}

/// Where Muse keeps its settings and sign-in.
pub fn muse_config_root() -> Option<PathBuf> {
    environment_path("XDG_CONFIG_HOME")
        .or_else(|| Some(home()?.join(".config")))
        .map(|root| root.join("muse"))
}

/// Where Muse keeps its sessions and their index.
pub fn muse_data_root() -> Option<PathBuf> {
    environment_path("XDG_DATA_HOME")
        .or_else(|| Some(home()?.join(".local/share")))
        .map(|root| root.join("muse"))
}

pub fn grok_root() -> Option<PathBuf> {
    environment_path("GROK_HOME").or_else(|| Some(home()?.join(".grok")))
}

pub fn agent_config_root(kind: &str, configured: Option<&str>) -> Option<PathBuf> {
    if let Some(configured) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(account_dir(configured));
    }
    let home = home()?;
    match kind {
        "claude" => {
            Some(environment_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")))
        }
        "codex" => Some(environment_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex"))),
        "omp" => omp_agent_root(),
        "grok" => grok_root(),
        "muse" => muse_config_root(),
        _ => Some(home),
    }
}
