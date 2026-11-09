//! What Settings' About page and the first screen ask of the host: a
//! release's notes and who made it, and which of the tools Sikemux works with
//! the host has.

use sikemux_view::prefs::PreferenceChange;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sikemux_process::user_environment::find_executable;

use super::connection::blocking;
use super::{Core, CoreError, CoreResult};
use crate::releases;

#[derive(Serialize, Deserialize)]
pub struct NotesParams {
    pub version: String,
}

pub(crate) async fn release_notes(params: NotesParams) -> CoreResult<releases::ReleaseNotes> {
    releases::release_notes(params.version)
        .await
        .map_err(CoreError::from)
}

#[derive(Serialize, Deserialize)]
pub struct AvatarsParams {
    pub urls: Vec<String>,
}

/// Contributors' pictures as `data:` addresses, by the address GitHub gave.
pub(crate) async fn release_avatars(params: AvatarsParams) -> HashMap<String, String> {
    releases::release_avatars(params.urls).await
}

/// The release running here, and whether the person has yet to read its notes.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsNew {
    pub version: String,
    /// Shown once the introduction is done, for a release the person has not
    /// marked seen by setting `lastSeenVersion` to it.
    pub unseen: bool,
}

pub(crate) fn whats_new(core: &Core) -> WhatsNew {
    let preferences = core.preferences.get();
    let version = core.config.build.version.clone();
    WhatsNew {
        unseen: preferences.onboarding_complete && preferences.last_seen_version != version,
        version,
    }
}

/// A first run has nothing new to show, so finishing the introduction counts
/// the release running as seen.
pub(crate) fn introduced(core: &Core) -> CoreResult<()> {
    let preferences = core.preferences.get();
    if preferences.onboarding_complete && preferences.last_seen_version.is_empty() {
        core.preferences.change(PreferenceChange::LastSeenVersion(
            core.config.build.version.clone(),
        ));
    }
    Ok(())
}

/// The person's shell, and whether git, Homebrew and the plugins' own
/// command-line tools are on the host.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tools {
    pub shell: String,
    pub git: bool,
    pub brew: bool,
    pub aws: bool,
    pub rnd: bool,
}

/// Whether a git that runs is here. A Mac without the developer tools has a stand-in at
/// /usr/bin/git that only offers to install them.
fn git_works() -> bool {
    let Some(git) = find_executable("git") else {
        return false;
    };
    if !cfg!(target_os = "macos") || git != std::path::Path::new("/usr/bin/git") {
        return true;
    }
    sikemux_process::user_environment::command("/usr/bin/xcode-select")
        .arg("-p")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) async fn tools() -> CoreResult<Tools> {
    blocking(|| {
        Ok(Tools {
            shell: sikemux_pty::user_shell::configured_shell(),
            git: git_works(),
            brew: find_executable("brew").is_some(),
            aws: find_executable("aws").is_some(),
            rnd: find_executable("rnd").is_some(),
        })
    })
    .await
}
