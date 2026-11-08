//! The theme Omarchy, an Arch Linux desktop, sets for the whole system. It
//! writes the current theme's colours to a folder it replaces on every switch,
//! so Sikemux offers one theme, Omarchy, that follows it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde::Deserialize;
use sikemux_view::theme::Theme;

use crate::server::changed::{Topic, What};
use crate::server::connection::blocking;
use crate::server::Core;

pub(crate) const THEME_ID: &str = "omarchy";
/// A switch replaces the folder and then stamps it, several writes in a row.
const SETTLE: Duration = Duration::from_millis(300);

/// Where Omarchy keeps the theme it set; only Linux runs Omarchy.
fn current() -> Option<PathBuf> {
    cfg!(target_os = "linux")
        .then(sikemux_process::paths::home)
        .flatten()
        .map(|home| home.join(".local/state/omarchy/current"))
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
    switches(current, |theme| {
        if core.look.set_omarchy(theme) {
            if core.preferences.get().theme_id == THEME_ID {
                core.theme_changed();
            }
            core.changed(What::Read(Topic::Preferences));
        }
    })
    .await;
}

/// Hands `switched` the theme in `current` after each change to the folder.
/// Omarchy replaces its `theme` folder on a switch, so this watches the folder around it.
async fn switches(current: PathBuf, mut switched: impl FnMut(Theme)) {
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
        let folder = current.clone();
        // Mid-switch the folder can be gone; the last theme stays until the new one is in place.
        if let Ok(Some(theme)) = blocking(move || Ok(read_from(&folder))).await {
            switched(theme);
        }
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
