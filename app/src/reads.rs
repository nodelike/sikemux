//! What the core's watches last answered, each under a field of its own and decoded once
//! where it arrives, so a screen reads a field rather than looking an answer up by its key.

use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_core::server::{ProjectEntry, RunView, SshHost, TaskList, TrustQuestion};
use sikemux_plugin_api::{Listed, TokenForm};
use sikemux_wire::catalog::CatalogAgent;
use sikemux_wire::theme::ThemeSummary;

/// The agents the core can run, as its catalog lists them.
#[derive(Default, PartialEq)]
pub struct Catalog(Vec<CatalogAgent>);

impl Catalog {
    pub fn agents(&self) -> impl Iterator<Item = &CatalogAgent> {
        self.0.iter()
    }

    /// The catalog's entry for the agent kind `provider`, such as `claude`.
    pub fn kind(&self, provider: &str) -> Option<&CatalogAgent> {
        self.0.iter().find(|agent| agent.kind == provider)
    }

    /// What the agent kind is called, such as "Claude", or its id until the catalog is read.
    pub fn label<'a>(&'a self, provider: &'a str) -> &'a str {
        self.kind(provider).map_or(provider, |agent| &agent.label)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The plugins built in, as the core last listed them.
#[derive(Default, PartialEq)]
pub struct Plugins(Vec<Listed>);

impl Plugins {
    pub fn all(&self) -> impl Iterator<Item = &Listed> {
        self.0.iter()
    }

    /// Those switched on.
    pub fn on(&self) -> impl Iterator<Item = &Listed> {
        self.0.iter().filter(|plugin| plugin.enabled)
    }

    /// Whether the person is signed in to `id`; none for a plugin with no sign-in.
    pub fn signed_in(&self, id: &str) -> Option<bool> {
        self.0.iter().find(|plugin| plugin.id == id)?.signed_in
    }

    /// The form `id` is signed in to with a token, when it has one.
    pub fn token_form(&self, id: &str) -> Option<&TokenForm> {
        self.0
            .iter()
            .find(|plugin| plugin.id == id)?
            .token_form
            .as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Default)]
pub struct Reads {
    pub catalog: Catalog,
    pub plugins: Plugins,
    pub hosts: Option<Value>,
    pub update: Option<Value>,
    pub usage: Option<Value>,
    pub shares: Option<Value>,
    pub stash: Option<Value>,
    pub themes: Vec<ThemeSummary>,
    /// This host's trust questions, and those of the account host in front.
    pub trust: Vec<TrustQuestion>,
    pub host_trust: Vec<TrustQuestion>,
    /// The composer drafts kept on this host, and on the account host in front.
    pub drafts: Option<Value>,
    pub host_drafts: Option<Value>,
    /// What the project in front holds: its tasks, their runs, what listens on its ports,
    /// its activity, and the repository its git pane shows.
    pub tasks: Option<TaskList>,
    pub runs: Vec<RunView>,
    pub ports: Option<Value>,
    pub activity: Option<Value>,
    pub git_project_repo: Option<Value>,
    pub git_overview: Option<Value>,
    pub git_draft: Option<Value>,
    /// Activity across every project, for Settings; null when the core refused it.
    pub activity_everywhere: Option<Value>,
    /// The worktree in front's pull requests, and whether its branch is merged.
    pub worktree_pulls: Option<Value>,
    pub worktree_merged: Option<Value>,
    /// What the project and host pickers offer: folders under the project roots, and SSH hosts.
    pub picker_folders: Vec<ProjectEntry>,
    pub ssh_hosts: Vec<SshHost>,
}

/// Which field keeps the answer to each watch, by the watch's key.
macro_rules! slots {
    ($($key:pat => $field:ident),* $(,)?) => {
        impl Reads {
            fn slot(&self, key: &str) -> Option<&Option<Value>> {
                Some(match key {
                    $($key => &self.$field,)*
                    _ => return None,
                })
            }

            fn slot_mut(&mut self, key: &str) -> Option<&mut Option<Value>> {
                Some(match key {
                    $($key => &mut self.$field,)*
                    _ => return None,
                })
            }
        }
    };
}

slots! {
    "hosts" => hosts,
    "update" => update,
    "usage" => usage,
    "shares" => shares,
    "stash" => stash,
    "drafts" => drafts,
    "host-drafts" => host_drafts,
    "ports" => ports,
    "activity" => activity,
    "git-project-repo" => git_project_repo,
    "git-overview" => git_overview,
    "git-draft" => git_draft,
    crate::settings::ACTIVITY => activity_everywhere,
    crate::worktree_head::PULLS => worktree_pulls,
    crate::worktree_head::MERGED => worktree_merged,
}

impl Reads {
    /// Keeps the agent catalog the core sent, and says whether it differs from the one kept.
    pub fn keep_catalog(&mut self, value: Value) -> bool {
        keep_decoded(&mut self.catalog.0, value, "agent catalog")
    }

    /// Keeps the plugins the core listed, and says whether they differ from those kept.
    pub fn keep_plugins(&mut self, value: Value) -> bool {
        keep_decoded(&mut self.plugins.0, value, "plugins")
    }

    /// Keeps the themes the core listed, and says whether they differ from those kept.
    pub fn keep_themes(&mut self, value: Value) -> bool {
        keep_decoded(&mut self.themes, value, "themes")
    }

    /// Keeps what the pickers were answered: the folders under the project roots, or this
    /// host's SSH hosts.
    pub fn keep_picked(&mut self, key: &str, value: Value) {
        match key {
            "pickerFolders" => keep_decoded(&mut self.picker_folders, value, "project folders"),
            _ => keep_decoded(&mut self.ssh_hosts, value, "SSH hosts"),
        };
    }

    /// Keeps what the project's tasks, the runs or a host's trust questions were answered, and
    /// says whether it differs from what was kept.
    pub fn keep_tasks(&mut self, key: &str, value: Value) -> bool {
        match key {
            "tasks" => keep_decoded(&mut self.tasks, value, "tasks"),
            "runs" => keep_decoded(&mut self.runs, value, "task runs"),
            "trust" => keep_decoded(&mut self.trust, value, "trust questions"),
            _ => keep_decoded(&mut self.host_trust, value, "trust questions"),
        }
    }

    /// Keeps what `key`'s watch answered. Gives the answer back when no field holds `key`.
    pub fn keep(&mut self, key: &str, value: Value) -> Result<(), Value> {
        match self.slot_mut(key) {
            Some(slot) => {
                *slot = Some(value);
                Ok(())
            }
            None => Err(value),
        }
    }

    /// Whether `key` already holds `value`, so a push carrying it changes nothing.
    pub fn holds(&self, key: &str, value: &Value) -> bool {
        self.slot(key)
            .is_some_and(|slot| slot.as_ref() == Some(value))
    }

    /// Forgets what `key` answered, so nothing shows the answer of what it watched before.
    pub fn forget(&mut self, key: &str) {
        match key {
            "tasks" => self.tasks = None,
            "runs" => self.runs.clear(),
            "trust" => self.trust.clear(),
            "host-trust" => self.host_trust.clear(),
            _ => {
                if let Some(slot) = self.slot_mut(key) {
                    *slot = None;
                }
            }
        }
    }
}

/// Keeps `value` decoded in `slot`, and says whether it differs from what was kept. An answer
/// that does not read leaves the slot as it was.
fn keep_decoded<T: DeserializeOwned + PartialEq>(slot: &mut T, value: Value, what: &str) -> bool {
    match serde_json::from_value::<T>(value) {
        Ok(read) if read == *slot => false,
        Ok(read) => {
            *slot = read;
            true
        }
        Err(error) => {
            eprintln!("sikemux: the core's {what} did not read: {error}");
            false
        }
    }
}
