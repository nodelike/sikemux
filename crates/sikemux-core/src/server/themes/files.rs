//! The person's own themes, kept as files in `~/.config/sikemux/themes`, each named by its id,
//! and Ghostty's in `~/.config/ghostty/themes`. Both folders are read again whenever they change.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sikemux_corekit::themes::{checked, ink_reads, Shelf};
use sikemux_view::prefs::{PreferenceChange, DEFAULT_THEME_ID};
use sikemux_view::theme::Theme;

use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::connection::blocking;
use crate::server::parts::Parts;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{watched_folder, Core, CoreResult, ServerCore};

/// An editor writes a file in several steps; the folder is read once they are done.
const SETTLE: Duration = Duration::from_millis(150);

/// The folder of the person's theme files; `themes.dev` in a development build.
pub(crate) fn folder() -> Option<PathBuf> {
    sikemux_process::paths::config_file("themes", "")
}

pub(super) fn ghostty_folder() -> Option<PathBuf> {
    sikemux_process::paths::home().map(|home| home.join(".config/ghostty/themes"))
}

/// Every theme beside the built-in ones, read now.
pub(super) fn read(omarchy: Option<Theme>) -> Shelf {
    let mut shelf = Shelf {
        omarchy,
        ..Shelf::default()
    };
    shelf.reload(folder().as_deref(), ghostty_folder().as_deref());
    shelf
}

/// Moves the themes an older core kept in preferences into files, then reads both folders again
/// each time one changes.
pub(super) async fn follow(core: Arc<Core>) {
    let Some(own) = folder() else {
        return;
    };
    let worker = core.clone();
    let _ = blocking(move || {
        let _ = std::fs::create_dir_all(&own);
        worker.preferences().move_old_themes(move_old);
        Ok(())
    })
    .await;
    reread(&core);
    let ghostty = ghostty_folder().filter(|ghostty| ghostty.is_dir());
    let changed = || {
        let core = core.clone();
        async move { reread(&core) }
    };
    let ghostty_changes = async {
        if let Some(ghostty) = ghostty {
            watched_folder::watch(core.watch_hub(), ghostty, SETTLE, changed).await;
        }
    };
    let own_changes = async {
        if let Some(own) = folder() {
            watched_folder::watch(core.watch_hub(), own, SETTLE, changed).await;
        }
    };
    tokio::join!(ghostty_changes, own_changes);
}

/// Reads both folders again, and redraws every app when the theme worn changed.
fn reread(core: &Core) {
    core.mods().personal_changed();
    core.look()
        .shelf()
        .reload(folder().as_deref(), ghostty_folder().as_deref());
    core.theme_changed();
    core.changed(What::Read(Topic::Preferences));
}

/// Writes each old theme to a file of its own, leaving any file already there; false when one
/// could not be written.
fn move_old(themes: &[Value]) -> bool {
    let mut moved = true;
    for theme in themes {
        if let Ok(theme) = serde_json::from_value::<Theme>(theme.clone()) {
            let taken = folder().is_some_and(|folder| file(&folder, &theme.id).exists());
            if !taken && write(&theme).is_err() {
                moved = false;
            }
        }
    }
    moved
}

fn file(folder: &std::path::Path, id: &str) -> PathBuf {
    folder.join(format!("{id}.json"))
}

/// Writes `theme` whole to its file, replacing what was there, and reads the folder again.
pub(crate) fn keep(core: &Core, theme: &Theme) -> Result<(), String> {
    write(theme)?;
    core.look()
        .shelf()
        .reload(folder().as_deref(), ghostty_folder().as_deref());
    core.changed(What::Read(Topic::Preferences));
    Ok(())
}

/// The one way the core writes a theme file: the checked theme, without its id, which is the
/// file's name.
fn write(theme: &Theme) -> Result<(), String> {
    let folder = folder().ok_or("there is no folder for themes on this computer")?;
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let mut fields = serde_json::to_value(theme).map_err(|error| error.to_string())?;
    if let Some(fields) = fields.as_object_mut() {
        fields.remove("id");
    }
    let text = serde_json::to_string_pretty(&fields).map_err(|error| error.to_string())? + "\n";
    let unfinished = folder.join(format!(".{}.json.part", theme.id));
    std::fs::write(&unfinished, text).map_err(|error| error.to_string())?;
    std::fs::rename(&unfinished, file(&folder, &theme.id)).map_err(|error| error.to_string())
}

/// Keeps a theme the person made in the app as a file, and wears it.
pub(crate) fn save(core: &Core, theme: Theme) -> CoreResult<()> {
    let (theme, _) = checked(theme)?;
    ink_reads(&theme)?;
    keep(core, &theme)?;
    crate::server::preferences::set(core, PreferenceChange::ThemeId(theme.id))?;
    Ok(())
}

/// Takes away one of the person's theme files, going back to the default theme when it was worn.
pub(crate) fn delete(core: &Core, id: &str) -> CoreResult<()> {
    let folder = folder().ok_or("there is no folder for themes on this computer")?;
    if core.look().shelf().own.kept().all(|(name, _)| name != id) {
        return Err(format!("there is no theme file {id}").into());
    }
    std::fs::remove_file(file(&folder, id)).map_err(|error| error.to_string())?;
    reread(core);
    if core.preferences().get().theme_id == id {
        crate::server::preferences::set(core, PreferenceChange::ThemeId(DEFAULT_THEME_ID.into()))?;
    }
    Ok(())
}
