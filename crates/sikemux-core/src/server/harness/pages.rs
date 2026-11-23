//! Pages an agent shows: writing one down, where it shows, and reading it back.

use super::page_index;
use super::*;
use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::serving::ServeParts;
pub use sikemux_calls::harness::{KeepPageParams, ListedPage, PageHtml, PageParams, PagesParams};

pub(super) const PAGE_IN_TAB: &str = "The page is open in a tab on your desk, and your reply shows a card that brings it forward. Do not describe or restate it; write only what it does not already say.";
pub(super) const PAGE_ON_DESK: &str =
    "The page is open on your desk, beside your terminal, where the person can see it.";
const PAGE_ON_SHUT_DESK: &str = "The person did not start this turn, so the page waits in a tab on your desk, which stays shut. To have them look, raise a call with ask and this page's id in page.";
pub(super) const PAGE_NO_WINDOW: &str = "No Sikemux window is open to show it on your desk.";

fn pages_dir(core: &Core) -> Result<(PathBuf, PathBuf), String> {
    let data_dir = (core.config().data_dir.clone())
        .ok_or("Pages need Sikemux's data folder, and this core was started without one")?;
    let pages = data_dir.join(crate::pages::FOLDER);
    Ok((data_dir, pages))
}

/// Keeps the page an agent wrote, or takes one already kept by its id, and opens it in a tab on
/// the agent's desk. A chat's reply shows a card with a picture of it that brings the tab forward.
pub(super) async fn show_page(
    core: &Arc<Core>,
    mut request: HarnessRequest,
) -> Result<Value, String> {
    let (data_dir, dir) = pages_dir(core)?;
    let asker = request.agent_id.clone();
    let ask = (asker.as_deref()).and_then(|agent_id| core.chats().get(agent_id)?.answering());
    let keep = request.params.get("keep").and_then(Value::as_bool) == Some(true);
    let page = match request.params.get("id").and_then(Value::as_str) {
        Some(id) => {
            let id = id.trim().to_owned();
            blocking(move || {
                let kept =
                    crate::pages::is_page_id(&id) && dir.join(format!("{id}.html")).is_file();
                if !kept {
                    return Ok(Err(format!(
                        "{id} is no page Sikemux keeps; page_list finds the kept ones"
                    )));
                }
                if let Err(error) = crate::pages::refresh(&dir, &id) {
                    return Ok(Err(error));
                }
                let title = (page_index::title_of(&data_dir, &id))
                    .or_else(|| crate::pages::title(&dir, &id))
                    .unwrap_or_else(|| "Page".to_owned());
                let _ = page_index::shown_again(&data_dir, &id, ask);
                if keep {
                    let _ = page_index::keep(&data_dir, &id, true);
                }
                Ok(Ok(crate::pages::PageRef { id, title }))
            })
            .await
            .map_err(|error| error.to_string())??
        }
        None => {
            let path = PathBuf::from(required_text(&request.params, "path")?);
            if !path.is_absolute() {
                return Err("path must be absolute".into());
            }
            let title = required_text(&request.params, "title")?.to_owned();
            let mut shown = page_index::Shown {
                project: crate::server::caller::project_of(core, &request),
                agent: asker.clone().unwrap_or_default(),
                name: (asker.as_deref())
                    .and_then(|agent_id| core.names().get(agent_id))
                    .map(|named| named.name)
                    .unwrap_or_default(),
                ask,
                ..Default::default()
            };
            blocking(move || {
                let page = crate::pages::publish(&dir, &path, &title);
                if let Ok(page) = &page {
                    shown.id = page.id.clone();
                    shown.title = page.title.clone();
                    if let Err(error) = page_index::add(&data_dir, &shown) {
                        eprintln!("sikemux core: page {} was not indexed: {error}", page.id);
                    } else if keep {
                        let _ = page_index::keep(&data_dir, &page.id, true);
                    }
                }
                Ok(page)
            })
            .await
            .map_err(|error| error.to_string())??
        }
    };
    core.changed(What::Read(Topic::Pages));
    let (_, kept) = pages_dir(core)?;
    let in_chat = (asker.as_deref()).is_none_or(|agent_id| core.chats().get(agent_id).is_some());
    let quiet = in_chat
        && (asker.as_deref())
            .and_then(|agent_id| core.chats().get(agent_id))
            .is_some_and(|chat| chat.feed.person_said().is_empty());
    let mut on_desk = false;
    let mut shut = false;
    if core.window().is_open() {
        request.params = json!({ "id": page.id, "picture": in_chat, "quiet": quiet });
        let shown = forward(core, request).await;
        on_desk = shown.is_ok();
        shut = (shown.as_ref().ok()).is_some_and(|shown| shown["shut"] == true);
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
        (true, true) if shut => PAGE_ON_SHUT_DESK,
        (true, true) => PAGE_IN_TAB,
        (true, false) => PAGE_ON_DESK,
        (false, _) => PAGE_NO_WINDOW,
    };
    let kept = if keep {
        format!(
            "Sikemux keeps it as page {} until the person lets it go: page_list finds it and page_show with that id opens it again.",
            page.id
        )
    } else {
        format!(
            "Sikemux keeps it as page {} for a week, or a day after the call it goes with settles, unless it is kept: page_list finds it and page_show with that id opens it again.",
            page.id
        )
    };
    Ok(json!({ "page": page, "note": format!("{note} {kept}") }))
}

