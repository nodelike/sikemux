//! How the person wants Sikemux to behave, kept here so every app follows
//! the same choices. Each one's default is `Preferences::default()`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use sikemux_process::sync::Locked;

use super::kept::Kept;
use super::{CoreError, CoreResult};
use crate::protocol::{
    CustomCommand, Preferences, MAX_PROJECT_DEPTH, MAX_TERMINAL_FONT_SIZE, MAX_TEXT_SCALE,
    MIN_TERMINAL_FONT_SIZE, MIN_TEXT_SCALE, UI_TEXT_SCALES,
};

const FILE: &str = "preferences.json";

pub(crate) struct Prefs(Arc<Held>);

struct Held {
    values: Mutex<Map<String, Value>>,
    kept: Kept,
    /// A save is on its way and will write what `values` holds by then.
    saving: AtomicBool,
    /// Taken for each save, so a later one never writes before an earlier.
    writing: Mutex<()>,
}

impl Prefs {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let mut values = defaults();
        let saved: Value = kept.load();
        for (key, value) in saved.as_object().into_iter().flatten() {
            let _ = put(&mut values, key, value.clone());
        }
        Self(Arc::new(Held {
            values: Mutex::new(values),
            kept,
            saving: AtomicBool::new(false),
            writing: Mutex::new(()),
        }))
    }

    pub(crate) fn get(&self) -> Preferences {
        read(&self.0.values.locked())
    }

    /// Sets the preference `key`, refusing a key there is none of or a value
    /// it cannot hold.
    pub(crate) fn set(&self, key: &str, value: Value) -> CoreResult<Preferences> {
        let preferences = put(&mut self.0.values.locked(), key, value)?;
        self.save_soon();
        Ok(preferences)
    }

    /// Saves off the caller's thread; a burst of changes, such as a pinch's,
    /// is written once each save has finished.
    fn save_soon(&self) {
        if self.0.saving.swap(true, Ordering::AcqRel) {
            return;
        }
        let held = self.0.clone();
        let save = move || {
            let _writing = held.writing.locked();
            held.saving.store(false, Ordering::Release);
            let values = held.values.locked().clone();
            held.kept.save(&values);
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(save)),
            Err(_) => save(),
        }
    }
}

/// Every preference's name.
pub(crate) fn keys() -> Vec<String> {
    defaults().keys().cloned().collect()
}

fn defaults() -> Map<String, Value> {
    match serde_json::to_value(Preferences::default()) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    }
}

fn read(values: &Map<String, Value>) -> Preferences {
    serde_json::from_value(Value::Object(values.clone())).unwrap_or_default()
}

fn put(values: &mut Map<String, Value>, key: &str, value: Value) -> CoreResult<Preferences> {
    if !values.contains_key(key) {
        return Err(CoreError::from(format!(
            "there is no preference named {key}"
        )));
    }
    let mut next = values.clone();
    next.insert(key.to_owned(), value);
    let preferences: Preferences = serde_json::from_value(Value::Object(next))
        .map_err(|_| CoreError::from(format!("that is not a value {key} can have")))?;
    let preferences = within_limits(preferences);
    if let Ok(Value::Object(fields)) = serde_json::to_value(&preferences) {
        *values = fields;
    }
    Ok(preferences)
}

