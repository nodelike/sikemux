//! Pages an agent shows: writing one down, where it shows, and reading it back.

use super::*;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::serving::ServeParts;
pub use sikemux_calls::harness::{PageHtml, PageParams};

pub(super) const PAGE_IN_TAB: &str = "The page is open in a tab on your desk, and your reply shows a card that brings it forward. Do not describe or restate it; write only what it does not already say.";
pub(super) const PAGE_ON_DESK: &str =
    "The page is open on your desk, beside your terminal, where the person can see it.";
pub(super) const PAGE_KEPT: &str =
    "The page is kept, but no Sikemux window is open to show it on your desk.";

/// Keeps the page an agent wrote and opens it in a tab on the agent's desk. A chat's reply
/// shows a card with a picture of it that brings the tab forward.
pub(super) async fn show_page(
    core: &Arc<Core>,
    mut request: HarnessRequest,
) -> Result<Value, String> {
    let path = PathBuf::from(required_text(&request.params, "path")?);
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    let title = required_text(&request.params, "title")?.to_owned();
    let dir = core
        .config()
        .data_dir
        .clone()
        .ok_or("Pages need Sikemux's data folder, and this core was started without one")?
        .join(crate::pages::FOLDER);
    let kept = dir.clone();
    let page = blocking(move || Ok(crate::pages::publish(&dir, &path, &title)))
        .await
        .map_err(|error| error.to_string())??;
    let asker = request.agent_id.clone();
    let in_chat = (asker.as_deref()).is_none_or(|agent_id| core.chats().get(agent_id).is_some());
    let mut on_desk = false;
    if core.window().is_open() {
        request.params = json!({ "id": page.id, "picture": in_chat });
        let shown = forward(core, request).await;
        on_desk = shown.is_ok();
        if let Some(picture) = shown
            .as_ref()
            .ok()
            .and_then(|shown| shown["picture"].as_str())
        {
            keep_picture(&kept, &page.id, picture).await;
        }
    }
    if let Some(agent_id) = asker.as_deref() {
        shown_by(core, agent_id, &page);
    }
    let note = match (on_desk, in_chat) {
        (true, true) => PAGE_IN_TAB,
        (true, false) => PAGE_ON_DESK,
        (false, _) => PAGE_KEPT,
    };
    Ok(json!({ "page": page, "note": note }))
}

/// Whoever started `agent_id` sees the page as a row in their chat, and a call about its work
/// links it.
fn shown_by(core: &Core, agent_id: &str, page: &crate::pages::PageRef) {
    let lead = core.workspaces().agent_leads().remove(agent_id);
    let name = (core.names().get(agent_id)).map_or_else(|| agent_id.to_owned(), |named| named.name);
    let shown = sikemux_view::chat::WorkerPage {
        id: page.id.clone(),
        title: page.title.clone(),
        agent_id: agent_id.to_owned(),
        name,
    };
    crate::server::chief::page_shown(core, lead.as_deref(), shown);
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

/// A page an agent showed with `page_show`, with the picture its card in the reply shows.
pub(crate) async fn read_page(core: &Core, params: PageParams) -> CoreResult<PageHtml> {
    if !crate::pages::is_page_id(&params.id) {
        return Err("that is not a page's id".into());
    }
    let dir = core
        .config()
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
