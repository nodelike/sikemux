//! How big a chat's, an editor's or a terminal's text is: ⌘=, ⌘−, ⌘0 and pinching on the trackpad
//! over one of them all move it here.

use gpui::{PinchEvent, TouchPhase};
use sikemux_view::prefs::PreferenceChange;
use sikemux_view::prefs::{
    DEFAULT_TERMINAL_FONT_SIZE, DEFAULT_TEXT_SCALE, MAX_TERMINAL_FONT_SIZE, MAX_TEXT_SCALE,
    MIN_TERMINAL_FONT_SIZE, MIN_TEXT_SCALE,
};

use crate::workbench::Workbench;

#[derive(Clone, Copy, PartialEq)]
pub enum Surface {
    Chat,
    Editor,
    Terminal,
}

impl Surface {
    /// The bounds of its size, and the step it moves by.
    fn bounds(self) -> (f64, f64, f64) {
        match self {
            Surface::Chat | Surface::Editor => (MIN_TEXT_SCALE, MAX_TEXT_SCALE, 0.05),
            Surface::Terminal => (
                MIN_TERMINAL_FONT_SIZE.into(),
                MAX_TERMINAL_FONT_SIZE.into(),
                1.,
            ),
        }
    }

    /// How far one press of ⌘= or ⌘− moves it, and what ⌘0 puts it back to.
    fn keys(self) -> (f64, f64) {
        match self {
            Surface::Chat | Surface::Editor => (0.1, DEFAULT_TEXT_SCALE),
            Surface::Terminal => (1., DEFAULT_TERMINAL_FONT_SIZE.into()),
        }
    }

    fn set(self, bench: &Workbench, size: f64) {
        let (min, max, _) = self.bounds();
        let size = size.clamp(min, max);
        let scale = (size * 100.).round() / 100.;
        crate::settings::set(
            bench,
            match self {
                Surface::Terminal => PreferenceChange::TerminalFontSize(size as u32),
                Surface::Chat => PreferenceChange::ChatTextScale(scale),
                Surface::Editor => PreferenceChange::EditorTextScale(scale),
            },
        );
    }

    /// Makes its text a step bigger or smaller, or with none back to its default size.
    pub fn step(self, bench: &Workbench, grow: Option<bool>) {
        let (step, default) = self.keys();
        let size = grow.map_or(default, |grow| {
            self.size(bench) + if grow { step } else { -step }
        });
        self.set(bench, size);
    }

    fn size(self, bench: &Workbench) -> f64 {
        let preferences = bench.preferences();
        match self {
            Surface::Chat => preferences.chat_text_scale,
            Surface::Editor => preferences.editor_text_scale,
            Surface::Terminal => preferences.terminal_font_size.into(),
        }
    }
}

/// One pinch: what it resizes, the size it started from, how far it has scaled that, and the
/// size last sent to the core.
pub struct Pinch {
    surface: Surface,
    base: f64,
    factor: f64,
    sent: f64,
}

pub fn resize(bench: &mut Workbench, surface: Surface, event: &PinchEvent) {
    let mut pinch = match bench.pinch.take() {
        Some(pinch) if pinch.surface == surface && event.phase != TouchPhase::Started => pinch,
        _ => {
            let base = surface.size(bench);
            Pinch {
                surface,
                base,
                factor: 1.,
                sent: base,
            }
        }
    };
    pinch.factor *= 1. + f64::from(event.delta);
    let (min, max, step) = surface.bounds();
    let size = ((pinch.base * pinch.factor / step).round() * step).clamp(min, max);
    if size != pinch.sent {
        pinch.sent = size;
        surface.set(bench, size);
    }
    if !matches!(event.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
        bench.pinch = Some(pinch);
    }
}
