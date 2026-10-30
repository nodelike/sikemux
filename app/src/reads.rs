//! What the core's watches last answered, each under a field of its own and decoded once
//! where it arrives, so a screen reads a field rather than looking an answer up by its key.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_core::git::log::GitOverview;
use sikemux_core::git::status::ProjectRepo;
use sikemux_core::server::{
    AccountUsage, ActivitySummary, Draft, HostRow, ProjectEntry, ProjectPort, RunView, ShareView,
    ShownStash, SshHost, TaskList, TrustQuestion, UpdateStatus,
};
use sikemux_plugin_api::codehost::PullList;
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

/// Drafts the core keeps, by the key each is kept under.
pub type Drafts = BTreeMap<String, Draft>;

#[derive(Default)]
pub struct Reads {
    pub catalog: Catalog,
    pub plugins: Plugins,
    /// The account's other hosts, then the SSH hosts a terminal is open to.
    pub hosts: Vec<HostRow>,
    /// How updating this app stands, once the core has said.
    pub update: Option<UpdateStatus>,
    /// How much of its plan each account has used, as the core last read it.
    pub usage: Vec<AccountUsage>,
    /// The chats this host shares read-only.
    pub shares: Vec<ShareView>,
    /// The account's Stash, as this host last listed it.
    pub stash: Option<ShownStash>,
    pub themes: Vec<ThemeSummary>,
    /// This host's trust questions, and those of the account host in front.
    pub trust: Vec<TrustQuestion>,
    pub host_trust: Vec<TrustQuestion>,
    /// The composer drafts kept on this host, and on the account host in front.
    pub drafts: Option<Drafts>,
    pub host_drafts: Option<Drafts>,
    /// What the project in front holds: its tasks, their runs, what listens on its ports,
    /// its activity, and the repository its git pane shows.
    pub tasks: Option<TaskList>,
    pub runs: Vec<RunView>,
    pub ports: Vec<ProjectPort>,
    pub activity: Option<ActivitySummary>,
    pub git_project_repo: Option<ProjectRepo>,
    /// The repository's status, branches and log, or why they did not read.
    pub git_overview: Option<Result<GitOverview, String>>,
    pub git_draft: Option<Drafts>,
    /// Activity across every project, for Settings; `Some(None)` when the core refused it.
    pub activity_everywhere: Option<Option<ActivitySummary>>,
    /// The worktree in front's pull requests, and whether its branch is merged.
    pub worktree_pulls: Option<PullList>,
    pub worktree_merged: Option<bool>,
    /// What the project and host pickers offer: folders under the project roots, and SSH hosts.
    pub picker_folders: Vec<ProjectEntry>,
    pub ssh_hosts: Vec<SshHost>,
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

    /// Keeps the hosts the core listed, and says whether they differ from those kept.
    pub fn keep_hosts(&mut self, value: Value) -> bool {
        keep_decoded(&mut self.hosts, value, "hosts")
    }

    /// Keeps how updating stands, and says whether it differs from what was kept.
    pub fn keep_update(&mut self, value: Value) -> bool {
        keep_decoded(&mut self.update, value, "update status")
    }

    /// Keeps the accounts' plan usage, and says whether it differs from what was kept.
    pub fn keep_usage(&mut self, value: Value) -> bool {
        keep_decoded(&mut self.usage, value, "plan usage")
    }

    /// Keeps the stash the core listed, and says whether it differs from the one kept. When
    /// this window counted the stash seen later than the answer says, its time stands.
    pub fn keep_stash(&mut self, value: Value) -> bool {
        let Some(mut told) = decoded::<ShownStash>(value, "stash") else {
            return false;
        };
        let seen = |stash: &ShownStash| {
            let at = stash.seen_at.as_deref()?;
            chrono::DateTime::parse_from_rfc3339(at).ok()
        };
        if let Some(kept) = self.stash.as_ref() {
            if seen(kept).is_some_and(|at| seen(&told).is_none_or(|told| told < at)) {
                told.seen_at = kept.seen_at.clone();
            }
        }
        keep(&mut self.stash, Some(told))
    }

