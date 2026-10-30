//! What a client reads again when the core says it changed.

use serde::{Deserialize, Serialize};

/// What a client reads again, as `Event::Changed` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Topic {
    Activity,
    AgentAccounts,
    AgentSignIn,
    AgentStatus,
    Boards,
    Ci,
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
    /// A language server's diagnostics for the files in `paths` moved.
    Lsp,
    /// How the person wants Sikemux to behave.
    Preferences,
    /// What the person has typed and not sent yet; `paths` names the drafts' keys.
    Drafts,
    /// A project's pull requests, runs, issues or releases on its code host.
    CodeHost,
    /// What a plugin's pane shows; `paths` names each view as `plugin/method`.
    PluginView,
    /// Files clients have open moved on disk; `paths` names them as the clients did.
    Files,
    /// Whether an update is out, and how installing it goes.
    Update,
    /// What the account keeps in its Stash.
    Stash,
    /// The core is stopping on purpose, so its clients do not start it again.
    Stopping,
    /// A topic from a newer core, which a client skips.
    #[serde(other)]
    Unknown,
}
