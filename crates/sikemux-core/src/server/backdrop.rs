//! What devices draw behind their panes: the host's grain, and the picture in its place, taken
//! from the person's preferences so that no app has to send it.

use crate::server::parts::Parts;
use sikemux_view::prefs::{PaneColours, Preferences};
use sikemux_view::theme::UiTokens;
use sikemux_view::workspace::BackdropLook;

use super::workspace::BackdropImage;
use super::Core;
use crate::server::changed::What;

/// Publishes the backdrop the preferences describe, shrinking a newly chosen picture first.
pub(crate) fn follow(core: &Core) {
    let Some(core) = core.arc() else {
        return;
    };
    let preferences = core.shown_preferences();
    let texture = preferences.pane_shader;
    let Some(path) = preferences.pane_image.filter(|_| texture) else {
        publish(&core, texture, None);
        return;
    };
    tokio::task::spawn_blocking(move || {
        let id = picture_id(&path);
        if id.is_some() && id == core.workspaces().backdrop_image_id() {
            return;
        }
        let image = id.and_then(|id| shrunk(&path, id));
        let now = core.shown_preferences();
        if now.pane_shader && now.pane_image.as_deref() == Some(path.as_str()) {
            publish(&core, true, image);
        }
    });
}

/// How the host's panes draw their backdrop, with the inks a themed picture takes resolved.
pub(crate) fn look(preferences: &Preferences, tokens: &UiTokens) -> BackdropLook {
    let inks = match preferences.pane_colours {
        PaneColours::Custom => (0..3)
            .map(|at| preferences.pane_inks.get(at).cloned())
            .collect(),
        PaneColours::Theme | PaneColours::Picture => {
            vec![None, Some(tokens.accent.clone()), Some(tokens.ink.clone())]
        }
    };
    BackdropLook {
        strength: preferences.pane_strength,
        inks,
        own_colours: preferences.pane_colours == PaneColours::Picture,
        pattern: preferences.pane_pattern,
        dot_size: preferences.pane_dot_size,
        contrast: preferences.pane_contrast,
        brightness: preferences.pane_brightness,
        glow: preferences.pane_glow,
        glow_size: preferences.pane_glow_size,
        coverage: preferences.pane_coverage,
    }
}

fn publish(core: &Core, texture: bool, image: Option<BackdropImage>) {
    match core.workspaces().publish_backdrop(texture, image) {
        Ok(()) => core.changed(What::View),
        Err(error) => eprintln!("sikemux core: the pane picture is not sent to devices: {error}"),
    }
}

/// Names the picture as it is on disk now, so a device fetches it again only once it changes.
pub(crate) fn picture_id(path: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let named = format!("{path}\0{}\0{}", metadata.len(), modified.as_nanos());
    let hash = Sha256::digest(named.as_bytes());
    Some(hash[..8].iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The picture as a JPEG no longer than a phone draws it, by macOS's `sips`.
#[cfg(target_os = "macos")]
fn shrunk(path: &str, id: String) -> Option<BackdropImage> {
    use base64::Engine;
    use std::process::Command;
    use std::time::Duration;

    /// A phone draws the picture across its width at most a few times over; past this it only
    /// adds bytes.
    const LONGEST_EDGE: u32 = 1280;
    const TIMEOUT: Duration = Duration::from_secs(10);

    let sips = || sikemux_process::user_environment::command("/usr/bin/sips");
    let run = |command: &mut Command| {
        sikemux_process::run(command, None, TIMEOUT, 64 * 1024, None)
            .ok()
            .filter(|done| done.status.success())
    };
    let sized = run(sips()
        .args(["-g", "pixelWidth", "-g", "pixelHeight"])
        .arg(path))?;
    let longest = String::from_utf8_lossy(&sized.stdout)
        .lines()
        .filter_map(|line| line.split_once(':')?.1.trim().parse::<u32>().ok())
        .max()?;
    let dir = tempfile::tempdir().ok()?;
    let out = dir.path().join("backdrop.jpg");
    let mut shrink = sips();
    if longest > LONGEST_EDGE {
        shrink.args(["-Z", &LONGEST_EDGE.to_string()]);
    }
    shrink
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "82"])
        .arg(path)
        .arg("--out")
        .arg(&out);
    run(&mut shrink)?;
    let bytes = std::fs::read(&out).ok()?;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(BackdropImage {
        id,
        data_url: format!("data:image/jpeg;base64,{data}"),
    })
}

#[cfg(not(target_os = "macos"))]
fn shrunk(_: &str, _: String) -> Option<BackdropImage> {
    None
}
