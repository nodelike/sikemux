//! The colour themes the person picks from, and the one they picked: the
//! Sikemux themes, Ghostty's catalogue with the app's colours worked out from
//! each terminal palette, the person's own theme files and Ghostty's, and on an
//! Omarchy desktop the theme it set for the system.

mod files;
mod omarchy;
pub(crate) mod tools;

pub(crate) use files::{delete, save};

pub(crate) use omarchy::{follow_background as follow_omarchy_background, here as on_omarchy};

use super::{Core, CoreResult};
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use serde::Deserialize;
pub use sikemux_calls::themes::{ThemePaletteParams, ThemeParams};
pub use sikemux_corekit::themes::default_palette;
pub(crate) use sikemux_corekit::themes::{
    by_name, from_colours, palette, palette_of, summaries, terminal_colors,
};
use sikemux_view::prefs::Preferences;
use std::sync::{Mutex, MutexGuard};

use sikemux_process::sync::Locked;

use sikemux_grid::TerminalColors;
use sikemux_view::theme::{Palette, Theme, ThemeSummary};

use sikemux_corekit::themes::Shelf;

/// The person's theme as the core resolved it, kept until their preferences
/// or the system's theme change.
pub(crate) struct Look {
    palette: Mutex<Palette>,
    /// The app's terminal theme, which programs asking for colours are told.
    terminal_colors: Mutex<TerminalColors>,
    /// Every theme beside the built-in ones: the person's files, Ghostty's and Omarchy's.
    shelf: Mutex<Shelf>,
    /// Sikemux's copy of the background Omarchy shows, while panes follow it.
    omarchy_background: Mutex<Option<String>>,
    copying_omarchy_background: Mutex<()>,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            palette: Mutex::new(default_palette()),
            terminal_colors: Mutex::default(),
            shelf: Mutex::new(files::read(omarchy::read())),
            omarchy_background: Mutex::default(),
            copying_omarchy_background: Mutex::default(),
        }
    }
}

impl Look {
    pub(crate) fn shelf(&self) -> MutexGuard<'_, Shelf> {
        self.shelf.locked()
    }

    /// Keeps the theme Omarchy switched to; false when it is the one kept already. The person's
    /// files are read again, since one may be made over it.
    fn set_omarchy(&self, theme: Theme) -> bool {
        let mut shelf = self.shelf.locked();
        if shelf.omarchy.as_ref() == Some(&theme) {
            return false;
        }
        shelf.omarchy = Some(theme);
        shelf.reload(files::folder().as_deref(), None);
        true
    }

    pub(crate) fn omarchy_background(&self) -> Option<String> {
        self.omarchy_background.locked().clone()
    }

    /// Keeps the copy of Omarchy's background; false when it is the one kept already.
    fn set_omarchy_background(&self, background: String) -> bool {
        self.omarchy_background.locked().replace(background.clone()) != Some(background)
    }

    pub(crate) fn set(&self, palette: Palette) {
        *self.terminal_colors.locked() = terminal_colors(&palette.theme);
        *self.palette.locked() = palette;
    }

    pub(crate) fn palette(&self) -> Palette {
        self.palette.locked().clone()
    }

    pub(crate) fn terminal_colors(&self) -> TerminalColors {
        self.terminal_colors.locked().clone()
    }

    pub(crate) fn tokens_by_name(&self) -> std::collections::BTreeMap<String, String> {
        by_name(&self.palette.locked())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThemeFromColoursParams {
    name: String,
    colours: Vec<String>,
    accent: Option<String>,
}

pub(crate) fn palette_for(core: &Core, params: ThemeParams) -> Palette {
    match params.id {
        Some(theme_id) => palette(
            &core.look().shelf(),
            &Preferences {
                theme_id,
                ..core.preferences().get()
            },
        ),
        None => core.look().palette(),
    }
}

pub(crate) fn listed(core: &Core) -> Vec<ThemeSummary> {
    summaries(&core.look().shelf())
}

impl super::part::Part for Look {
    fn start(&self, core: &std::sync::Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![
            tokio::spawn(omarchy::follow(core.clone())),
            tokio::spawn(files::follow(core.clone())),
        ]
    }
}

pub(crate) fn built_from_colours(params: ThemeFromColoursParams) -> CoreResult<Theme> {
    from_colours(&params.name, &params.colours, params.accent.as_deref()).ok_or_else(|| {
        "a theme needs 21 colours, each #rrggbb, the selection's may be empty".into()
    })
}

#[cfg(test)]
mod tests {
    use super::default_palette;

    /// The shared chat page draws with this copy, having no core to ask.
    const COPY: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/chat/defaultPalette.json"
    );

    #[test]
    fn the_default_palette_copy_is_up_to_date() {
        let palette = default_palette();
        if std::env::var_os("SIKEMUX_WRITE_DEFAULT_PALETTE").is_some() {
            let pretty = serde_json::to_string_pretty(&palette).expect("a palette serializes");
            std::fs::write(COPY, pretty + "\n").expect("the copy is writable");
        }
        let copy: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(COPY).expect("the copy exists"))
                .expect("the copy parses");
        assert!(
            copy == serde_json::to_value(&palette).expect("a palette serializes"),
            "shared/chat/defaultPalette.json is out of date: run \
             SIKEMUX_WRITE_DEFAULT_PALETTE=1 cargo test -p sikemux-core --lib default_palette_copy"
        );
    }
}
