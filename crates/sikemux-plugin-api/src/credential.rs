//! Short-lived credentials a plugin mints for one command against a remote. Only the core asks
//! for them: no plugin method, mod or agent tool can reach one by name.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    /// The git URL or web address as the command has it.
    pub remote: String,
    #[serde(default)]
    pub reach: Reach,
}

/// What a credential may write, named by who uses it.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum Reach {
    /// An agent's own command: pull requests, issues and Actions, never the repository's contents.
    #[default]
    Agent,
    /// The core's own push or merge, which may write contents. Revoked once it is done.
    Core,
    /// The core's push of a workflow change the person passed. Revoked once it is done.
    CoreWorkflows,
}

/// Who commits made with a credential carry as committer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub name: String,
    pub email: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub username: String,
    pub secret: String,
    /// Milliseconds since the Unix epoch.
    pub expires_at: u64,
    pub actor: Actor,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credential")
            .field("username", &self.username)
            .field("expires_at", &self.expires_at)
            .field("actor", &self.actor)
            .finish_non_exhaustive()
    }
}
