//! What a client reads again when the core says it changed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a client reads again, as `Event::Changed` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Topic {
    Activity,
    AgentAccounts,
    AgentSignIn,
    AgentStatus,
    Boards,
    Git,
    Ports,
    Runs,
    Shares,
    Skills,
    Tasks,
    Trust,
    /// How much of their plans the agent accounts have used.
    Usage,
    /// The SSH config's hosts, or how a terminal reaches one.
    Ssh,
    /// Which plugins are on, and whether each is signed in.
    Plugins,
    /// The questions agents put to the person, and the assumptions they went on.
    Questions,
    /// Who this host is signed in to Sikemux as.
    Account,
    /// The hosts linked with this one, the SSH hosts a terminal is open to,
    /// and how each is reached.
    Hosts,
    /// The programs outside Sikemux the person lets drive its chats, and those waiting.
    Outside,
    /// A language server's diagnostics for the files in `paths` moved.
    Lsp,
    /// How the person wants Sikemux to behave.
    Preferences,
    /// What the person has typed and not sent yet; `paths` names the drafts' keys.
    Drafts,
    /// A project's pull requests, runs, issues or releases on its code host.
    CodeHost,
    /// Files clients have open moved on disk; `paths` names them as the clients did.
    Files,
    /// Whether an update is out, and how installing it goes.
    Update,
    /// What the account keeps in its Stash.
    Stash,
    /// The rules and knowledge the core keeps for a project or a folder above it.
    Memory,
    /// What a project's Playbook lists: its rules, places, landing or automation.
    Playbook,
    /// A goal's summary, or how writing the next one goes.
    GoalSummary,
    /// What the `goals` call answers: its agents, their desks or what waits on the person.
    Goals,
    /// The mods in the person's mods folder; `paths` names those whose files moved.
    Mods,
    /// A mod's own data; `paths` names each collection that changed as `mod/collection`.
    ModData,
    /// The core is stopping on purpose, so its clients do not start it again.
    Stopping,
    /// How busy this host is, for its page, while a client shows it.
    HostStats,
    /// A topic from a newer core, which a client skips.
    #[serde(other)]
    Unknown,
}

impl Topic {
    /// As `Event::Changed` carries it, since the wire crate doesn't name this type.
    pub fn to_json(self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// What `Event::Changed` names. A topic this build doesn't know is [`Topic::Unknown`].
    pub fn of_json(topic: &Value) -> Self {
        Self::deserialize(topic).unwrap_or(Self::Unknown)
    }
}
