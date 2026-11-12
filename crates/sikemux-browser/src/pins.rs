//! Pins on a page: the app tells the page which pins it holds and in which colours, and hears
//! when the person answers one in the page's own float.

use std::time::Duration;

use serde_json::{json, Value};

use crate::manager::BrowserManager;
use crate::native;
use crate::tools::{active, call, screenshot};
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

/// Room left around an element in a picture of it, in the page's pixels.
const ROOM: f64 = 16.0;

/// `area` of the page's viewport with room around it, kept inside the viewport.
fn with_room(area: &Value, viewport: (f64, f64)) -> (f64, f64, f64, f64) {
    let side = |key: &str| area[key].as_f64().unwrap_or(0.0);
    let left = (side("left") - ROOM).max(0.0);
    let top = (side("top") - ROOM).max(0.0);
    let width = (side("width") + 2.0 * ROOM).min(viewport.0 - left).max(1.0);
    let height = (side("height") + 2.0 * ROOM).min(viewport.1 - top).max(1.0);
    (left, top, width, height)
}

/// A picture of `area` of tab `tab_id`, the person's own, as it shows now: nothing loads.
pub async fn spot(
    manager: &BrowserManager,
    agent_id: &str,
    tab_id: String,
    area: &Value,
) -> Result<Vec<u8>, String> {
    let view = manager
        .loaded_view(agent_id, &tab_id)
        .ok_or("the tab closed before its picture was taken")?;
    let size = call(&view, "viewportSize", &[]).await?;
    let viewport = (
        size["width"].as_f64().unwrap_or(f64::MAX),
        size["height"].as_f64().unwrap_or(f64::MAX),
    );
    screenshot(&view, None, Some(with_room(area, viewport))).await
}

/// A picture of the element `selector` finds on the agent's shown page, the person's own tab,
/// when it is in sight there; nothing scrolls or loads.
pub async fn snapshot(
    manager: &BrowserManager,
    agent_id: &str,
    selector: &str,
) -> Result<Option<Vec<u8>>, String> {
    let (tab_id, view) = active(manager, agent_id)?;
    let area = call(&view, "pinRect", &[json!(selector)]).await?;
    if area.is_null() {
        return Ok(None);
    }
    spot(manager, agent_id, tab_id, &area).await.map(Some)
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
