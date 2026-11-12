//! Where Claude Code will compact a session on its own, so the context meter fills toward that
//! point rather than the model's whole window.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_wire::protocol::ChatLaunch;

use super::super::super::cap;
use crate::agent_catalog::AgentKind;
use crate::sessions::roots::account_root;

/// Claude Code compacts this many tokens short of the window it compacts against.
const RESERVE: u64 = 33_000;
/// Claude Code ignores an `autoCompactWindow` outside these bounds.
const SMALLEST_WINDOW: u64 = 100_000;
const LARGEST_WINDOW: u64 = 1_000_000;

/// The window Claude Code compacts against, as the launch and the person's settings give it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AutoCompact {
    off: bool,
    window: Option<u64>,
    /// A model's own window, or none where its setting says `auto`.
    by_model: BTreeMap<String, Option<u64>>,
}

impl AutoCompact {
    /// A capped chat is launched with the core's window, which beats every setting.
    pub fn of(launch: &ChatLaunch, capped: bool) -> Self {
        if capped {
            return Self {
                window: Some(cap::CLAUDE_WINDOW),
                ..Self::default()
            };
        }
        let root = account_root(AgentKind::Claude, None, &launch.env);
        Self::from_settings(&launch.cwd, root.as_deref())
    }

    /// Reads local, then project, then the account's settings, as Claude Code ranks them.
    fn from_settings(cwd: &Path, account_root: Option<&Path>) -> Self {
        let files = [
            Some(cwd.join(".claude/settings.local.json")),
            Some(cwd.join(".claude/settings.json")),
            account_root.map(|root| root.join("settings.json")),
        ];
        let mut found = Self::default();
        for settings in files.into_iter().flatten().filter_map(|file| read(&file)) {
            found.off |= settings["autoCompactEnabled"] == Value::Bool(false);
            if found.window.is_none() {
                found.window = window(&settings["autoCompactWindow"]);
            }
            if let Some(models) = settings["modelSettings"].as_object() {
                for (model, setting) in models {
                    let own = &setting["autoCompactWindow"];
                    if own.is_null() || found.by_model.contains_key(model) {
                        continue;
                    }
                    found.by_model.insert(model.clone(), window(own));
                }
            }
        }
        let global = account_root.and_then(|root| read(&root.join(".claude.json")));
        found.off |=
            global.is_some_and(|config| config["autoCompactEnabled"] == Value::Bool(false));
        found
    }

    /// Tokens of context at which Claude compacts `model`'s session, whose window is `size`.
    pub fn at(&self, model: Option<&str>, size: u64) -> Option<u64> {
        if self.off || size == 0 {
            return None;
        }
        let bare = model.map(|model| model.split('[').next().unwrap_or(model));
        let window = match bare.and_then(|model| self.by_model.get(model)) {
            Some(own) => *own,
            None => self.window,
        };
        let window = window.map_or(size, |window| window.min(size));
        Some(window.saturating_sub(RESERVE))
    }
}

pub fn read(file: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

fn window(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .filter(|window| (SMALLEST_WINDOW..=LARGEST_WINDOW).contains(window))
}
