//! The themes the person keeps as files: Sikemux's own theme files, each a theme written as
//! changes to a base, and Ghostty's theme files, each a terminal palette.

use std::path::Path;

use sikemux_view::theme::{Theme, ThemeChanges, ThemeTerminalChanges};

use super::{catalogue, checked, from_colours, ink_reads};
use crate::folder::{without_comments, Folder};

/// Every theme beside the built-in ones, as the core last read them.
#[derive(Clone, Debug, Default)]
pub struct Shelf {
    /// The person's theme files, by id.
    pub own: Folder<Theme>,
    /// Ghostty's theme files, by Ghostty's name for each.
    pub ghostty: Folder<Theme>,
    /// The theme an Omarchy desktop set.
    pub omarchy: Option<Theme>,
}

impl Shelf {
    /// Reads both folders again. A theme file's base is looked up among the themes that are not
    /// the person's own files, so a base is never itself made of changes.
    pub fn reload(&mut self, own: Option<&Path>, ghostty: Option<&Path>) {
        if let Some(ghostty) = ghostty {
            self.ghostty.reload(ghostty, "", |name, text| {
                from_ghostty_file(name, text)
                    .map(|theme| (theme, Vec::new()))
                    .ok_or_else(|| "it is not a Ghostty theme with every colour".to_owned())
            });
        }
        if let Some(own) = own {
            let mut own_folder = std::mem::take(&mut self.own);
            own_folder.reload(own, "json", |id, text| {
                let changes: ThemeChanges = serde_json::from_str(&without_comments(text))
                    .map_err(|error| error.to_string())?;
                made(id, changes, |base| self.not_own(base))
            });
            self.own = own_folder;
        }
    }

    /// The theme with `id` among those that are not the person's own files.
    pub fn not_own(&self, id: &str) -> Option<Theme> {
        let catalogue = catalogue();
        (self.ghostty.kept().map(|(_, theme)| theme))
            .chain(&self.omarchy)
            .chain(&catalogue.sikemux)
            .chain(&catalogue.ghostty)
            .find(|theme| theme.id == id)
            .cloned()
    }
}

/// The theme `changes` make under `id`, laid over its base, or worked out from its terminal
/// colours when it names none; checked, with what was clamped.
pub fn made(
    id: &str,
    mut changes: ThemeChanges,
    base_of: impl Fn(&str) -> Option<Theme>,
) -> Result<(Theme, Vec<String>), String> {
    let name = changes.name.take();
    let mut theme = match changes.base.take() {
        Some(base) => base_of(&base).ok_or_else(|| format!("there is no base theme {base}"))?,
        None => whole(id, &changes).map_or_else(
            || from_terminal(name.as_deref().unwrap_or(id), &changes),
            Ok,
        )?,
    };
    changes.lay_over(&mut theme);
    theme.id = id.to_owned();
    if let Some(name) = name {
        theme.name = name;
    } else if theme.name.is_empty() {
        theme.name = id.to_owned();
    }
    let (theme, clamped) = checked(theme)?;
    ink_reads(&theme)?;
    Ok((theme, clamped))
}

/// The theme `changes` give when they hold every part in full, as the core writes a theme.
fn whole(id: &str, changes: &ThemeChanges) -> Option<Theme> {
    let mut fields = serde_json::to_value(changes).ok()?;
    let fields_map = fields.as_object_mut()?;
    fields_map.insert("id".into(), id.into());
    fields_map.entry("name").or_insert_with(|| id.into());
    serde_json::from_value(fields).ok()
}

/// A theme with no base, worked out from its terminal colours as Ghostty's are. The accent, when
/// `chrome.acc` gives one, picks the app's colours too.
fn from_terminal(name: &str, changes: &ThemeChanges) -> Result<Theme, String> {
    const NEEDED: &str = "a theme with no base needs terminal colours: background, foreground and \
                          the 16 from black to brightWhite, each #rrggbb";
    let ThemeTerminalChanges {
        background,
        foreground,
        cursor,
        cursor_accent,
        selection_background,
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    } = changes.terminal.clone().ok_or(NEEDED)?;
    let (background, foreground) = (background.ok_or(NEEDED)?, foreground.ok_or(NEEDED)?);
    let ansi = [
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    ];
    let mut colours = vec![
        background.clone(),
        foreground.clone(),
        cursor.unwrap_or_else(|| foreground.clone()),
        cursor_accent.unwrap_or(background),
        selection_background.unwrap_or_default(),
    ];
    for colour in ansi {
        colours.push(colour.ok_or(NEEDED)?);
    }
    let accent = (changes.chrome.as_ref()).and_then(|chrome| chrome.acc.as_deref());
    from_colours(name, &colours, accent).ok_or_else(|| NEEDED.to_owned())
}

/// A Ghostty theme file: `background = 15141b`, `palette = 4=#a277ff` and the like.
pub fn from_ghostty_file(name: &str, text: &str) -> Option<Theme> {
    let mut colours = vec![String::new(); 21];
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let (slot, value) = match key {
            "background" => (0, value),
            "foreground" => (1, value),
            "cursor-color" => (2, value),
            "cursor-text" => (3, value),
            "selection-background" => (4, value),
            "palette" => {
                let (index, colour) = value.split_once('=')?;
                let index: usize = index.trim().parse().ok()?;
                if index >= 16 {
                    continue;
                }
                (5 + index, colour.trim())
            }
            _ => continue,
        };
        colours[slot] = format!("#{}", value.trim_start_matches('#').to_ascii_lowercase());
    }
    for (slot, fallback) in [(2, 1), (3, 0)] {
        if colours[slot].is_empty() {
            colours[slot] = colours[fallback].clone();
        }
    }
    from_colours(name, &colours, None)
}