/// The latest version of each page agents showed, newest first.
pub(super) async fn list_pages(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let (data_dir, _) = pages_dir(core)?;
    let words = request.params.get("words").and_then(Value::as_str);
    let words: Vec<String> = (words.unwrap_or_default().split_whitespace())
        .take(8)
        .map(str::to_owned)
        .collect();
    let every = request.params.get("allProjects").and_then(Value::as_bool) == Some(true);
    let project = (!every).then(|| crate::server::caller::project_of(core, request));
    let open_calls = core.harness().questions.open_ids();
    let pages = blocking(move || {
        Ok(page_index::list(
            &data_dir,
            &words,
            project.as_deref(),
            &open_calls,
            page_index::MOST_LISTED,
        ))
    })
    .await
    .map_err(|error| error.to_string())??;
    if every {
        return Ok(json!({ "pages": pages }));
    }
    Ok(json!({
        "pages": pages,
        "note": "Only this project's. allProjects: true lists every project's too, and the pages shown before Sikemux kept a list of them.",
    }))
}

/// Every page agents showed in a project, for its library.
pub(crate) async fn library(core: &Core, params: PagesParams) -> CoreResult<Vec<ListedPage>> {
    let (data_dir, _) = pages_dir(core)?;
    let words: Vec<String> = (params.words.split_whitespace())
        .take(8)
        .map(str::to_owned)
        .collect();
    let open_calls = core.harness().questions.open_ids();
    blocking(move || {
        page_index::list(
            &data_dir,
            &words,
            Some(&params.project),
            &open_calls,
            page_index::LIBRARY_HOLDS,
        )
        .map_err(CoreError::from)
    })
    .await
}

/// Keeps a page, or lets it go again.
pub(crate) async fn keep_page(core: &Core, params: KeepPageParams) -> CoreResult<()> {
    let (data_dir, _) = pages_dir(core)?;
    blocking(move || page_index::keep(&data_dir, &params.id, params.keep).map_err(CoreError::from))
        .await?;
    core.changed(What::Read(Topic::Pages));
    Ok(())
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
        let media =
            Some(crate::pages::media_folder(&dir, &params.id)).filter(|media| media.is_dir());
        Ok(PageHtml {
            html,
            picture,
            media,
            written_in: crate::pages::written_in(&dir, &params.id),
        })
    })
    .await
}
