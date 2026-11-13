use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveChatToHostParams {
    pub agent_id: String,
    /// The host it goes to, by its core's key.
    pub host: String,
    /// The project there, by its id there.
    pub project: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMoved {
    /// The chat on the host it went to.
    pub agent_id: String,
}

#[derive(Deserialize, Serialize)]
pub struct AliasParams {
    pub alias: String,
}

/// How a set-up ended: why it stopped short, if it did, and the host it added to the account.
#[derive(Deserialize, Serialize)]
pub struct SetupEnded {
    pub problem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOnParams {
    pub path: PathBuf,
    pub host: String,
    /// What is not committed goes too, as a commit on top.
    #[serde(default)]
    pub uncommitted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SetUpWork {
    Install,
    /// Updates a host that runs an older release, then restarts its core; a host signed out
    /// signs in on the way.
    Update,
    SignIn,
    /// Starts the stopped core of a host Sikemux is already on.
    Start,
}

#[derive(Deserialize, Serialize)]
pub struct InstallParams {
    pub alias: String,
    pub work: SetUpWork,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendParams {
    /// The project, by id.
    pub project: String,
    pub host: String,
    #[serde(default)]
    pub uncommitted: bool,
    /// Moves rather than copies.
    #[serde(default, rename = "move")]
    pub moving: bool,
    /// Into a worktree at this project's commit on the other host, when its checkout there
    /// stands elsewhere.
    #[serde(default)]
    pub worktree: bool,
    /// The project on the other host, by its id there, the person has in front, which the
    /// session goes into when that host has several checkouts of the repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub there: Option<String>,
}

/// What stays behind, and why.
#[derive(Serialize, Deserialize)]
pub struct Left {
    pub title: String,
    pub why: String,
}

/// Where a project's session went: the host and project there, the chats it carried, those a
/// move closed here and those it left, and how its uncommitted changes went.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSent {
    pub host: String,
    pub host_name: String,
    pub project: String,
    pub path: Option<String>,
    pub uncommitted: Option<String>,
    /// The chats the other host holds now.
    pub carried: Vec<String>,
    /// Of those, the ones a move closed here.
    #[serde(default)]
    pub closed: Vec<String>,
    pub left: Vec<Left>,
    /// Why nothing went: the other host's checkout stands at another commit or branch, so
    /// the session can go into a worktree there instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub differs: Option<String>,
}
