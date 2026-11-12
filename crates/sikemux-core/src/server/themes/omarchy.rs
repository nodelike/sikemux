//! The theme Omarchy, an Arch Linux desktop, sets for the whole system. It
//! writes the current theme's colours to a folder it replaces on every switch,
//! so Sikemux offers one theme, Omarchy, that follows it.

use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use crate::server::ServerCore;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde::Deserialize;
use sikemux_view::theme::Theme;

use sikemux_process::sync::Locked;

use crate::server::backdrop;
use crate::server::changed::{Topic, What};
use crate::server::connection::blocking;
use crate::server::Core;

pub(crate) const THEME_ID: &str = "omarchy";
/// A switch replaces the folder and then stamps it, several writes in a row.
const SETTLE: Duration = Duration::from_millis(300);
/// The folder under the core's data that holds its copy of Omarchy's background.
const BACKGROUND_COPIES: &str = "omarchy-background";
/// Omarchy can also show a video, which panes cannot dither.
const STILL_PICTURES: [&str; 6] = ["jpg", "jpeg", "png", "webp", "bmp", "gif"];

/// Where Omarchy keeps the theme it set; only Linux runs Omarchy.
fn current() -> Option<PathBuf> {
    cfg!(target_os = "linux")
        .then(sikemux_process::paths::home)
        .flatten()
        .map(|home| home.join(".local/state/omarchy/current"))
}

/// Whether this is an Omarchy desktop with a theme set.
pub(crate) fn here() -> bool {
    current().is_some_and(|current| current.join("theme/colors.toml").is_file())
}

/// The theme Omarchy set, when this is an Omarchy desktop.
pub(super) fn read() -> Option<Theme> {
    read_from(&current()?)
}

fn read_from(current: &Path) -> Option<Theme> {
    let colors = std::fs::read_to_string(current.join("theme/colors.toml")).ok()?;
    let name = std::fs::read_to_string(current.join("theme.name")).ok();
    from_colors(&colors, name.as_deref())
}

/// The colours an Omarchy theme's `colors.toml` names, which Omarchy fills
/// each app's own theme file from.
#[derive(Deserialize)]
struct Colors {
    accent: Option<String>,
    selection: Option<String>,
    muted: String,
    background: String,
    foreground: String,
    bright_foreground: Option<String>,
    red: String,
    green: String,
    yellow: String,
    blue: String,
    magenta: String,
    cyan: String,
    bright_red: String,
    bright_green: String,
    bright_yellow: String,
    bright_blue: String,
    bright_magenta: String,
    bright_cyan: String,
}

/// The theme `colors.toml` describes, its terminal colours placed as Omarchy
/// places them in Ghostty's theme, under the name in `theme.name`.
fn from_colors(colors: &str, name: Option<&str>) -> Option<Theme> {
    let c: Colors = toml::from_str(colors).ok()?;
    let bright_foreground = c.bright_foreground.unwrap_or_else(|| c.foreground.clone());
    let colours = [
        &c.background,
        &c.foreground,
        &bright_foreground,
        &c.background,
        c.selection.as_ref().unwrap_or(&String::new()),
        &c.background,
        &c.red,
        &c.green,
        &c.yellow,
        &c.blue,
        &c.magenta,
        &c.cyan,
        &c.foreground,
        &c.muted,
        &c.bright_red,
        &c.bright_green,
        &c.bright_yellow,
        &c.bright_blue,
        &c.bright_magenta,
        &c.bright_cyan,
        &bright_foreground,
    ]
    .map(|colour| colour.trim().to_owned());
    let mut theme = super::from_colours("Omarchy", &colours, c.accent.as_deref())?;
    theme.id = THEME_ID.to_owned();
    if let Some(name) = name.map(display_name).filter(|name| !name.is_empty()) {
        theme.name = format!("Omarchy · {name}");
    }
    Some(theme)
}

