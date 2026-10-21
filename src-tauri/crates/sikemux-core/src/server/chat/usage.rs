//! A resumed chat says nothing of its context window until it runs a turn, so
//! the transcript its CLI saved tells every app how full the window was.

use serde_json::{json, Value};

use crate::acp::current_choice;
use crate::agent_catalog::AgentKind;
use crate::launch::named_account;
use crate::protocol::{ChatEventKind, ChatStart};
use crate::sessions::context::read_session_context;

use super::Chat;

const DEFAULT_WINDOW: u64 = 200_000;
const LONG_WINDOW: u64 = 1_000_000;

/// Claude's transcript does not say how big its window is. A model named for
/// a 1M window has one, and so does a session already past 200K.
fn window(setup: &Value, launched_with: Option<&str>, used: u64) -> u64 {
    let current = current_choice(setup, "model");
    let label = current.as_deref().and_then(|value| {
        setup["configOptions"]
            .as_array()?
            .iter()
            .find(|option| option["id"] == "model")?["options"]
            .as_array()?
            .iter()
            .find(|option| option["value"] == value)?["name"]
            .as_str()
    });
    let names_long = [current.as_deref(), label, launched_with]
        .into_iter()
        .flatten()
        .any(|name| {
            let name = name.to_ascii_lowercase();
            name.split(|char: char| !char.is_ascii_alphanumeric())
                .any(|word| word == "1m")
        });
    if names_long || used > DEFAULT_WINDOW {
        LONG_WINDOW
    } else {
        DEFAULT_WINDOW
    }
}

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
        let start = start.clone();
        tokio::task::spawn_blocking(move || {
            let cwd = launch.cwd.to_string_lossy();
            let Some(saved) = read_session_context(kind, &cwd, &resumed, account.as_deref()) else {
                return;
            };
            let size = saved
                .size
                .unwrap_or_else(|| window(&start.setup, launch.model.as_deref(), saved.used));
            let Some(chat) = core.upgrade().and_then(|core| core.chats.get(&agent_id)) else {
                return;
            };
            if chat.generation == generation {
                chat.feed.emit(
                    ChatEventKind::SessionUpdate,
                    json!({
                        "sessionId": start.session_id,
                        "update": { "sessionUpdate": "usage_update", "used": saved.used, "size": size },
                    }),
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models(current: &str, options: &[(&str, &str)]) -> Value {
        let options: Vec<Value> = options
            .iter()
            .map(|(value, name)| json!({ "value": value, "name": name }))
            .collect();
        json!({ "configOptions": [{ "id": "model", "type": "select", "currentValue": current, "options": options }] })
    }

    #[test]
    fn a_model_named_for_a_million_tokens_has_the_long_window() {
        assert_eq!(window(&models("opus[1m]", &[]), None, 0), LONG_WINDOW);
        assert_eq!(
            window(
                &models("default", &[("default", "Opus (1M context)")]),
                None,
                0
            ),
            LONG_WINDOW
        );
        assert_eq!(window(&json!({}), Some("sonnet[1m]"), 0), LONG_WINDOW);
    }

    #[test]
    fn a_session_past_the_standard_window_has_the_long_one() {
        assert_eq!(
            window(&json!({}), Some("claude-opus-5-5"), 515_222),
            LONG_WINDOW
        );
        assert_eq!(
            window(&json!({}), Some("claude-opus-5-5"), 150_000),
            DEFAULT_WINDOW
        );
    }

    #[test]
    fn every_other_model_has_the_standard_window() {
        assert_eq!(
            window(&models("sonnet", &[("sonnet", "Sonnet 5")]), None, 0),
            DEFAULT_WINDOW
        );
        assert_eq!(window(&json!({}), Some("sonnet"), 0), DEFAULT_WINDOW);
        assert_eq!(
            window(&models("claude-1mini", &[]), None, 0),
            DEFAULT_WINDOW
        );
    }
}
