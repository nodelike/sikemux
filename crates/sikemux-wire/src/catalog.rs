//! The agents Sikemux can run, as every client names and offers them, so
//! none ships its own copy.

use serde::{Deserialize, Serialize};

/// Which of the core's chat drivers runs an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    Claude,
    Codex,
    Muse,
    Acp,
}

/// An agent in the catalog, as clients read it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAgent {
    /// Its id, such as `claude`.
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    /// One line on what it is, for someone choosing which to install.
    pub about: String,
    pub command: String,
    /// Its colour, as `#rrggbb`.
    pub brand: String,
    /// The company whose account it runs on by default, which heads the models it
    /// offers from that account; none when it has no account of its own.
    pub maker: Option<String>,
    /// Offered first, before anything else is installed.
    pub featured: bool,
    /// It keeps each sign-in in its own folder, so the person can hold
    /// several accounts.
    pub accounts: bool,
    /// The app can read how much of its plan is used.
    pub usage: bool,
    /// The app can delete a saved chat from its own history.
    pub deletable_chats: bool,
    /// It can also name a saved session by its transcript's path.
    pub resumes_by_path: bool,
    /// Its saved sessions can follow its chat into another folder.
    pub relocatable_sessions: bool,
    /// It can start without asking for permissions.
    pub bypass: bool,
    /// The models the core can ask it for a commit message with, the default first;
    /// none when it cannot write one.
    pub commit_models: Vec<String>,
    /// The reasoning efforts its CLI takes.
    pub efforts: Vec<String>,
    /// Its official install command; none when it has none to run.
    pub install: Option<String>,
    /// The arguments that start its own sign-in; none when it has none.
    pub login: Option<Vec<String>>,
    /// The chat driver that runs it; none when it has no chat.
    pub chat: Option<Driver>,
}
