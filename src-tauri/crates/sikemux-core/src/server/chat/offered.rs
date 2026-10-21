//! The models each agent offered when a chat of it last started, so a choice
//! made before any chat of it runs, such as the agent Auto starts, can list
//! them.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde_json::Value;
use sikemux_process::sync::Locked;

static OFFERED: LazyLock<Mutex<HashMap<String, Value>>> = LazyLock::new(Mutex::default);

/// Keeps the model choice a chat of `agent` started with.
pub(crate) fn remember(agent: &str, setup: &Value) {
    let Some(model) = setup["configOptions"]
        .as_array()
        .and_then(|options| options.iter().find(|option| option["id"] == "model"))
    else {
        return;
    };
    OFFERED.locked().insert(agent.to_owned(), model.clone());
}

/// The model choice `agent` last offered, if a chat of it has started since
/// the core did.
pub(crate) fn models(agent: &str) -> Option<Value> {
    OFFERED.locked().get(agent).cloned()
}
