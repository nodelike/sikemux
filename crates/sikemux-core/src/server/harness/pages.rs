//! Pages an agent shows: writing one down, where it shows, and reading it back.

use super::page_index;
use super::*;
use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::serving::ServeParts;
use crate::server::share::page::{PutError, Sharing, SIGNED_OUT_PROBLEM};
pub use sikemux_calls::harness::{KeepPageParams, ListedPage, PageHtml, PageParams, PagesParams};
use sikemux_calls::harness::{PageShare, SharePageParams};
use sikemux_corekit::backoff::Backoff;
use sikemux_process::clock::epoch_ms;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

pub(super) const PAGE_IN_TAB: &str = "The page is open in a tab on your desk, and your reply shows a card that brings it forward. Do not describe or restate it; write only what it does not already say.";
pub(super) const PAGE_ON_DESK: &str =
    "The page is open on your desk, beside your terminal, where the person can see it.";
const HOUR_MS: u64 = 60 * 60 * 1000;
const SHARED_AT_MOST_MS: u64 = 30 * 24 * HOUR_MS;
const UPLOAD_BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(2),
    cap: Duration::from_secs(5 * 60),
};
const PAGE_ON_SHUT_DESK: &str = "The person did not start this turn, so the page waits in a tab on your desk, which stays shut. To have them look, raise a call with ask and this page's id in page.";
pub(super) const PAGE_NO_WINDOW: &str = "No Sikemux window is open to show it on your desk.";

/// Coordinates background retries and direct share/stop calls so two versions never cross and a
/// stopped share is never uploaded after stopping.
#[derive(Default)]
pub(crate) struct PageShares {
    wake: Arc<Notify>,
    reset_backoff: AtomicBool,
    signed_in_again: AtomicBool,
    links: Mutex<HashMap<String, Arc<LinkGate>>>,
}

struct LinkGate {
    busy: tokio::sync::Mutex<()>,
    cancel: watch::Sender<u64>,
}

impl Default for LinkGate {
    fn default() -> Self {
        Self {
            busy: tokio::sync::Mutex::new(()),
            cancel: watch::channel(0).0,
        }
    }
}

impl PageShares {
    fn wake_now(&self, reset_backoff: bool) {
        if reset_backoff {
            self.reset_backoff.store(true, Ordering::Release);
        }
        self.wake.notify_one();
    }

    fn gate(&self, link_id: &str) -> Arc<LinkGate> {
        let mut links = self.links.locked();
        links
            .entry(link_id.to_owned())
            .or_insert_with(|| Arc::new(LinkGate::default()))
            .clone()
    }

    fn cancel_and_gate(&self, link_id: &str) -> Arc<LinkGate> {
        let gate = self.gate(link_id);
        gate.cancel.send_modify(|n| *n = n.wrapping_add(1));
        gate
    }

    fn forget_gate(&self, link_id: &str) {
        self.links.locked().remove(link_id);
    }
}

/// Network came back or the Mac woke: stop waiting out backoff and upload behind shares now.
pub(crate) fn network_back(core: &Core) {
    core.harness().page_shares.wake_now(true);
}

