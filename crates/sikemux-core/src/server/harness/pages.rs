//! Pages an agent shows: writing one down, where it shows, and reading it back.

use super::*;
use serde::{Deserialize, Serialize};

pub(super) const PAGE_IN_REPLY: &str = "The person sees the page in your reply, above the text you write next. Do not describe or restate it; write only what it does not already say.";
pub(super) const PAGE_ON_DESK: &str =
    "The page is open on your desk, beside your terminal, where the person can see it.";
pub(super) const PAGE_IN_TAB: &str = "The page is open in a tab on your desk, and your reply shows a card that brings it forward. Do not describe or restate it; write only what it does not already say.";
pub(super) const PAGE_KEPT: &str =
    "The page is kept, but no Sikemux window is open to show it on your desk.";

/// Keeps the page an agent wrote. A chat shows it in its reply; for an agent
/// in a terminal, or one that asked for a tab, the window opens it on the agent's desk,
/// and a chat's reply then shows a card with a picture of it.
pub(super) async fn show_page(
    core: &Arc<Core>,
    mut request: HarnessRequest,
) -> Result<Value, String> {
    let path = PathBuf::from(required_text(&request.params, "path")?);
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    let title = required_text(&request.params, "title")?.to_owned();
    let tab = request.params.get("tab").and_then(Value::as_bool) == Some(true);
    let height = match request.params.get("height") {
        None | Some(Value::Null) => None,
        Some(_) => Some(integer(&request.params, "height", 0, 4000, 1)? as u32),
    };
    let dir = core
        .config
        .data_dir
        .clone()
        .ok_or("Pages need Sikemux's data folder, and this core was started without one")?
        .join(crate::pages::FOLDER);
    let kept = dir.clone();
    let mut page = blocking(move || Ok(crate::pages::publish(&dir, &path, &title, height)))
        .await
        .map_err(|error| error.to_string())??;
    let in_chat = request
        .agent_id
        .as_deref()
        .is_none_or(|agent_id| core.chats.get(agent_id).is_some());
    let mut on_desk = false;
    if core.window.is_open() {
        let card = in_chat && tab;
        request.params = json!({ "id": page.id, "desk": !in_chat || tab, "picture": card });
        let shown = forward(core, request).await;
        on_desk = shown.is_ok() && (!in_chat || tab);
        if let Some(picture) = shown
            .as_ref()
            .ok()
            .and_then(|shown| shown["picture"].as_str())
        {
            keep_picture(&kept, &page.id, picture).await;
        }
        page.tab = card && on_desk;
    }
    let note = if page.tab {
        PAGE_IN_TAB
    } else if in_chat {
        PAGE_IN_REPLY
    } else if on_desk {
        PAGE_ON_DESK
    } else {
        PAGE_KEPT
    };
    Ok(json!({ "page": page, "note": note }))
}

#[derive(Deserialize, Serialize)]
pub struct PageParams {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct PageHtml {
    pub html: String,
    /// A JPEG of the page as its desk tab first drew it, in base64, for a page shown in a tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
}

fn picture_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.jpg"))
}

async fn keep_picture(dir: &Path, id: &str, picture: &str) {
    use base64::Engine as _;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(picture) else {
        return;
    };
    let path = picture_path(dir, id);
    let _ = blocking(move || Ok(std::fs::write(path, bytes))).await;
}

/// A page an agent showed with `page_show`, as a chat shows it in the reply.
pub(crate) async fn read_page(core: &Core, params: PageParams) -> CoreResult<PageHtml> {
    if !crate::pages::is_page_id(&params.id) {
        return Err("that is not a page's id".into());
    }
    let dir = core
        .config
        .data_dir
        .clone()
        .ok_or("this core keeps no pages")?
        .join(crate::pages::FOLDER);
    blocking(move || {
        use base64::Engine as _;
        let html = std::fs::read_to_string(dir.join(format!("{}.html", params.id)))
            .map_err(|_| CoreError::from("that page is gone"))?;
        let picture = std::fs::read(picture_path(&dir, &params.id))
            .ok()
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
        Ok(PageHtml { html, picture })
    })
    .await
}
