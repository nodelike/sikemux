//! What devices draw behind their panes: the host's grain, and the picture in its place, taken
//! from the person's preferences so that no app has to send it.

use std::sync::Arc;

use super::Core;
use crate::protocol::BackdropImage;

/// Publishes the backdrop the preferences describe, shrinking a newly chosen picture first.
pub(crate) fn follow(core: &Arc<Core>) {
    let preferences = core.preferences.get();
    let texture = preferences.pane_shader;
    let Some(path) = preferences.pane_image.filter(|_| texture) else {
        publish(core, texture, None);
        return;
    };
    let core = core.clone();
    tokio::task::spawn_blocking(move || {
        let id = picture_id(&path);
        if id.is_some() && id == core.workspaces.backdrop_image_id() {
            return;
        }
        let image = id.and_then(|id| shrunk(&path, id));
        let now = core.preferences.get();
        if now.pane_shader && now.pane_image.as_deref() == Some(path.as_str()) {
            publish(&core, true, image);
        }
    });
}

fn publish(core: &Core, texture: bool, image: Option<BackdropImage>) {
    match core.workspaces.publish_backdrop(texture, image) {
        Ok(()) => core.view_changed(),
        Err(error) => eprintln!("sikemux core: the pane picture is not sent to devices: {error}"),
    }
}

/// Names the picture as it is on disk now, so a device fetches it again only once it changes.
fn picture_id(path: &str) -> Option<String> {
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
