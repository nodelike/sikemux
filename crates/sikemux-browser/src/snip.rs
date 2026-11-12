//! Snip mode: the person drags a box over an agent's page, and what it covers comes back as a
//! picture with the page's address and the elements under it, for the agent's chat.

use std::time::Duration;

use serde_json::{json, Value};

use crate::manager::BrowserManager;
use crate::native;
use crate::tools::{active, call, screenshot};
use crate::PAGE_SCRIPT;

/// How long one wait for a box lasts before the next takes over.
const WAIT: Duration = Duration::from_secs(60);

/// A box the person dragged over a page.
pub struct Snip {
    pub jpeg: Vec<u8>,
    /// Where the box sat in the page's viewport, the page's address and title, and the
    /// elements mostly inside it, or the one holding it, as the page described them.
    pub page: Value,
}

impl Snip {
    /// The box in the viewport, which the next wait flashes once the picture is taken.
    pub fn area(&self) -> Value {
        json!({
            "left": self.page["left"],
            "top": self.page["top"],
            "width": self.page["width"],
            "height": self.page["height"],
        })
    }
}

pub enum Snipped {
    /// The person dragged a box over tab `tab_id`; `page` is what the page said of it.
    Boxed { tab_id: String, page: Value },
    /// The person clicked an element and wrote a note on it: `pin` is what the page said of it.
    Pinned { pin: Value },
    /// The person pressed Escape on the page.
    Ended,
    /// The wait ran out, or another took it over.
    Nothing,
}

/// Puts the agent's shown page in snip mode, its box and cursor drawn in `accent` (`#rrggbb`),
/// and waits for the person's next box, flashing `taken`, the box taken last, first.
pub async fn wait(
    manager: &BrowserManager,
    agent_id: &str,
    taken: Option<&Value>,
    accent: &str,
) -> Result<Snipped, String> {
    let (tab_id, view) = active(manager, agent_id)?;
    let taken = taken.map_or_else(|| "null".to_owned(), Value::to_string);
    let accent = Value::from(accent);
    let body = format!(
        "{PAGE_SCRIPT}\nreturn JSON.stringify(await window.__sikemux.snip(true, {taken}, {accent}));"
    );
    let Some(answer) = native::wait_helper(&view, &body, WAIT).await? else {
        return Ok(Snipped::Nothing);
    };
    let page: Value = serde_json::from_str(&answer)
        .map_err(|_| "the page returned an unreadable answer".to_string())?;
    if page.is_null() {
        return Ok(Snipped::Nothing);
    }
    if page["ended"] == true {
        return Ok(Snipped::Ended);
    }
    if page["pin"].is_object() {
        return Ok(Snipped::Pinned {
            pin: page["pin"].clone(),
        });
    }
    Ok(Snipped::Boxed { tab_id, page })
}

/// The picture of a box [`wait`] brought back.
pub async fn take(
    manager: &BrowserManager,
    agent_id: &str,
    tab_id: String,
    page: Value,
) -> Result<Snip, String> {
    let view = manager
        .loaded_view(agent_id, &tab_id)
        .ok_or("the tab closed before its picture was taken")?;
    let side = |key: &str| page[key].as_f64().unwrap_or(0.0);
    let area = (side("left"), side("top"), side("width"), side("height"));
    let jpeg = screenshot(&view, None, Some(area)).await?;
    Ok(Snip { jpeg, page })
}

/// Takes tab `tab_id` out of snip mode.
pub async fn stop(manager: &BrowserManager, agent_id: &str, tab_id: &str) {
    if let Some(view) = manager.loaded_view(agent_id, tab_id) {
        let _ = call(&view, "snip", &[json!(false)]).await;
    }
}
