//! Short-lived credentials a plugin mints for one command against a remote. Only the core asks
//! for them: no plugin method, mod or agent tool can reach one by name.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    /// The git URL or web address as the command has it.
    pub remote: String,
    /// Set only by the core, for a push the person passed with a button.
    pub workflows: bool,
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
