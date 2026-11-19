//! A resumed chat says nothing of its context window until it runs a turn, so
//! the transcript its CLI saved tells every app how full the window was.

use crate::server::chat_state::ChatParts;
use serde_json::{json, Value};

use crate::agent_catalog::AgentKind;
use crate::launch::{claude_windows, named_account};
use crate::sessions::context::read_session_context;
use crate::sessions::roots::account_root;
use sikemux_fold::ChatEventKind;

use super::native::saved_window;
use super::Chat;
use sikemux_drivers::server::chat::cap::{claude_window, compact_at};

impl Chat {
    pub fn tell_saved_usage(&self, session_id: &str, setup: &Value) {
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
        let (session_id, setup) = (session_id.to_owned(), setup.clone());
        tokio::task::spawn_blocking(move || {
            let cwd = launch.cwd.to_string_lossy();
            let Some(saved) = read_session_context(kind, &cwd, &resumed, account.as_deref()) else {
                return;
            };
            let learned = saved.model.as_deref().and_then(|model| {
                let account = account_root(kind, None, &launch.env)?;
                claude_windows::of(&account, model)
            });
            let size = saved.size.unwrap_or_else(|| {
                saved_window(&setup, launch.model.as_deref(), saved.used, learned)
            });
            let Some(chat) = core.upgrade().and_then(|core| core.chats().get(&agent_id)) else {
                return;
            };
            let mut update = sikemux_fold::update::usage(saved.used, size);
            if let (AgentKind::Claude, Some(fields)) = (kind, update.as_object_mut()) {
                let compact = compact_at(size, claude_window(launch.model.as_deref()));
                fields.insert("compactAt".to_owned(), json!(compact));
            }
            if chat.generation == generation {
                chat.feed.emit(
                    ChatEventKind::SessionUpdate,
                    json!({ "sessionId": session_id, "update": update }),
                );
            }
        });
    }
}
