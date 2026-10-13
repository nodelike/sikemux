// The Slack workspaces signed in here. Each workspace's token lives in the
// Keychain; the file beside it only says which workspaces there are, who is
// signed in to each, and which one a call uses when it names none.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{SlackError, SlackResult};

#[cfg(not(test))]
const TOKEN_SERVICE: &str = "sikemux-slack-token";
/// Tests keep to an entry of their own, so they never replace or delete a real token.
#[cfg(test)]
const TOKEN_SERVICE: &str = "sikemux-slack-token-test";

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Slack's team id, `T…`.
    pub id: String,
    pub name: String,
    /// The workspace's address, `acme.slack.com`, which its message links use.
    pub domain: String,
    pub user_id: String,
    pub user: String,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SlackConfig {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub default: Option<String>,
}

impl SlackConfig {
    /// The workspace named, or with none named, the default one.
    pub fn workspace(&self, id: Option<&str>) -> Option<&Workspace> {
        let wanted = id.filter(|id| !id.is_empty()).or(self.default.as_deref());
        match wanted {
            Some(wanted) => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == wanted),
            None => self.workspaces.first(),
        }
    }

    /// The workspace a message link points into, by its address.
    pub fn by_domain(&self, domain: &str) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.domain.eq_ignore_ascii_case(domain))
    }

    /// Adds the workspace, or replaces the one with the same id. The first becomes the default.
    pub fn upsert(&mut self, workspace: Workspace) {
        match self
            .workspaces
            .iter_mut()
            .find(|kept| kept.id == workspace.id)
        {
            Some(kept) => *kept = workspace,
            None => self.workspaces.push(workspace),
        }
        if !self
            .default
            .as_ref()
            .is_some_and(|id| self.workspaces.iter().any(|kept| &kept.id == id))
        {
            self.default = self
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone());
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<Workspace> {
        let index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)?;
        let removed = self.workspaces.remove(index);
        if self.default.as_deref() == Some(id) {
            self.default = self
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone());
        }
        Some(removed)
    }
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

pub fn load(data_dir: &Path) -> SlackConfig {
    std::fs::read(config_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn io_error(error: std::io::Error) -> SlackError {
    SlackError::Transport(format!("saving settings: {error}"))
}

pub fn save(data_dir: &Path, config: &SlackConfig) -> SlackResult<()> {
    std::fs::create_dir_all(data_dir).map_err(io_error)?;
    let path = config_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(config)?).map_err(io_error)?;
    std::fs::rename(&staged, &path).map_err(io_error)
}

fn keychain_error(error: sikemux_keychain::KeychainError) -> SlackError {
    match error {
        sikemux_keychain::KeychainError::Invalid(message) => SlackError::BadArg(message),
        sikemux_keychain::KeychainError::Failed(message) => SlackError::Keychain(message),
    }
}

pub fn keychain_read(workspace: &Workspace) -> SlackResult<Option<String>> {
    sikemux_keychain::read(TOKEN_SERVICE, &workspace.id).map_err(keychain_error)
}

pub fn keychain_write(workspace: &Workspace, token: &str) -> SlackResult<()> {
    sikemux_keychain::write(TOKEN_SERVICE, &workspace.id, token).map_err(keychain_error)
}

pub fn keychain_delete(workspace: &Workspace) -> SlackResult<()> {
    sikemux_keychain::delete(TOKEN_SERVICE, &workspace.id).map_err(keychain_error)
}

/// Runs work that starts a process or touches the Keychain on a thread meant for blocking.
pub async fn blocking<T: Send + 'static>(
    work: Box<dyn FnOnce() -> SlackResult<T> + Send>,
) -> SlackResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| SlackError::Keychain(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: &str, domain: &str) -> Workspace {
        Workspace {
            id: id.into(),
            name: id.into(),
            domain: domain.into(),
            user_id: "U1".into(),
            user: "ankit".into(),
        }
    }

    #[test]
    fn the_first_workspace_is_the_default_and_links_find_theirs_by_address() {
        let mut config = SlackConfig::default();
        assert!(config.workspace(None).is_none());
        config.upsert(workspace("T1", "acme.slack.com"));
        config.upsert(workspace("T2", "swishx.slack.com"));
        assert_eq!(config.workspace(None).map(|w| w.id.as_str()), Some("T1"));
        assert_eq!(
            config.workspace(Some("T2")).map(|w| w.id.as_str()),
            Some("T2")
        );
        assert_eq!(
            config.by_domain("SwishX.slack.com").map(|w| w.id.as_str()),
            Some("T2")
        );
        config.remove("T1");
        assert_eq!(config.default.as_deref(), Some("T2"));
    }

    #[test]
    fn nothing_saved_reads_as_no_workspaces() {
        let dir = std::env::temp_dir().join(format!("sikemux-slack-config-{}", std::process::id()));
        assert_eq!(load(&dir), SlackConfig::default());
        let mut saved = SlackConfig::default();
        saved.upsert(workspace("T1", "acme.slack.com"));
        save(&dir, &saved).expect("saves");
        assert_eq!(load(&dir), saved);
        std::fs::remove_dir_all(dir).ok();
    }
}
