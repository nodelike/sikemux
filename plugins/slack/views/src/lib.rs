//! What the Slack plugin's views hand a pane: a workspace's channels, a channel's latest
//! messages, and a thread with what an agent is handed of it.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChannelKind {
    Channel,
    Private,
    Dm,
    Group,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
    pub updated: Option<u64>,
}

/// The channels the person is in, then their direct messages, each by name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Channels {
    pub rooms: Vec<Channel>,
    pub people: Vec<Channel>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub ts: String,
    pub user: Option<String>,
    pub name: String,
    pub bot: bool,
    /// The text as markdown, people and channels named.
    pub text: String,
    /// When it was written, as an RFC 3339 time.
    pub at: String,
    pub thread_ts: Option<String>,
    pub reply_count: u64,
    pub files: Vec<String>,
}

/// A channel's latest messages, oldest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub messages: Vec<Message>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub workspace: String,
    pub channel: String,
    pub ts: String,
    pub permalink: Option<String>,
    pub messages: Vec<Message>,
    /// The thread is longer than what came back.
    pub truncated: bool,
}

/// A thread as a pane shows it, with the text an agent is handed when the person sends it one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenThread {
    pub thread: Thread,
    pub for_agent: String,
}

/// The workspaces signed in, as the plugin's `status` says.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Signed {
    pub workspaces: Vec<Listed>,
}

/// A workspace signed in, and who in it.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub domain: String,
    pub user: String,
    pub user_id: String,
    pub is_default: bool,
}
