use serde::{Deserialize, Serialize};

/// Which theme to answer for; the person's pick when none is named.
#[derive(Serialize, Deserialize, Default)]
pub struct ThemeParams {
    pub id: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ThemePaletteParams {
    pub theme: sikemux_view::theme::Theme,
}
