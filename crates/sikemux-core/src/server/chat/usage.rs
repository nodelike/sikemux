//! A resumed chat says nothing of its context window until it runs a turn, so
//! the transcript its CLI saved tells every app how full the window was.

use crate::server::chat_state::ChatParts;
use serde_json::json;

use crate::agent_catalog::AgentKind;
use crate::launch::named_account;
use crate::sessions::context::read_session_context;
use sikemux_fold::ChatEventKind;
use sikemux_wire::protocol::ChatStart;

use super::native::{saved_window, AutoCompact};
use super::Chat;

impl Chat {
    pub(super) fn tell_saved_usage(&self, start: &ChatStart) {
        let launch = self.current_launch();
        let (Some(resumed), Some(kind)) = (
            launch.resume_id.clone(),
            AgentKind::from_name(&launch.provider),
        ) else {
            return;
        };
        let account = named_account(&launch.provider, &launch.env).map(str::to_owned);
        let core = self.core.clone();
        let agent_id = self.agent_id().to_owned();
        let generation = self.generation;
        let capped = self.capped();
        let start = start.clone();
        tokio::task::spawn_blocking(move || {
            let cwd = launch.cwd.to_string_lossy();
            let Some(saved) = read_session_context(kind, &cwd, &resumed, account.as_deref()) else {
                return;
            };
            let size = saved
                .size
                .unwrap_or_else(|| saved_window(&start.setup, launch.model.as_deref(), saved.used));
            let Some(chat) = core.upgrade().and_then(|core| core.chats().get(&agent_id)) else {
                return;
            };
            let mut update = sikemux_fold::update::usage(saved.used, size);
            if kind == AgentKind::Claude {
                let model = launch.model.as_deref();
                if let Some(at) = AutoCompact::of(&launch, capped).at(model, size) {
                    update["compactAt"] = json!(at);
                }
            }
            if chat.generation == generation {
                chat.feed.emit(
                    ChatEventKind::SessionUpdate,
                    json!({ "sessionId": start.session_id, "update": update }),
                );
            }
        });
    }
}
