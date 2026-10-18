// The Linear workspaces signed in here. Each one's API key, or the refresh token
// of a browser sign-in, lives in the Keychain; the file beside it only says which workspaces there are, who is
// signed in to each, and which one a call uses when it names none.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{LinearError, LinearResult};

/// A development build keeps entries of its own, so it never spends or replaces the
/// installed app's sign-in.
#[cfg(not(test))]
const KEY_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-linear-key-dev"
} else {
    "sikemux-linear-key"
};
/// Tests keep to an entry of their own, so they never replace or delete a real key.
#[cfg(test)]
const KEY_SERVICE: &str = "sikemux-linear-key-test";

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Linear's organization id.
    pub id: String,
    pub name: String,
    /// The workspace's part of its addresses, `linear.app/<url_key>/…`.
    pub url_key: String,
    pub user_id: String,
    pub user: String,
    #[serde(default)]
    pub sign_in: SignIn,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SignIn {
    #[default]
    Key,
    Browser,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LinearConfig {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub default: Option<String>,
}

impl LinearConfig {
    /// The workspace named by id or url key, or with none named, the default one.
    pub fn workspace(&self, wanted: Option<&str>) -> Option<&Workspace> {
        let wanted = wanted
            .filter(|wanted| !wanted.is_empty())
            .or(self.default.as_deref());
        match wanted {
            Some(wanted) => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == wanted || workspace.url_key == wanted),
            None => self.workspaces.first(),
        }
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

pub fn load(data_dir: &Path) -> LinearConfig {
    std::fs::read(config_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn io_error(error: std::io::Error) -> LinearError {
    LinearError::Transport(format!("saving settings: {error}"))
}

pub fn save(data_dir: &Path, config: &LinearConfig) -> LinearResult<()> {
    std::fs::create_dir_all(data_dir).map_err(io_error)?;
    let path = config_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(config)?).map_err(io_error)?;
    std::fs::rename(&staged, &path).map_err(io_error)
}

fn keychain_error(error: sikemux_keychain::KeychainError) -> LinearError {
    match error {
        sikemux_keychain::KeychainError::Invalid(message) => LinearError::BadArg(message),
        sikemux_keychain::KeychainError::Failed(message) => LinearError::Keychain(message),
    }
}

pub fn keychain_read(workspace: &Workspace) -> LinearResult<Option<String>> {
    sikemux_keychain::read(KEY_SERVICE, &workspace.id).map_err(keychain_error)
}

pub fn keychain_write(workspace: &Workspace, key: &str) -> LinearResult<()> {
    sikemux_keychain::write(KEY_SERVICE, &workspace.id, key).map_err(keychain_error)
}

pub fn keychain_delete(workspace: &Workspace) -> LinearResult<()> {
    sikemux_keychain::delete(KEY_SERVICE, &workspace.id).map_err(keychain_error)
}

/// Runs work that touches the Keychain on a thread meant for blocking.
pub async fn blocking<T: Send + 'static>(
    work: Box<dyn FnOnce() -> LinearResult<T> + Send>,
) -> LinearResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| LinearError::Keychain(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: &str, url_key: &str) -> Workspace {
        Workspace {
            id: id.into(),
            name: id.into(),
            url_key: url_key.into(),
            user_id: "u1".into(),
            user: "ankit".into(),
            sign_in: SignIn::Key,
        }
    }

    #[test]
    fn the_first_workspace_is_the_default_and_a_url_key_finds_its_workspace() {
        let mut config = LinearConfig::default();
        assert!(config.workspace(None).is_none());
        config.upsert(workspace("o1", "acme"));
        config.upsert(workspace("o2", "side"));
        assert_eq!(config.workspace(None).map(|w| w.id.as_str()), Some("o1"));
        assert_eq!(
            config.workspace(Some("side")).map(|w| w.id.as_str()),
            Some("o2")
        );
        config.remove("o1");
        assert_eq!(config.default.as_deref(), Some("o2"));
    }

    #[test]
    fn nothing_saved_reads_as_no_workspaces() {
        let dir =
            std::env::temp_dir().join(format!("sikemux-linear-config-{}", std::process::id()));
        assert_eq!(load(&dir), LinearConfig::default());
        let mut saved = LinearConfig::default();
        saved.upsert(workspace("o1", "acme"));
        save(&dir, &saved).expect("saves");
        assert_eq!(load(&dir), saved);
        std::fs::remove_dir_all(dir).ok();
    }
}
