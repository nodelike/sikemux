//! The agents Sikemux can run, as every client names and offers them, so
//! none ships its own copy.

use serde::Serialize;

use crate::agent_catalog::{ChatDriver, AGENTS};

/// Which of the core's chat drivers runs an agent.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Driver {
    Claude,
    Codex,
    Muse,
    Acp,
}

/// An agent in the catalog, as clients read it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogAgent {
    /// Its id, such as `claude`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub label: &'static str,
    /// One line on what it is, for someone choosing which to install.
    pub about: &'static str,
    pub command: &'static str,
    /// Its colour, as `#rrggbb`.
    pub brand: &'static str,
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
    pub commit_models: Vec<&'static str>,
    /// The reasoning efforts its CLI takes.
    pub efforts: Vec<&'static str>,
    /// Its official install command; none when it has none to run.
    pub install: Option<&'static str>,
    /// The arguments that start its own sign-in; none when it has none.
    pub login: Option<Vec<&'static str>>,
    /// The chat driver that runs it; none when it has no chat.
    pub chat: Option<Driver>,
}

pub(crate) fn view() -> Vec<CatalogAgent> {
    AGENTS
        .iter()
        .map(|agent| CatalogAgent {
            kind: agent.kind,
            label: agent.label,
            about: agent.about,
            command: agent.command,
            brand: agent.brand,
            featured: agent.featured,
            accounts: agent.accounts,
            usage: agent.usage,
            deletable_chats: agent.deletable_chats,
            resumes_by_path: agent.resumes_by_path,
            relocatable_sessions: agent.relocatable_sessions,
            bypass: agent.bypass,
            commit_models: agent.commit_models.to_vec(),
            efforts: agent.efforts.to_vec(),
            install: agent.install,
            login: agent.login.map(<[_]>::to_vec),
            chat: agent.chat.as_ref().map(|chat| match chat {
                ChatDriver::Claude => Driver::Claude,
                ChatDriver::Codex => Driver::Codex,
                ChatDriver::Muse => Driver::Muse,
                ChatDriver::Acp { .. } => Driver::Acp,
            }),
        })
        .collect()
}