    /// Keeps the drafts `key`'s watch answered, and says whether they differ from those kept.
    pub fn keep_drafts(&mut self, key: &str, value: Value) -> bool {
        let slot = match key {
            "drafts" => &mut self.drafts,
            "host-drafts" => &mut self.host_drafts,
            _ => &mut self.git_draft,
        };
        keep_decoded(slot, value, "drafts")
    }

    /// Keeps what listens on the project's ports, and says whether it differs from what was kept.
    pub fn keep_ports(&mut self, value: Value) -> bool {
        keep_decoded(&mut self.ports, value, "ports")
    }

    /// Keeps the activity summary `key`'s watch answered, and says whether it differs from the
    /// one kept. The project's activity draws empty when the core refused it.
    pub fn keep_activity(&mut self, key: &str, value: Value) -> bool {
        let read = match value.is_null() {
            true => None,
            false => match decoded::<ActivitySummary>(value, "activity") {
                Some(summary) => Some(summary),
                None => return false,
            },
        };
        match key {
            "activity" => keep(&mut self.activity, Some(read.unwrap_or_default())),
            _ => keep(&mut self.activity_everywhere, Some(read)),
        }
    }

    /// Keeps which repository the project's Git views open on, and those inside its folder.
    pub fn keep_project_repo(&mut self, value: Value) {
        if let Some(found) = decoded(value, "project repository") {
            self.git_project_repo = Some(found);
        }
    }

    /// Keeps the repository's overview, even one that does not read, which the Git pane says.
    pub fn keep_overview(&mut self, value: Value) {
        self.git_overview = Some(serde_json::from_value(value).map_err(|error| error.to_string()));
    }

    /// Keeps the chats this host shares, and says whether they differ from those kept.
    pub fn keep_shares(&mut self, value: Value) -> bool {
        keep_decoded(&mut self.shares, value, "shares")
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

    /// Keeps what the worktree in front's pull requests or merged check answered.
    pub fn keep_worktree(&mut self, key: &str, value: Value) {
        match key {
            crate::worktree_head::PULLS => {
                self.worktree_pulls = crate::codehost::answer(&value)
                    .inspect_err(|error| eprintln!("sikemux: the worktree's pulls: {error}"))
                    .ok();
            }
            _ => self.worktree_merged = decoded(value, "worktree merged"),
        }
    }

    /// Forgets what `key` answered, so nothing shows the answer of what it watched before.
    pub fn forget(&mut self, key: &str) {
        match key {
            "tasks" => self.tasks = None,
            "git-draft" => self.git_draft = None,
            "runs" => self.runs.clear(),
            "ports" => self.ports.clear(),
            "activity" => self.activity = None,
            "git-project-repo" => self.git_project_repo = None,
            "trust" => self.trust.clear(),
            "host-trust" => self.host_trust.clear(),
            crate::worktree_head::PULLS => self.worktree_pulls = None,
            crate::worktree_head::MERGED => self.worktree_merged = None,
            _ => {}
        }
    }
}

/// Keeps `value` decoded in `slot`, and says whether it differs from what was kept. An answer
/// that does not read leaves the slot as it was.
fn keep_decoded<T: DeserializeOwned + PartialEq>(slot: &mut T, value: Value, what: &str) -> bool {
    decoded(value, what).is_some_and(|read| keep(slot, read))
}

/// Puts `read` in `slot`, and says whether it differs from what was there.
fn keep<T: PartialEq>(slot: &mut T, read: T) -> bool {
    let changed = *slot != read;
    *slot = read;
    changed
}

/// `value` read as a `T`, or none when it does not read as one, which is logged.
fn decoded<T: DeserializeOwned>(value: Value, what: &str) -> Option<T> {
    serde_json::from_value(value)
        .inspect_err(|error| eprintln!("sikemux: the core's {what} did not read: {error}"))
        .ok()
}