fn within_limits(mut preferences: Preferences) -> Preferences {
    let scale =
        |value: f64| ((value * 100.0).round() / 100.0).clamp(MIN_TEXT_SCALE, MAX_TEXT_SCALE);
    preferences.chat_text_scale = scale(preferences.chat_text_scale);
    preferences.editor_text_scale = scale(preferences.editor_text_scale);
    preferences.terminal_font_size = preferences
        .terminal_font_size
        .clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE);
    preferences.ui_text_scale = UI_TEXT_SCALES
        .into_iter()
        .min_by(|a, b| {
            (a - preferences.ui_text_scale)
                .abs()
                .total_cmp(&(b - preferences.ui_text_scale).abs())
        })
        .unwrap_or(UI_TEXT_SCALES[0]);
    for root in &mut preferences.project_roots {
        root.depth = root.depth.min(MAX_PROJECT_DEPTH);
    }
    preferences.custom_commands = commands_within_limits(preferences.custom_commands);
    preferences.pane_strength = ((preferences.pane_strength * 20.0).round() / 20.0).clamp(0.0, 2.0);
    let hundredths = |value: f64, low: f64| ((value * 100.0).round() / 100.0).clamp(low, 1.0);
    preferences.pane_dot_size = preferences.pane_dot_size.clamp(1, 6);
    preferences.pane_contrast = hundredths(preferences.pane_contrast, -1.0);
    preferences.pane_brightness = hundredths(preferences.pane_brightness, -1.0);
    preferences.pane_glow = hundredths(preferences.pane_glow, 0.0);
    preferences.pane_glow_size = hundredths(preferences.pane_glow_size, 0.0);
    let inks = Preferences::default().pane_inks;
    preferences.pane_inks.truncate(inks.len());
    preferences
        .pane_inks
        .extend(inks.into_iter().skip(preferences.pane_inks.len()));
    preferences.window_opacity = preferences.window_opacity.clamp(0.0, 1.0);
    preferences.window_blur = preferences.window_blur.round();
    preferences.side_rail_width = preferences
        .side_rail_width
        .clamp(SIDE_RAIL_WIDTH.0, SIDE_RAIL_WIDTH.1)
        .round();
    preferences.tab_rail_width = preferences
        .tab_rail_width
        .clamp(TAB_RAIL_WIDTH.0, TAB_RAIL_WIDTH.1)
        .round();
    preferences
        .recent_command_keys
        .retain(|key| !key.is_empty() && key.len() <= 200);
    preferences
        .recent_command_keys
        .truncate(REMEMBERED_COMMANDS);
    preferences.keybinding_overrides = preferences
        .keybinding_overrides
        .into_iter()
        .filter(|(action, keys)| {
            action.len() <= 200 && keys.as_ref().is_none_or(|keys| keys.len() <= 100)
        })
        .take(MAX_KEYBINDING_OVERRIDES)
        .collect();
    preferences.cloud_browser = preferences.cloud_browser.trim().to_owned();
    preferences.cloud_browser_shortcut = preferences.cloud_browser_shortcut.trim().to_owned();
    preferences.last_agent_type = preferences
        .last_agent_type
        .filter(|agent| crate::agent_catalog::agent(agent).is_some());
    preferences
}

/// The narrowest and widest each rail can be, in points, as the Mac's rails are.
const SIDE_RAIL_WIDTH: (f64, f64) = (180.0, 480.0);
const TAB_RAIL_WIDTH: (f64, f64) = (240.0, 560.0);
/// How many commands the command deck remembers having run.
const REMEMBERED_COMMANDS: usize = 20;

const MAX_CUSTOM_COMMANDS: usize = 100;
const MAX_KEYBINDING_OVERRIDES: usize = 1_000;

/// Keeps commands with an id, a title and a command, once each by id, each
/// field cut to what the palette shows.
fn commands_within_limits(commands: Vec<CustomCommand>) -> Vec<CustomCommand> {
    let cut = |text: &str, chars: usize| text.chars().take(chars).collect::<String>();
    let mut seen = HashSet::new();
    commands
        .into_iter()
        .filter(|command| {
            !command.title.trim().is_empty()
                && !command.command.trim().is_empty()
                && !command.id.is_empty()
                && seen.insert(command.id.clone())
        })
        .take(MAX_CUSTOM_COMMANDS)
        .map(|command| CustomCommand {
            id: cut(&command.id, 100),
            title: cut(command.title.trim(), 120),
            detail: cut(command.detail.trim(), 240),
            command: cut(&command.command, 8_000),
            ..command
        })
        .collect()
}
