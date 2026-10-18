//! The agents on linked hosts this host works with: workers its agents
//! started there, and leads there whose workers run here. Each is known by
//! the host it runs on and its agent id, and shown under its own name with
//! the host's. Kept in `host-team.json`, so a team outlives a restart.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::protocol::AgentStatus;

use super::super::kept::Kept;

const FILE: &str = "host-team.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Role {
    /// An agent here started it on its host.
    Worker,
    /// It started a worker that runs here.
    Lead,
}

/// An agent on a linked host.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Foreign {
    pub agent_id: String,
    /// The key of the host it runs on.
    pub host: String,
    pub name: String,
    pub cli: String,
    /// The project here its team works in.
    pub project: String,
    /// The agent that started it.
    pub parent: Option<String>,
    pub role: Role,
    pub status: AgentStatus,
    #[serde(default)]
    pub active_at: Option<u64>,
    /// The checkout of its lead's project here that its commits come back to.
    #[serde(default)]
    pub repo: Option<PathBuf>,
    /// The branch its latest commits came back on and how many there are,
    /// until its next report says so.
    #[serde(default)]
    pub landed: Option<(String, usize)>,
}

/// A worker's own checkout here, cut for its lead on another host.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tree {
    pub agent_id: String,
    /// The repository the checkout is a worktree of.
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// The lead's commit it started from.
    pub base: String,
    /// The commit its work last went back to the lead at.
    #[serde(default)]
    pub sent: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    agents: Vec<Foreign>,
    trees: Vec<Tree>,
}

pub(crate) struct Team {
    record: Mutex<Record>,
    kept: Kept,
}

impl Team {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            record: Mutex::new(kept.load()),
            kept,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Record> {
        self.record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn change<R>(&self, change: impl FnOnce(&mut Record) -> R) -> R {
        let mut record = self.lock();
        let changed = change(&mut record);
        self.kept.save(&*record);
        changed
    }

    pub(crate) fn get(&self, agent_id: &str) -> Option<Foreign> {
        self.lock()
            .agents
            .iter()
            .find(|agent| agent.agent_id == agent_id)
            .cloned()
    }

    pub(crate) fn all(&self) -> Vec<Foreign> {
        self.lock().agents.clone()
    }

    pub(crate) fn ids(&self) -> HashSet<String> {
        self.lock()
            .agents
            .iter()
            .map(|agent| agent.agent_id.clone())
            .collect()
    }

    pub(crate) fn on(&self, host: &str) -> Vec<Foreign> {
        self.lock()
            .agents
            .iter()
            .filter(|agent| agent.host == host)
            .cloned()
            .collect()
    }

    pub(crate) fn has_host(&self, host: &str) -> bool {
        self.lock().agents.iter().any(|agent| agent.host == host)
    }

    pub(crate) fn add(&self, foreign: Foreign) {
        self.change(|record| {
            record
                .agents
                .retain(|agent| agent.agent_id != foreign.agent_id);
            record.agents.push(foreign);
        });
    }

    pub(crate) fn remove(&self, agent_id: &str) -> Option<Foreign> {
        self.change(|record| {
            let index = record
                .agents
                .iter()
                .position(|agent| agent.agent_id == agent_id)?;
            Some(record.agents.remove(index))
        })
    }

    pub(crate) fn remove_host(&self, host: &str) -> Vec<Foreign> {
        self.change(|record| {
            let (gone, kept) = std::mem::take(&mut record.agents)
                .into_iter()
                .partition(|agent| agent.host == host);
            record.agents = kept;
            gone
        })
    }

    /// Changes what is known of an agent, if it is one.
    pub(crate) fn update(&self, agent_id: &str, edit: impl FnOnce(&mut Foreign)) -> bool {
        self.change(|record| {
            match record
                .agents
                .iter_mut()
                .find(|agent| agent.agent_id == agent_id)
            {
                Some(agent) => {
                    edit(agent);
                    true
                }
                None => false,
            }
        })
    }

    pub(crate) fn tree(&self, agent_id: &str) -> Option<Tree> {
        self.lock()
            .trees
            .iter()
            .find(|tree| tree.agent_id == agent_id)
            .cloned()
    }

    pub(crate) fn add_tree(&self, tree: Tree) {
        self.change(|record| {
            record.trees.retain(|known| known.agent_id != tree.agent_id);
            record.trees.push(tree);
        });
    }

    pub(crate) fn tree_sent(&self, agent_id: &str, head: &str) {
        self.change(|record| {
            if let Some(tree) = record
                .trees
                .iter_mut()
                .find(|tree| tree.agent_id == agent_id)
            {
                tree.sent = Some(head.to_owned());
            }
        });
    }

    pub(crate) fn remove_tree(&self, agent_id: &str) -> Option<Tree> {
        self.change(|record| {
            let index = record
                .trees
                .iter()
                .position(|tree| tree.agent_id == agent_id)?;
            Some(record.trees.remove(index))
        })
    }
}