/// `matte-black` as Omarchy shows it: Matte Black.
fn display_name(name: &str) -> String {
    name.trim()
        .split(['-', '_', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut letters = word.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reads the theme again each time Omarchy switches it, and redraws every
/// app when the person wears it.
pub(super) async fn follow(core: Arc<Core>) {
    let Some(current) = current().filter(|current| current.is_dir()) else {
        return;
    };
    wear_background_off_thread(&core).await;
    switches(current.clone(), || switched(core.clone(), current.clone())).await;
}

async fn switched(core: Arc<Core>, current: PathBuf) {
    // Mid-switch the folder can be gone; the last theme stays until the new one is in place.
    if let Ok(Some(theme)) = blocking(move || Ok(read_from(&current))).await {
        if core.look().set_omarchy(theme) {
            if core.preferences().get().theme_id == THEME_ID {
                core.theme_changed();
            }
            core.changed(What::Read(Topic::Preferences));
        }
    }
    wear_background_off_thread(&core).await;
}

/// Takes up Omarchy's background now that the person has asked panes to follow it.
pub(crate) fn follow_background(core: &Core) {
    let Some(core) = core.arc() else {
        return;
    };
    tokio::task::spawn_blocking(move || wear_background(&core));
}

async fn wear_background_off_thread(core: &Arc<Core>) {
    let core = core.clone();
    let _ = blocking(move || {
        wear_background(&core);
        Ok(())
    })
    .await;
}

/// Shows Omarchy's current background in panes. One that cannot be read yet, or is a video,
/// leaves the last picture up.
fn wear_background(core: &Arc<Core>) {
    if !core.preferences().get().pane_image_follows_omarchy {
        return;
    }
    let (Some(current), Some(data_dir)) = (current(), core.config().data_dir.as_deref()) else {
        return;
    };
    let _copying = core.look().copying_omarchy_background.locked();
    let Some(background) = copy_background(&current, &data_dir.join(BACKGROUND_COPIES)) else {
        return;
    };
    if core.look().set_omarchy_background(background) {
        backdrop::follow(core);
        core.changed(What::Read(Topic::Preferences));
    }
}

/// Copies the background into `copies`, returning the copy's path. Omarchy deletes the original
/// on its next switch and reuses file names between themes, so apps are given a path that only
/// ever means this one picture.
fn copy_background(current: &Path, copies: &Path) -> Option<String> {
    let picture = std::fs::canonicalize(current.join("background")).ok()?;
    let extension = picture.extension()?.to_str()?.to_ascii_lowercase();
    if !STILL_PICTURES.contains(&extension.as_str()) {
        return None;
    }
    let id = backdrop::picture_id(picture.to_str()?)?;
    let copy = copies.join(format!("{id}.{extension}"));
    if !copy.exists() {
        std::fs::create_dir_all(copies).ok()?;
        let unfinished = copies.join(format!("{id}.part"));
        std::fs::copy(&picture, &unfinished).ok()?;
        std::fs::rename(&unfinished, &copy).ok()?;
        for entry in std::fs::read_dir(copies).ok()?.flatten() {
            if entry.path() != copy {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    copy.to_str().map(str::to_owned)
}

/// Calls `switched` after each change to the folder settles.
/// Omarchy replaces its `theme` folder on a switch, so this watches the folder around it.
async fn switches<Done: Future<Output = ()>>(current: PathBuf, mut switched: impl FnMut() -> Done) {
    let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let Ok(mut watcher) =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok() {
                let _ = changed.send(());
            }
        })
    else {
        return;
    };
    if watcher
        .watch(&current, RecursiveMode::NonRecursive)
        .is_err()
    {
        return;
    }
    while changes.recv().await.is_some() {
        tokio::time::sleep(SETTLE).await;
        while changes.try_recv().is_ok() {}
        switched().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_comes_from_omarchys_colours() {
        let theme = from_colors(
            include_str!("../fixtures/omarchy/matte-black.colors.toml"),
            Some("matte-black\n"),
        )
        .expect("matte black's colours make a theme");
        assert_eq!(theme.id, THEME_ID);
        assert_eq!(theme.name, "Omarchy · Matte Black");
        assert!(theme.dark);
        assert_eq!(theme.chrome.acc, "#e68e0d");
        let t = &theme.terminal;
        assert_eq!(t.background, "#121212");
        assert_eq!(t.foreground, "#bebebe");
        assert_eq!(t.selection_background, "#2a2a2a");
        assert_eq!(t.black, "#121212");
        assert_eq!(t.red, "#d35f5f");
        assert_eq!(t.yellow, "#b91c1c");
        assert_eq!(t.blue, "#e68e0d");
        assert_eq!(t.white, "#bebebe");
        assert_eq!(t.bright_black, "#333333");
        assert_eq!(t.bright_yellow, "#b90a0a");
        assert_eq!(t.bright_cyan, "#eaeaea");
        assert_eq!(t.bright_white, "#bebebe");
    }
}