/// Account sign-in or live connection came back: retry behind shares, including ones held while signed out.
pub(crate) fn signed_in(core: &Core) {
    core.harness()
        .page_shares
        .signed_in_again
        .store(true, Ordering::Release);
    core.harness().page_shares.wake_now(true);
}

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
    let (_, dir) = pages_dir(core)?;
    let index = core.harness().page_index.clone();
    let asker = request.agent_id.clone();
    let ask = (asker.as_deref()).and_then(|agent_id| core.chats().get(agent_id)?.answering());
    let keep = request.params.get("keep").and_then(Value::as_bool) == Some(true);
    let page = match request.params.get("id").and_then(Value::as_str) {
        Some(id) => {
            let id = id.trim().to_owned();
            blocking(move || {
                let kept = crate::pages::page_file(&dir, &id).is_some();
                if !kept {
                    return Ok(Err(format!(
                        "{id} is no page Sikemux keeps; page_list finds the kept ones"
                    )));
                }
                let changed = match crate::pages::refresh(&dir, &id) {
                    Ok(changed) => changed,
                    Err(error) => return Ok(Err(error)),
                };
                let title = index
                    .shown_again(&id, ask, changed, keep)
                    .ok()
                    .flatten()
                    .or_else(|| crate::pages::title(&dir, &id))
                    .unwrap_or_else(|| "Page".to_owned());
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
                    if let Err(error) = index.add(&shown, keep) {
                        eprintln!("sikemux core: page {} was not indexed: {error}", page.id);
                    }
                }
                Ok(page)
            })
            .await
            .map_err(|error| error.to_string())??
        }
    };
    core.changed(What::Read(Topic::Pages));
    core.harness().page_shares.wake_now(true);
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
    let _ = pages_dir(core)?;
    let index = core.harness().page_index.clone();
    let words = request.params.get("words").and_then(Value::as_str);
    let words: Vec<String> = (words.unwrap_or_default().split_whitespace())
        .take(8)
        .map(str::to_owned)
        .collect();
    let every = request.params.get("allProjects").and_then(Value::as_bool) == Some(true);
    let project = (!every).then(|| crate::server::caller::project_of(core, request));
    let open_calls = core.harness().questions.open_ids();
    let pages = blocking(move || {
        Ok(index.list(
            &words,
            project.as_deref(),
            &open_calls,
            page_index::MOST_LISTED,
        ))
    })
    .await
    .map_err(|error| error.to_string())??;
    let pages: Vec<ListedPage> = pages
        .into_iter()
        .map(|mut page| {
            page.agent = crate::server::names::at_name(&page.agent);
            page
        })
        .collect();
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
    let _ = pages_dir(core)?;
    let index = core.harness().page_index.clone();
    let words: Vec<String> = (params.words.split_whitespace())
        .take(8)
        .map(str::to_owned)
        .collect();
    let open_calls = core.harness().questions.open_ids();
    blocking(move || {
        index
            .list(
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
    let _ = pages_dir(core)?;
    let index = core.harness().page_index.clone();
    blocking(move || index.keep(&params.id, params.keep).map_err(CoreError::from)).await?;
    core.changed(What::Read(Topic::Pages));
    Ok(())
}

/// Shares a page through a link of its own, which keeps it. A page already shared keeps its link
/// and when it ends.
pub(crate) async fn share_page(core: &Core, params: SharePageParams) -> CoreResult<PageShare> {
    let (_, dir) = pages_dir(core)?;
    let index = core.harness().page_index.clone();
    let initial = read_shared(index.clone(), dir.clone(), params.id.clone()).await?;
    let (link, existing) = match initial.link {
        Some(link) => (
            page_index::Link {
                anyone: params.anyone,
                theme: params.theme,
                told: link.told || params.told,
                ..link
            },
            true,
        ),
        None => {
            let (id, key) = crate::server::share::page::new_link()?;
            let lifetime = params.lifetime_ms.clamp(HOUR_MS, SHARED_AT_MOST_MS);
            (
                page_index::Link {
                    id,
                    key,
                    anyone: params.anyone,
                    expires: epoch_ms() + lifetime,
                    theme: params.theme,
                    told: params.told,
                },
                false,
            )
        }
    };
    let gate = core.harness().page_shares.cancel_and_gate(&link.id);
    let _busy = gate.busy.lock().await;
    let (latest_id, title, html, seen_behind) =
        read_latest(index.clone(), dir.clone(), params.id.clone()).await?;
    let html = blocking(move || {
        crate::pages::for_share(&dir, &latest_id, &html, crate::pages::MAX_SHARED_HTML_BYTES)
            .map_err(CoreError::from)
    })
    .await?;
    if let Err(error) = upload(core, &link, &title, &html).await {
        if let PutError::Transient {
            ref message,
            unreachable: true,
            ..
        } = error
        {
            eprintln!("sikemux core: could not share page {}: {message}", link.id);
        }
        if matches!(error, PutError::Ended) && existing {
            let (index, link_id) = (index.clone(), link.id.clone());
            let _ = blocking(move || index.drop_share(&link_id).map_err(CoreError::from)).await;
            core.harness().page_shares.forget_gate(&link.id);
            core.changed(What::Read(Topic::Pages));
        }
        return Err(error.into());
    }
    let (id, stored) = (params.id, link.clone());
    let still_behind = blocking(move || {
        index
            .shared(&id, &stored, seen_behind)
            .map_err(CoreError::from)
    })
    .await?;
    drop(_busy);
    if still_behind {
        core.harness().page_shares.wake_now(false);
    }
    core.changed(What::Read(Topic::Pages));
    Ok(PageShare {
        url: crate::server::share::page::url(&link.id, &link.key),
        expires_at: link.expires,
        anyone: link.anyone,
        problem: None,
    })
}

/// Ends a page's link. The page stays kept.
pub(crate) async fn stop_page_share(core: &Core, params: PageParams) -> CoreResult<()> {
    let _ = pages_dir(core)?;
    let index = core.harness().page_index.clone();
    let (from, id) = (index.clone(), params.id.clone());
    let link = blocking(move || from.share_of(&id).map_err(CoreError::from)).await?;
    if let Some(link) = link {
        let gate = core.harness().page_shares.cancel_and_gate(&link.id);
        let _busy = gate.busy.lock().await;
        crate::server::share::page::delete(core, &link.id).await?;
        let id = params.id.clone();
        blocking(move || index.unshared(&id).map(drop).map_err(CoreError::from)).await?;
        core.harness().page_shares.forget_gate(&link.id);
    } else {
        blocking(move || {
            index
                .unshared(&params.id)
                .map(drop)
                .map_err(CoreError::from)
        })
        .await?;
    }
    core.changed(What::Read(Topic::Pages));
    Ok(())
}

struct ToShare {
    link: Option<page_index::Link>,
}

async fn read_shared(
    index: Arc<page_index::PageIndex>,
    dir: PathBuf,
    id: String,
) -> CoreResult<ToShare> {
    if !crate::pages::is_page_id(&id) {
        return Err("that is not a page's id".into());
    }
    blocking(move || {
        if crate::pages::page_file(&dir, &id).is_none() {
            return Err("that page is gone".into());
        }
        let link = index.share_of(&id)?;
        Ok(ToShare { link })
    })
    .await
}

async fn read_latest(
    index: Arc<page_index::PageIndex>,
    dir: PathBuf,
    id: String,
) -> CoreResult<(String, String, String, Option<i64>)> {
    blocking(move || {
        let (latest_id, title, behind) = match index.latest_for_page(&id)? {
            Some((latest_id, title, behind)) => (latest_id, title, behind),
            None => {
                let title = index
                    .title_of(&id)
                    .or_else(|| crate::pages::title(&dir, &id))
                    .unwrap_or_else(|| "Page".to_owned());
                (id, title, None)
            }
        };
        let html = crate::pages::read_html(&dir, &latest_id)
            .map_err(|_| CoreError::from("that page is gone"))?;
        Ok((latest_id, title, html, behind))
    })
    .await
}

/// Puts a page's latest version behind its link, in the theme it was shared in, with nothing it
/// may fetch from the network.
async fn upload(
    core: &Core,
    link: &page_index::Link,
    title: &str,
    html: &str,
) -> Result<(), PutError> {
    let html = crate::pages::network_closed(&crate::pages::themed(html, &link.theme, None), &[]);
    let at = (link.id.as_str(), link.key.as_str());
    let sharing = Sharing {
        anyone: link.anyone,
        expires_at: link.expires,
        told: link.told,
    };
    crate::server::share::page::put(core, at, title, &html, sharing).await
}

enum SyncOutcome {
    Idle,
    More,
    Retry(Option<Duration>),
}

/// Keeps shared pages' stored copies up to date with their latest versions, waking only when a
/// page changes, the network or account comes back, or backoff elapses.
pub(crate) async fn serve_uploads(core: Arc<Core>, data_dir: PathBuf) {
    serve_uploads_with(core, data_dir, UPLOAD_BACKOFF).await;
}

async fn serve_uploads_with(core: Arc<Core>, data_dir: PathBuf, backoff: Backoff) {
    let wake = core.harness().page_shares.wake.clone();
    if core.remote().signed_in_api().is_some() {
        core.harness()
            .page_shares
            .signed_in_again
            .store(true, Ordering::Release);
    }
    let mut failures: u32 = 0;
    loop {
        let notified = wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if core
            .harness()
            .page_shares
            .reset_backoff
            .swap(false, Ordering::AcqRel)
        {
            failures = 0;
        }
        if core
            .harness()
            .page_shares
            .signed_in_again
            .swap(false, Ordering::AcqRel)
        {
            let index = core.harness().page_index.clone();
            let cleared = blocking(move || {
                index
                    .clear_problem(SIGNED_OUT_PROBLEM)
                    .map_err(CoreError::from)
            })
            .await;
            if matches!(cleared, Ok(n) if n > 0) {
                core.changed(What::Read(Topic::Pages));
            }
        }

        match sync_pending(&core, &data_dir, failures).await {
            SyncOutcome::Idle => {
                failures = 0;
                notified.await;
            }
            SyncOutcome::More => {
                failures = 0;
            }
            SyncOutcome::Retry(hint) => {
                failures = failures.saturating_add(1);
                let wait = hint.unwrap_or_else(|| backoff.after(failures));
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = &mut notified => {}
                }
            }
        }
    }
}

async fn sync_pending(core: &Arc<Core>, data_dir: &Path, failures: u32) -> SyncOutcome {
    let index = core.harness().page_index.clone();
    let (pending, expired_dropped) = match blocking({
        let index = index.clone();
        move || index.pending_shares().map_err(CoreError::from)
    })
    .await
    {
        Ok(pending) => pending,
        Err(error) => {
            eprintln!("sikemux core: shared pages index could not be read: {error}");
            return SyncOutcome::Retry(None);
        }
    };
    if expired_dropped {
        core.changed(What::Read(Topic::Pages));
    }
    if pending.is_empty() {
        return SyncOutcome::Idle;
    }

    let pages_folder = data_dir.join(crate::pages::FOLDER);
    let mut more = false;
    for link_id in pending {
        let gate = core.harness().page_shares.gate(&link_id);
        let mut cancelled = gate.cancel.subscribe();
        cancelled.borrow_and_update();
        let _busy = gate.busy.lock().await;
        if cancelled.has_changed().unwrap_or(false) {
            more = true;
            continue;
        }
        let (index_read, id) = (index.clone(), link_id.clone());
        let Ok(Some(share)) =
            blocking(move || index_read.pending_for(&id).map_err(CoreError::from)).await
        else {
            continue;
        };
        if share.link.expires <= epoch_ms() {
            let (index_drop, id) = (index.clone(), link_id.clone());
            if matches!(
                blocking(move || index_drop.drop_share(&id).map_err(CoreError::from)).await,
                Ok(true)
            ) {
                core.harness().page_shares.forget_gate(&link_id);
                core.changed(What::Read(Topic::Pages));
            }
            continue;
        }
        let prepared = match share.page_id.clone() {
            Some(page_id) => {
                let dir = pages_folder.clone();
                blocking(move || {
                    let Ok(html) = crate::pages::read_html(&dir, &page_id) else {
                        return Ok(None);
                    };
                    Ok(Some(crate::pages::for_share(
                        &dir,
                        &page_id,
                        &html,
                        crate::pages::MAX_SHARED_HTML_BYTES,
                    )))
                })
                .await
                .ok()
                .flatten()
            }
            None => None,
        };
        let Some(prepared) = prepared else {
            let (index_drop, id) = (index.clone(), link_id.clone());
            if matches!(
                blocking(move || index_drop.drop_share(&id).map_err(CoreError::from)).await,
                Ok(true)
            ) {
                core.harness().page_shares.forget_gate(&link_id);
                core.changed(What::Read(Topic::Pages));
            }
            continue;
        };
        let outcome = match prepared {
            Ok(html) => {
                tokio::select! {
                    biased;
                    _ = cancelled.changed() => {
                        more = true;
                        continue;
                    }
                    outcome = upload(core, &share.link, &share.title, &html) => outcome,
                }
            }
            Err(message) => Err(PutError::Refused(message)),
        };
        match outcome {
            Ok(()) => {
                let (index_sync, id, behind) = (index.clone(), link_id.clone(), share.behind);
                if matches!(
                    blocking(move || index_sync.mark_synced(&id, behind).map_err(CoreError::from))
                        .await,
                    Ok(true)
                ) {
                    more = true;
                }
            }
            Err(PutError::Ended) => {
                let (index_drop, id) = (index.clone(), link_id.clone());
                if matches!(
                    blocking(move || index_drop.drop_share(&id).map_err(CoreError::from)).await,
                    Ok(true)
                ) {
                    core.harness().page_shares.forget_gate(&link_id);
                    core.changed(What::Read(Topic::Pages));
                }
            }
            Err(PutError::SignedOut) => {
                let (index_prob, id, behind) = (index.clone(), link_id.clone(), share.behind);
                if matches!(
                    blocking(move || {
                        index_prob
                            .set_problem(&id, behind, SIGNED_OUT_PROBLEM)
                            .map_err(CoreError::from)
                    })
                    .await,
                    Ok(true)
                ) {
                    core.changed(What::Read(Topic::Pages));
                }
            }
            Err(PutError::Refused(message)) => {
                eprintln!("sikemux core: shared page {link_id} was refused: {message}");
                let (index_prob, id, behind) = (index.clone(), link_id.clone(), share.behind);
                if matches!(
                    blocking(move || {
                        index_prob
                            .set_problem(&id, behind, &message)
                            .map_err(CoreError::from)
                    })
                    .await,
                    Ok(true)
                ) {
                    core.changed(What::Read(Topic::Pages));
                }
            }
            Err(PutError::Transient {
                message,
                retry_after,
                unreachable,
            }) => {
                if failures == 0 || !unreachable {
                    eprintln!("sikemux core: shared page {link_id} was not updated: {message}");
                }
                return SyncOutcome::Retry(retry_after);
            }
        }
    }
    if more {
        SyncOutcome::More
    } else {
        SyncOutcome::Idle
    }
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
    let data_dir = (core.config().data_dir.clone()).ok_or("this core keeps no pages")?;
    let dir = data_dir.join(crate::pages::FOLDER);
    let index = core.harness().page_index.clone();
    let id = params.id.clone();
    let shown = blocking(move || Ok(index.shown_by(&id))).await?;
    let desk = desk_for(core, params.agent, shown);
    blocking(move || {
        use base64::Engine as _;
        let path = crate::pages::page_file(&dir, &params.id)
            .ok_or_else(|| CoreError::from("that page is gone"))?;
        let html = crate::pages::read_file_html(&path)
            .map_err(|_| CoreError::from("that page is gone"))?;
        let picture = std::fs::read(picture_path(&dir, &params.id))
            .ok()
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
        let media =
            Some(crate::pages::media_folder(&dir, &params.id)).filter(|media| media.is_dir());
        Ok(PageHtml {
            html,
            path: Some(path),
            picture,
            media,
            written_in: crate::pages::written_in(&dir, &params.id),
            desk,
        })
    })
    .await
}

/// The open agent whose desk a page opens on: `wanted`, or the one that showed it, else the
/// nearest lead above them still open, else its project's chief.
fn desk_for(
    core: &Core,
    wanted: Option<String>,
    shown: Option<(String, String)>,
) -> Option<String> {
    let open = |agent: &String| core.workspaces().agent(agent).is_some();
    let (agent, project) = shown.unzip();
    let wanted = wanted.or(agent)?;
    let chief = || crate::server::chief::chief_at(core, project.as_deref()?);
    std::iter::once(wanted.clone())
        .chain(core.names().leads_of(&wanted))
        .find(open)
        .or_else(|| chief().filter(open))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::remote;
    use crate::server::remote_state::RemoteParts;
    use crate::server::ServerCore;
    use iroh::SecretKey;
    use std::collections::HashMap;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    #[derive(Default)]
    struct ServerState {
        stored: HashMap<String, String>,
        ended: HashMap<String, bool>,
        puts: usize,
        failing: Option<u16>,
        hold_next_put: Option<Arc<tokio::sync::Notify>>,
        put_arrived: Option<Arc<tokio::sync::Notify>>,
    }

    async fn stand_in_server(state: Arc<Mutex<ServerState>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = state.clone();
                tokio::spawn(async move {
                    let mut stream = BufReader::new(stream);
                    let mut first = String::new();
                    if stream.read_line(&mut first).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let mut length = 0usize;
                    loop {
                        let mut line = String::new();
                        if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                            break;
                        }
                        let lower = line.trim().to_ascii_lowercase();
                        if lower.is_empty() {
                            break;
                        }
                        if let Some(value) = lower.strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0; length];
                    if length > 0 {
                        let _ = stream.read_exact(&mut body).await;
                    }
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or("");
                    let path = parts.next().unwrap_or("");
                    let link_id = path.strip_prefix("/v1/pages/").unwrap_or("").to_owned();
                    if method == "PUT" {
                        let hold = {
                            let mut s = state.locked();
                            if let Some(arrived) = s.put_arrived.take() {
                                arrived.notify_one();
                            }
                            s.hold_next_put.take()
                        };
                        if let Some(hold) = hold {
                            hold.notified().await;
                        }
                    }
                    let (status, answer) = {
                        let mut s = state.locked();
                        match method {
                            "PUT" => {
                                s.puts += 1;
                                if let Some(code) = s.failing {
                                    (
                                        code,
                                        json!({ "error": { "code": "refused", "message": format!("status {code}") } }).to_string(),
                                    )
                                } else if s.ended.get(&link_id).copied().unwrap_or(false) {
                                    (
                                        404,
                                        json!({ "error": { "code": "not_found", "message": "That share has ended." } }).to_string(),
                                    )
                                } else {
                                    s.stored
                                        .insert(link_id.clone(), String::from_utf8(body).unwrap());
                                    (
                                        200,
                                        json!({ "id": link_id, "expiresAt": epoch_ms() + HOUR_MS, "views": 0 }).to_string(),
                                    )
                                }
                            }
                            "DELETE" => {
                                s.ended.insert(link_id.clone(), true);
                                s.stored.remove(&link_id);
                                (
                                    200,
                                    json!({ "id": link_id, "expiresAt": epoch_ms() + HOUR_MS, "views": 0, "ended": true }).to_string(),
                                )
                            }
                            _ => (404, "{}".to_owned()),
                        }
                    };
                    let reply = format!(
                        "HTTP/1.1 {status} Answer\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                        answer.len()
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
        base
    }

    async fn wait_until(mut check: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("condition was not met in time");
    }

    fn show_request(project: &str, agent: &str, path: &Path, title: &str) -> HarnessRequest {
        HarnessRequest {
            id: "show".into(),
            project: project.into(),
            agent_id: Some(agent.into()),
            method: "page.show".into(),
            params: json!({
                "path": path.to_string_lossy(),
                "title": title,
            }),
        }
    }

    fn signed_in_core(dir: &Path, base: &str) -> Arc<Core> {
        let core = Core::new(Default::default(), Some(dir)).unwrap();
        core.remote()
            .stand_in(SecretKey::generate(), Some("user_2abc"), Vec::new());
        core.remote().set_accounts_api(base);
        core
    }

    #[tokio::test]
    async fn a_shared_page_retries_failed_uploads_without_reviving_or_reordering() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(ServerState::default()));
        let base = stand_in_server(state.clone()).await;
        const LONG_BACKOFF: Backoff = Backoff {
            first: Duration::from_secs(60),
            cap: Duration::from_secs(60),
        };

        let core = signed_in_core(dir.path(), &base);
        let uploader = tokio::spawn(serve_uploads_with(
            core.clone(),
            dir.path().to_owned(),
            LONG_BACKOFF,
        ));

        let src = dir.path().join("report.html");
        std::fs::write(&src, "<p>v1</p>").unwrap();
        let shown = show_page(&core, show_request("/proj", "agent-1", &src, "Report"))
            .await
            .unwrap();
        let v1_id = shown["page"]["id"].as_str().unwrap().to_owned();

        let shared = share_page(
            &core,
            SharePageParams {
                id: v1_id,
                lifetime_ms: HOUR_MS,
                anyone: true,
                told: false,
                theme: String::new(),
            },
        )
        .await
        .unwrap();
        let link_id = shared
            .url
            .rsplit('/')
            .next()
            .unwrap()
            .split('#')
            .next()
            .unwrap()
            .to_owned();
        let v1_len = state.locked().stored[&link_id].len();

        // 1. Failing upload retries immediately when network/wake returns.
        state.locked().failing = Some(503);
        let v2_html = format!("<p>v2 {}</p>", "a".repeat(400));
        std::fs::write(&src, &v2_html).unwrap();
        show_page(&core, show_request("/proj", "agent-1", &src, "Report"))
            .await
            .unwrap();
        wait_until(|| state.locked().puts >= 2).await;
        assert_eq!(state.locked().stored[&link_id].len(), v1_len);

        state.locked().failing = None;
        remote::network_back(&core).await;
        wait_until(|| state.locked().stored[&link_id].len() > v1_len + 400).await;
        let v2_len = state.locked().stored[&link_id].len();

        // 2. Two versions close together end with the newest stored, never the older one over it.
        let arrived = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        {
            let mut s = state.locked();
            s.put_arrived = Some(arrived.clone());
            s.hold_next_put = Some(release.clone());
        }
        std::fs::write(&src, format!("<p>v3 {}</p>", "b".repeat(800))).unwrap();
        show_page(&core, show_request("/proj", "agent-1", &src, "Report"))
            .await
            .unwrap();
        arrived.notified().await;
        std::fs::write(&src, format!("<p>v4 {}</p>", "c".repeat(1600))).unwrap();
        show_page(&core, show_request("/proj", "agent-1", &src, "Report"))
            .await
            .unwrap();
        release.notify_one();
        wait_until(|| state.locked().stored[&link_id].len() > v2_len + 1200).await;
        let v4_len = state.locked().stored[&link_id].len();

        // 3. Permanent refusal (413) stops retrying and shows in library(); signed-out (401)
        // stops retrying until sign-in comes back.
        state.locked().failing = Some(413);
        std::fs::write(&src, format!("<p>v5 {}</p>", "d".repeat(2000))).unwrap();
        show_page(&core, show_request("/proj", "agent-1", &src, "Report"))
            .await
            .unwrap();
        wait_until(|| {
            core.harness()
                .page_index
                .list(&[], Some("/proj"), &HashSet::new(), 10)
                .ok()
                .and_then(|pages| pages.into_iter().next()?.shared?.problem)
                .is_some()
        })
        .await;
        let listed = library(
            &core,
            PagesParams {
                project: "/proj".into(),
                words: String::new(),
            },
        )
        .await
        .unwrap();
        let problem = listed[0].shared.as_ref().unwrap().problem.as_deref();
        assert!(problem.is_some_and(|p| p.contains("25 MiB")), "{problem:?}");
        let puts = state.locked().puts;
        remote::network_back(&core).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(state.locked().puts, puts, "permanent 413 must not retry");
        assert_eq!(state.locked().stored[&link_id].len(), v4_len);

        state.locked().failing = Some(401);
        std::fs::write(&src, format!("<p>v6 {}</p>", "e".repeat(2400))).unwrap();
        show_page(&core, show_request("/proj", "agent-1", &src, "Report"))
            .await
            .unwrap();
        wait_until(|| {
            core.harness()
                .page_index
                .list(&[], Some("/proj"), &HashSet::new(), 10)
                .ok()
                .and_then(|pages| pages.into_iter().next()?.shared?.problem)
                .as_deref()
                == Some(SIGNED_OUT_PROBLEM)
        })
        .await;
        let puts = state.locked().puts;
        remote::network_back(&core).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            state.locked().puts,
            puts,
            "signed-out refusal waits for sign-in"
        );
        state.locked().failing = None;
        signed_in(&core);
        wait_until(|| state.locked().stored[&link_id].len() > v4_len + 800).await;
        let v6_len = state.locked().stored[&link_id].len();

        // 4. Surviving a core restart: a behind version left on disk uploads when a new core starts.
        uploader.abort();
        std::fs::write(&src, format!("<p>v7 {}</p>", "f".repeat(3000))).unwrap();
        let shown_v7 = show_page(&core, show_request("/proj", "agent-1", &src, "Report"))
            .await
            .unwrap();
        let v7_id = shown_v7["page"]["id"].as_str().unwrap().to_owned();
        drop(core);

        let restarted = signed_in_core(dir.path(), &base);
        let uploader = tokio::spawn(serve_uploads_with(
            restarted.clone(),
            dir.path().to_owned(),
            LONG_BACKOFF,
        ));
        wait_until(|| state.locked().stored[&link_id].len() > v6_len + 600).await;

        // 5. Stopping a share while an upload waits drops it and never revives it.
        let puts = state.locked().puts;
        state.locked().failing = Some(503);
        std::fs::write(&src, format!("<p>v8 {}</p>", "g".repeat(3400))).unwrap();
        show_page(&restarted, show_request("/proj", "agent-1", &src, "Report"))
            .await
            .unwrap();
        wait_until(|| state.locked().puts > puts).await;
        stop_page_share(&restarted, PageParams::new(&v7_id))
            .await
            .unwrap();
        // An upload the stop cut off may still reach the server.
        tokio::time::sleep(Duration::from_millis(60)).await;
        let puts = state.locked().puts;
        state.locked().failing = None;
        remote::network_back(&restarted).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            state.locked().puts,
            puts,
            "stopped share must never upload again"
        );
        assert!(!state.locked().stored.contains_key(&link_id));
        uploader.abort();
    }
}
