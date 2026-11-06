//! The conversations the agent CLIs saved on this host: listing them, and
//! renaming, deleting or moving one to another project the way each
//! provider's own commands do.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent_catalog::AgentKind;
use crate::sessions::{self, recent::RecentRequest, SavedRef};

use super::connection::blocking;
use super::{Core, CoreResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Listing {
    agent: AgentKind,
    cwd: String,
    config_path: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Recent {
    request: RecentRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Renamed {
    #[serde(flatten)]
    saved: SavedRef,
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Moved {
    agent: AgentKind,
    from_cwd: String,
    to_cwd: String,
    session_id: String,
    config_path: Option<String>,
}

pub(super) async fn list(listing: Listing) -> CoreResult<Vec<sessions::AgentSession>> {
    blocking(move || {
        Ok(sessions::agent_sessions(
            listing.agent,
            &listing.cwd,
            listing.config_path.as_deref(),
        ))
    })
    .await
}

pub(super) async fn recent(recent: Recent) -> CoreResult<sessions::recent::RecentPage> {
    blocking(move || Ok(sessions::recent::recent_page(recent.request)?)).await
}

pub(super) async fn rename(renamed: Renamed) -> CoreResult<()> {
    Ok(sessions::rename::rename(renamed.saved, renamed.title).await?)
}

pub(super) async fn delete(core: &Arc<Core>, saved: SavedRef) -> CoreResult<()> {
    let session_id = saved.session_id.clone();
    sessions::delete::delete(saved).await?;
    super::board::lease::session_deleted(core, &session_id);
    Ok(())
}

pub(super) async fn relocate(moved: Moved) -> CoreResult<()> {
    Ok(sessions::relocate::relocate(
        moved.agent,
        moved.from_cwd,
        moved.to_cwd,
        moved.session_id,
        moved.config_path,
    )
    .await?)
}

/// A recent chat, named as the workspace lists it.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentChat {
    pub recent: String,
    /// The folder it ran in, for a chat the core no longer lists.
    pub cwd: Option<PathBuf>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRenamed {
    pub recent: String,
    pub title: String,
    pub cwd: Option<PathBuf>,
}

/// Renames a recent chat in its CLI's own history, as the launcher that found it signs in.
pub(super) async fn rename_recent(core: &Arc<Core>, renamed: RecentRenamed) -> CoreResult<()> {
    let session = core
        .workspaces
        .recent_session(&renamed.recent, renamed.cwd)?;
    sessions::rename::rename(session, renamed.title).await?;
    core.workspaces.recent_stale();
    Ok(())
}

/// Deletes a recent chat from its CLI's own history, for good.
pub(super) async fn delete_recent(core: &Arc<Core>, chat: RecentChat) -> CoreResult<()> {
    let session = core.workspaces.recent_session(&chat.recent, chat.cwd)?;
    delete(core, session).await?;
    core.workspaces.recent_stale();
    Ok(())
}
