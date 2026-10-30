//! Pinching on the trackpad over a chat, an editor or a terminal resizes its text, as ⌘= and ⌘−
//! do for whichever of them has the focus.

use gpui::{PinchEvent, TouchPhase};
use sikemux_wire::protocol::{
    MAX_TERMINAL_FONT_SIZE, MAX_TEXT_SCALE, MIN_TERMINAL_FONT_SIZE, MIN_TEXT_SCALE,
};

use crate::workbench::Workbench;

#[derive(Clone, Copy, PartialEq)]
pub enum Surface {
    Chat,
    Editor,
    Terminal,
}

impl Surface {
    /// The preference it reads its size from, the bounds of that size, and the step it moves by.
    fn preference(self) -> (&'static str, f64, f64, f64) {
        match self {
            Surface::Chat => ("chatTextScale", MIN_TEXT_SCALE, MAX_TEXT_SCALE, 0.05),
            Surface::Editor => ("editorTextScale", MIN_TEXT_SCALE, MAX_TEXT_SCALE, 0.05),
            Surface::Terminal => (
                "terminalFontSize",
                MIN_TERMINAL_FONT_SIZE.into(),
                MAX_TERMINAL_FONT_SIZE.into(),
                1.,
            ),
        }
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
    let (key, min, max, step) = surface.preference();
    let size = ((pinch.base * pinch.factor / step).round() * step).clamp(min, max);
    if size != pinch.sent {
        pinch.sent = size;
        let value = match surface {
            Surface::Terminal => serde_json::Value::from(size as u32),
            _ => serde_json::Value::from((size * 100.).round() / 100.),
        };
        crate::settings::set(bench, key, value);
    }
    if !matches!(event.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
        bench.pinch = Some(pinch);
    }
}
