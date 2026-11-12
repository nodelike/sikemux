//! Pins on a page: the app tells the page which pins it holds and in which colours, and hears
//! when the person answers one in the page's own float.

use std::time::Duration;

use serde_json::{json, Value};

use crate::manager::BrowserManager;
use crate::native;
use crate::tools::{active, call};
use crate::PAGE_SCRIPT;

/// How long one wait for an answer lasts before the next takes over.
const WAIT: Duration = Duration::from_secs(60);

/// What the person did on a pin in the page.
pub enum PinAnswer {
    /// Wrote another note on pin `id`.
    Note {
        id: String,
        words: String,
    },
    /// Said pin `id` looks right.
    LooksRight {
        id: String,
    },
    Nothing,
}

/// Draws `pins` over the agent's shown page, in the colours and with the words `look` gives.
pub async fn show(
    manager: &BrowserManager,
    agent_id: &str,
    pins: &Value,
    look: &Value,
) -> Result<(), String> {
    let (_, view) = active(manager, agent_id)?;
    call(&view, "pins", &[pins.clone(), look.clone()]).await?;
    Ok(())
}

/// Waits for the person's next answer to a pin on the agent's shown page.
pub async fn wait(manager: &BrowserManager, agent_id: &str) -> Result<PinAnswer, String> {
    let (_, view) = active(manager, agent_id)?;
    let body = format!("{PAGE_SCRIPT}\nreturn JSON.stringify(await window.__sikemux.pinWait());");
    let Some(answer) = native::wait_helper(&view, &body, WAIT).await? else {
        return Ok(PinAnswer::Nothing);
    };
    let answer: Value = serde_json::from_str(&answer)
        .map_err(|_| "the page returned an unreadable answer".to_string())?;
    let text = |at: &Value, key: &str| at[key].as_str().map(str::to_owned);
    if let (Some(id), Some(words)) = (text(&answer["note"], "id"), text(&answer["note"], "words")) {
        return Ok(PinAnswer::Note { id, words });
    }
    if let Some(id) = text(&answer["looksRight"], "id") {
        return Ok(PinAnswer::LooksRight { id });
    }
    Ok(PinAnswer::Nothing)
}

/// What a page that runs the pin's look says about it, for the words it is drawn with.
pub fn look(
    accent: &str,
    ground: &str,
    ink: &str,
    quiet: &str,
    line: &str,
    to: Option<&str>,
) -> Value {
    json!({ "accent": accent, "ground": ground, "ink": ink, "quiet": quiet, "line": line, "to": to })
}
