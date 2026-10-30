//! Where each agent CLI keeps its settings and saved sessions.

use std::collections::BTreeMap;
use std::path::PathBuf;

use sikemux_process::paths::home;
use sikemux_process::user_environment::shell_var;
use sikemux_pty::launch::account_dir;

use crate::agent_catalog::AgentKind;

pub fn environment_path(key: &str) -> Option<PathBuf> {
    shell_var(key).map(PathBuf::from)
}

impl AgentKind {
    /// The folder the agent keeps its settings and sessions in, where the
    /// person's shell points it.
    pub fn home_root(self) -> Option<PathBuf> {
        account_root(self, None, &BTreeMap::new())
    }
}

/// The folder `kind` keeps its settings and sessions in: the account folder
/// `config_path` names, else the one `env` or the person's shell names, else
/// the agent's default.
pub fn account_root(
    kind: AgentKind,
    config_path: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    if let Some(path) = config_path.map(str::trim).filter(|path| !path.is_empty()) {
        return Some(account_dir(path));
    }
    let Some(folder) = kind.entry().home.as_ref() else {
        return kind.ops().home();
    };
    let root = env
        .get(folder.variable)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| environment_path(folder.variable))
        .or_else(|| Some(home()?.join(folder.dir)))?;
    Some(match folder.within {
        Some(within) => root.join(within),
        None => root,
    })
}

fn omp_profile_name() -> Option<String> {
    let raw = shell_var("OMP_PROFILE").or_else(|| shell_var("PI_PROFILE"))?;
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
    let config = shell_var("PI_CONFIG_DIR").unwrap_or_else(|| ".omp".to_string());
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

/// Where Muse keeps its sessions and their index.
pub fn muse_data_root() -> Option<PathBuf> {
    environment_path("XDG_DATA_HOME")
        .or_else(|| Some(home()?.join(".local/share")))
        .map(|root| root.join("muse"))
}

pub fn agent_config_root(kind: &str, configured: Option<&str>) -> Option<PathBuf> {
    account_root(AgentKind::from_name(kind)?, configured, &BTreeMap::new())
}
