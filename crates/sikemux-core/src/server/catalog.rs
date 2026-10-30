//! The agent catalog clients read, built from the core's own list.

use sikemux_wire::catalog::{CatalogAgent, Driver};

use crate::agent_catalog::{ChatDriver, AGENTS};

pub(crate) fn view() -> Vec<CatalogAgent> {
    AGENTS
        .iter()
        .map(|agent| CatalogAgent {
            kind: agent.kind.to_owned(),
            label: agent.label.to_owned(),
            about: agent.about.to_owned(),
            command: agent.command.to_owned(),
            brand: agent.brand.to_owned(),
            maker: agent.maker.map(str::to_owned),
            featured: agent.featured,
            accounts: agent.accounts,
            usage: agent.usage,
            deletable_chats: agent.deletable_chats,
            resumes_by_path: agent.resumes_by_path,
            relocatable_sessions: agent.relocatable_sessions,
            bypass: agent.bypass,
            commit_models: agent
                .commit_models
                .iter()
                .map(|model| (*model).to_owned())
                .collect(),
            efforts: agent
                .efforts
                .iter()
                .map(|effort| (*effort).to_owned())
                .collect(),
            install: agent.install.map(str::to_owned),
            login: agent
                .login
                .map(|login| login.iter().map(|arg| (*arg).to_owned()).collect()),
            chat: agent.chat.as_ref().map(|chat| match chat {
                ChatDriver::Claude => Driver::Claude,
                ChatDriver::Codex => Driver::Codex,
                ChatDriver::Muse => Driver::Muse,
                ChatDriver::Acp { .. } => Driver::Acp,
            }),
        })
        .collect()
}
