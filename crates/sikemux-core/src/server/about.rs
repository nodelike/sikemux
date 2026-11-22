//! What Settings' About page and the first screen ask of the host: a
//! release's notes and who made it, and which of the tools Sikemux works with
//! the host has.

use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::about::{AvatarsParams, NotesParams, Tools, WhatsNew};
use sikemux_view::prefs::PreferenceChange;
use std::collections::HashMap;

use sikemux_process::user_environment::find_executable;

use super::connection::blocking;
use super::{Core, CoreError, CoreResult};
use crate::releases;

pub(crate) async fn release_notes(
    core: &Core,
    params: NotesParams,
) -> CoreResult<releases::ReleaseNotes> {
    let dir = core.config().data_dir.as_deref();
    match params.fetch {
        true => releases::release_notes(params.version, params.since, dir).await,
        false => releases::held_notes(params.version, params.since, dir),
    }
    .map_err(CoreError::from)
}

/// Contributors' pictures as `data:` addresses, by the address GitHub gave.
pub(crate) async fn release_avatars(params: AvatarsParams) -> HashMap<String, String> {
    releases::release_avatars(params.urls).await
}

/// An app that asks before updating showed the notes before the update went in, so only
/// one that updates by itself has notes left to show once it runs the new release. A nightly
/// installs itself after every idle break, so its notes wait to be asked for.
pub(crate) fn whats_new(core: &Core) -> WhatsNew {
    let preferences = core.preferences().get();
    let version = core.config().build.version.clone();
    let unread = super::updates::updates_by_itself(core)
        && preferences.onboarding_complete
        && preferences.last_seen_version != version;
    let nightly = version.contains("-nightly.");
    let since = Some(preferences.last_seen_version).filter(|seen| !seen.is_empty());
    WhatsNew {
        unseen: unread && !nightly,
        since: since.filter(|_| unread && nightly),
        version,
    }
}

/// A first run has nothing new to show, so finishing the introduction counts
/// the release running as seen.
pub(crate) fn introduced(core: &Core) -> CoreResult<()> {
    let preferences = core.preferences().get();
    if preferences.onboarding_complete && preferences.last_seen_version.is_empty() {
        core.preferences().change(PreferenceChange::LastSeenVersion(
            core.config().build.version.clone(),
        ));
    }
    Ok(())
}

/// Whether a git that runs is here. On Windows a miss reads the person's PATH again, so a Git
/// for Windows installed since is found. A Mac without the developer tools has a stand-in at
/// /usr/bin/git that only offers to install them.
fn git_works() -> bool {
    let found = find_executable("git").or_else(|| {
        cfg!(windows).then(sikemux_process::user_environment::reread)?;
        find_executable("git")
    });
    let Some(git) = found else {
        return false;
    };
    if !cfg!(target_os = "macos") || git != std::path::Path::new("/usr/bin/git") {
        return true;
    }
    sikemux_process::user_environment::command("/usr/bin/xcode-select")
        .arg("-p")
        .stdout(sikemux_process::command::Stdio::null())
        .stderr(sikemux_process::command::Stdio::null())
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
