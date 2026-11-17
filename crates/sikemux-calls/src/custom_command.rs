use serde::{Deserialize, Serialize};
use sikemux_view::layout::At;
use sikemux_wire::protocol::SessionId;

#[derive(Serialize, Deserialize)]
pub struct CommandResult {
    pub code: i32,
    pub output: String,
}

#[derive(Serialize, Deserialize)]
pub struct RunCustomParams {
    /// The strip in front it runs for: a project, the terminals of no project or an SSH host's.
    #[serde(flatten)]
    pub at: At,
    /// The command, by its id in the person's preferences.
    #[serde(default)]
    pub id: Option<String>,
    /// Or the project's `sikemux.json` action, by its id, which runs once the person trusts
    /// the file.
    #[serde(default)]
    pub action: Option<String>,
    /// The tab in front and its view in focus, which a split or replacing command goes
    /// beside or in place of.
    pub tab: Option<String>,
    pub pane: Option<String>,
}

/// What running a custom command did: the tab it opened or changed, the terminal to float
/// over the window, or what it printed in the background.
#[derive(Serialize, Deserialize, Default)]
pub struct CommandRan {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub popup: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<CommandResult>,
}
