//! What a theme draws behind each pane. Every theme draws the grain unless it names another
//! kind; the kinds are built in and a theme only gives their numbers.

use serde::{Deserialize, Serialize};

use crate::prefs::PaneCoverage;
use crate::theme::theme_parts;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackdropKind {
    #[default]
    Grain,
    Gradient,
    Stars,
    Grid,
}

impl BackdropKind {
    /// How solid the kind's brightest pixel is at strength 1.
    pub fn solid(self) -> f64 {
        match self {
            Self::Grain => 0.85,
            Self::Gradient => 0.35,
            Self::Stars => 0.9,
            Self::Grid => 0.6,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GridShape {
    #[default]
    Lines,
    Dots,
}

theme_parts! {
/// A theme's backdrop. Each number applies to the kinds its comment names and is clamped to
/// its range when the theme is checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ThemeBackdrop / ThemeBackdropChanges {
    pub kind: BackdropKind,
    /// From 0 to 2; lowered when the theme is checked until text over it reads.
    pub strength: f64,
    /// From 0 to 2.
    pub speed: f64,
    /// The size of its features, from 0.5 to 3.
    pub scale: f64,
    /// Up to three, each a colour or a palette token's name such as `accent`; none takes the
    /// kind's own.
    pub inks: Vec<String>,
    pub coverage: PaneCoverage,
    /// From 0 to 65535, so two themes of one kind can differ.
    pub seed: u32,
    /// Gradient: how far each ink spreads, from 0 to 1.
    pub softness: f64,
    /// Gradient: how much of it is drawn in dots, from 0 to 1.
    pub dither: f64,
    /// Stars: how many, from 0 to 1.
    pub density: f64,
    /// Stars: how much each one breathes, from 0 to 1.
    pub twinkle: f64,
    /// Stars: how many depths, from 1 to 3.
    pub depth: u8,
    /// Stars and grid: how fast they cross the pane, from 0 to 1.
    pub drift: f64,
    /// Grid.
    pub shape: GridShape,
    /// Grid: the distance between lines, from 8 to 96 points.
    pub cell: f64,
    /// Grid: how thick a line is, or how wide a dot, from 0.5 to 3 points.
    pub weight: f64,
}
}

impl Eq for ThemeBackdrop {}

impl Default for ThemeBackdrop {
    fn default() -> Self {
        Self {
            kind: BackdropKind::Grain,
            strength: 1.,
            speed: 1.,
            scale: 1.,
            inks: Vec::new(),
            coverage: PaneCoverage::Band,
            seed: 0,
            softness: 0.6,
            dither: 0.,
            density: 0.5,
            twinkle: 0.5,
            depth: 2,
            drift: 0.2,
            shape: GridShape::Lines,
            cell: 24.,
            weight: 1.,
        }
    }
}

impl ThemeBackdrop {
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// A theme's backdrop as the core resolved it for drawing: its numbers in range, its inks
/// as `#rrggbb`, and the most strength the person's own setting may take it to before text
/// over it stops reading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WornBackdrop {
    pub look: ThemeBackdrop,
    pub inks: Vec<String>,
    pub most: f64,
}

impl Eq for WornBackdrop {}

impl Default for WornBackdrop {
    fn default() -> Self {
        Self {
            look: ThemeBackdrop::default(),
            inks: Vec::new(),
            most: 2.,
        }
    }
}
