//! The conversations the agent CLIs saved on this host: listing them, and
//! renaming, deleting or moving one to another project the way each
//! provider's own commands do.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;

use crate::agent_catalog::AgentKind;
use crate::sessions::{self, recent::RecentRequest};

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
pub(super) struct Saved {
    agent: AgentKind,
    cwd: String,
    session_id: String,
    executable_path: Option<String>,
    config_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Renamed {
    #[serde(flatten)]
    saved: Saved,
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
    let saved = renamed.saved;
    Ok(sessions::rename::rename(
        saved.agent,
        saved.cwd,
        saved.session_id,
        renamed.title,
        saved.executable_path,
        saved.config_path,
    )
    .await?)
}

pub(super) async fn delete(saved: Saved) -> CoreResult<()> {
    Ok(sessions::delete::delete(
        saved.agent,
        saved.cwd,
        saved.session_id,
        saved.executable_path,
        saved.config_path,
    )
    .await?)
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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RecentChat {
    recent: String,
    /// The folder it ran in, for a chat the core no longer lists.
    cwd: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RecentRenamed {
    recent: String,
    title: String,
    cwd: Option<PathBuf>,
}

/// Renames a recent chat in its CLI's own history, as the launcher that found it signs in.
pub(super) async fn rename_recent(core: &Arc<Core>, renamed: RecentRenamed) -> CoreResult<()> {
    let session = core
        .workspaces
        .recent_session(&renamed.recent, renamed.cwd)?;
    sessions::rename::rename(
        session.agent,
        session.cwd,
        session.session_id,
        renamed.title,
        session.program,
        session.config_path,
    )
    .await?;
    core.workspaces.recent_stale();
    Ok(())
}

/// Deletes a recent chat from its CLI's own history, for good.
pub(super) async fn delete_recent(core: &Arc<Core>, chat: RecentChat) -> CoreResult<()> {
    let session = core.workspaces.recent_session(&chat.recent, chat.cwd)?;
    sessions::delete::delete(
        session.agent,
        session.cwd,
        session.session_id,
        session.program,
        session.config_path,
    )
    .await?;
    core.workspaces.recent_stale();
    Ok(())
}
