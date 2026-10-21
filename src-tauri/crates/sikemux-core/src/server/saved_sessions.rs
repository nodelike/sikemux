//! The conversations the agent CLIs saved on this host: listing them, and
//! renaming, deleting or moving one to another project the way each
//! provider's own commands do.

use serde::Deserialize;

use crate::agent_catalog::AgentKind;
use crate::sessions::{self, recent::RecentRequest};

use super::connection::blocking;
use super::CoreResult;

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
