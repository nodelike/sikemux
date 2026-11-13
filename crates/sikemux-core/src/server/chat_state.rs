//! What the core keeps of its chats, and how the rest reads it.

use std::time::Duration;

use super::{chat, mcp_apps, model_proxy, Core};

pub(crate) struct ChatFields {
    pub(crate) chats: chat::Chats,
    pub(crate) mcp_apps: mcp_apps::McpApps,
    pub(crate) model_proxy: model_proxy::ModelProxy,
    pub(crate) chat_above: super::chat_above::ChatAbove,
    /// How long a chat whose agent stopped by itself waits before it starts it again.
    pub(crate) restart_wait: Duration,
}

pub(crate) trait ChatParts {
    fn chats(&self) -> &chat::Chats;
    fn mcp_apps(&self) -> &mcp_apps::McpApps;
    fn model_proxy(&self) -> &model_proxy::ModelProxy;
    fn chat_above(&self) -> &super::chat_above::ChatAbove;
    fn restart_wait(&self) -> &Duration;
}

impl ChatParts for Core {
    fn chats(&self) -> &chat::Chats {
        &self.kept::<ChatFields>().chats
    }

    fn mcp_apps(&self) -> &mcp_apps::McpApps {
        &self.kept::<ChatFields>().mcp_apps
    }

    fn model_proxy(&self) -> &model_proxy::ModelProxy {
        &self.kept::<ChatFields>().model_proxy
    }

    fn chat_above(&self) -> &super::chat_above::ChatAbove {
        &self.kept::<ChatFields>().chat_above
    }

    fn restart_wait(&self) -> &Duration {
        &self.kept::<ChatFields>().restart_wait
    }
}
