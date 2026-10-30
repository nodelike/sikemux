//! What Settings' About page and the first screen ask of the host: a
//! release's notes and who made it, and which of the tools Sikemux works with
//! the host has.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sikemux_process::user_environment::find_executable;

use super::connection::blocking;
use super::{Core, CoreError, CoreResult};
use crate::releases;

#[derive(Deserialize)]
pub(crate) struct NotesParams {
    version: String,
}

pub(crate) async fn release_notes(params: NotesParams) -> CoreResult<releases::ReleaseNotes> {
    releases::release_notes(params.version)
        .await
        .map_err(CoreError::from)
}

#[derive(Deserialize)]
pub(crate) struct AvatarsParams {
    urls: Vec<String>,
}

/// Contributors' pictures as `data:` addresses, by the address GitHub gave.
pub(crate) async fn release_avatars(params: AvatarsParams) -> HashMap<String, String> {
    releases::release_avatars(params.urls).await
}

/// The release running here, and whether the person has yet to read its notes.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WhatsNew {
    version: String,
    /// Shown once the introduction is done, for a release the person has not
    /// marked seen by setting `lastSeenVersion` to it.
    unseen: bool,
}

pub(crate) fn whats_new(core: &Core) -> WhatsNew {
    let preferences = core.preferences.get();
    let version = core.build.version.clone();
    WhatsNew {
        unseen: preferences.onboarding_complete && preferences.last_seen_version != version,
        version,
    }
}

/// A first run has nothing new to show, so finishing the introduction counts
/// the release running as seen.
pub(crate) fn introduced(core: &Core, key: &str) -> CoreResult<()> {
    let preferences = core.preferences.get();
    if key == "onboardingComplete"
        && preferences.onboarding_complete
        && preferences.last_seen_version.is_empty()
    {
        core.preferences
            .set("lastSeenVersion", core.build.version.clone().into())?;
    }
    Ok(())
}

/// The person's shell, and whether git, Homebrew and the plugins' own
/// command-line tools are on the host.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tools {
    shell: String,
    git: bool,
    brew: bool,
    aws: bool,
    rnd: bool,
}

pub(crate) async fn tools() -> CoreResult<Tools> {
    blocking(|| {
        Ok(Tools {
            shell: sikemux_pty::user_shell::configured_shell(),
            git: find_executable("git").is_some(),
            brew: find_executable("brew").is_some(),
            aws: find_executable("aws").is_some(),
            rnd: find_executable("rnd").is_some(),
        })
    })
    .await
}
