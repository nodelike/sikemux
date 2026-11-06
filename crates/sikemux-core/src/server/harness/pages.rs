//! Pages an agent shows: writing one down, where it shows, and reading it back.

use super::*;
use serde::{Deserialize, Serialize};

pub(super) const PAGE_IN_REPLY: &str = "The person sees the page in your reply, above the text you write next. Do not describe or restate it; write only what it does not already say.";
pub(super) const PAGE_ON_DESK: &str =
    "The page is open on your desk, beside your terminal, where the person can see it.";
pub(super) const PAGE_KEPT: &str =
    "The page is kept, but no Sikemux window is open to show it on your desk.";

/// Keeps the page an agent wrote. A chat shows it in its reply; for an agent
/// in a terminal the window opens it on the agent's desk.
pub(super) async fn show_page(
    core: &Arc<Core>,
    mut request: HarnessRequest,
) -> Result<Value, String> {
    let path = PathBuf::from(required_text(&request.params, "path")?);
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    let title = required_text(&request.params, "title")?.to_owned();
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
    let page = blocking(move || Ok(crate::pages::publish(&dir, &path, &title, height)))
        .await
        .map_err(|error| error.to_string())??;
    let in_chat = request
        .agent_id
        .as_deref()
        .is_none_or(|agent_id| core.chats.get(agent_id).is_some());
    let mut on_desk = false;
    if core.window.is_open() {
        request.params = json!({ "id": page.id, "desk": !in_chat });
        on_desk = forward(core, request).await.is_ok() && !in_chat;
    }
    let note = if in_chat {
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
    let html = blocking(move || {
        std::fs::read_to_string(dir.join(format!("{}.html", params.id)))
            .map_err(|_| CoreError::from("that page is gone"))
    })
    .await?;
    Ok(PageHtml { html })
}
