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

/// A set-up as the host's row shows it: running, or why it stopped.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SetupRow {
    pub work: SetUpWork,
    pub problem: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reach {
    Connected,
    Connecting,
    /// A linked host this one has not reached, which it keeps trying, or an
    /// SSH host whose terminals are not connected.
    Away,
    /// Only for an SSH host: ssh gave up connecting to it.
    Failed,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostRow {
    /// The host's core key; none for an SSH host not linked with this one.
    pub id: Option<String>,
    pub name: String,
    pub platform: Option<String>,
    pub reach: Reach,
    /// The SSH host it was installed on or found at from here, or the row's own alias.
    pub ssh: Option<String>,
    /// What is being done to its SSH host from here: installing, updating or signing in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupRow>,
    /// What its SSH host was found to run when last asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<Found>,
    /// Why its SSH host has no answer yet; nothing is offered meanwhile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unanswered: Option<Unanswered>,
    /// What setting its SSH host up takes, once it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_up: Option<SetUpWork>,
}

/// A linked host as Settings and phones list it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedHost {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub linked_at: u64,
    pub last_seen: Option<u64>,
    pub connected: bool,
    /// Agents of teams this host shares with it.
    pub agents: usize,
}

/// What the SSH host's Sikemux is, beside this host's account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Found {
    /// The host answered, and no Sikemux is there.
    Nothing,
    Same,
    SignedOut,
    Other,
    Stopped,
    /// A release too old to say which host it is, or to be signed in from here.
    Outdated,
}

impl Found {
    /// What setting the host up takes, if anything does.
    pub fn set_up(self) -> Option<SetUpWork> {
        match self {
            Found::Nothing => Some(SetUpWork::Install),
            Found::SignedOut => Some(SetUpWork::SignIn),
            Found::Stopped => Some(SetUpWork::Start),
            Found::Outdated => Some(SetUpWork::Update),
            Found::Same | Found::Other => None,
        }
    }
}

/// Why an SSH host has no answer yet. Nothing is offered until it has one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Unanswered {
    /// Not asked yet, or being asked.
    Checking,
    /// ssh could not reach it.
    Unreachable { detail: String },
    /// ssh needs a password or a key it was not offered.
    SshSignIn { detail: String },
    /// It answered with something that says nothing about Sikemux.
    Failed { detail: String },
}
