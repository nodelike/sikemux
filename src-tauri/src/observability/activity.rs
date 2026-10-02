use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

const MAX_ACTIVITY_ENTRIES: usize = 32;
const MAX_ACTIVITY_STRING_CHARS: usize = 200;
pub const ACTIVITY_HISTORY_CAPACITY: usize = 8;

/// A command the UI has invoked and not yet seen answered.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiInflightCommand {
    pub command: String,
    pub age_ms: u64,
}

/// A command the UI has already seen answered.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiRecentCommand {
    pub command: String,
    pub ms: u64,
    pub ok: bool,
}

/// Something the user did, such as a key press or a pointer drag.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiInteraction {
    pub kind: String,
    pub age_ms: u64,
}

/// A repeated unhandled promise rejection, collapsed by message.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiActivityRejection {
    pub message: String,
    pub count: u64,
}

/// What the UI believes it is doing.
///
/// Every field arrives from the webview, so the whole snapshot is bounded
/// before it is stored and can never grow this process's memory.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiActivitySnapshot {
    pub at_ms: u64,
    pub inflight: Vec<UiInflightCommand>,
    pub recent: Vec<UiRecentCommand>,
    pub focus_pane: Option<String>,
    pub interactions: Vec<UiInteraction>,
    pub rejections: Vec<UiActivityRejection>,
}

impl UiActivitySnapshot {
    fn bounded(mut self) -> Self {
        self.inflight.truncate(MAX_ACTIVITY_ENTRIES);
        self.recent.truncate(MAX_ACTIVITY_ENTRIES);
        self.interactions.truncate(MAX_ACTIVITY_ENTRIES);
        self.rejections.truncate(MAX_ACTIVITY_ENTRIES);
        for entry in &mut self.inflight {
            truncate_chars(&mut entry.command);
        }
        for entry in &mut self.recent {
            truncate_chars(&mut entry.command);
        }
        for entry in &mut self.interactions {
            truncate_chars(&mut entry.kind);
        }
        for entry in &mut self.rejections {
            truncate_chars(&mut entry.message);
        }
        if let Some(pane) = &mut self.focus_pane {
            truncate_chars(pane);
        }
        self
    }
}

fn truncate_chars(value: &mut String) {
    if let Some((boundary, _)) = value.char_indices().nth(MAX_ACTIVITY_STRING_CHARS) {
        value.truncate(boundary);
    }
}

#[derive(Debug, Default)]
struct UiActivityState {
    latest: Option<UiActivitySnapshot>,
    history: VecDeque<UiActivitySnapshot>,
}

/// The latest UI activity snapshot and a short history behind it.
#[derive(Debug, Default)]
pub struct UiActivityLog {
    state: Mutex<UiActivityState>,
}

impl UiActivityLog {
    pub fn record(&self, snapshot: UiActivitySnapshot) {
        let snapshot = snapshot.bounded();
        let mut state = self.lock_state();
        if let Some(previous) = state.latest.replace(snapshot) {
            if state.history.len() == ACTIVITY_HISTORY_CAPACITY {
                state.history.pop_front();
            }
            state.history.push_back(previous);
        }
    }

    /// The latest snapshot, and the ones before it oldest first.
    pub fn snapshot(&self) -> (Option<UiActivitySnapshot>, Vec<UiActivitySnapshot>) {
        let state = self.lock_state();
        (
            state.latest.clone(),
            state.history.iter().cloned().collect(),
        )
    }

    fn lock_state(&self) -> MutexGuard<'_, UiActivityState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostile_activity_snapshot_cannot_grow_the_process() {
        let snapshot = UiActivitySnapshot {
            at_ms: 42,
            inflight: (0..128)
                .map(|index| UiInflightCommand {
                    command: format!("{index}{}", "c".repeat(4_096)),
                    age_ms: 1,
                })
                .collect(),
            recent: (0..128)
                .map(|_| UiRecentCommand {
                    command: "r".repeat(4_096),
                    ms: 1,
                    ok: true,
                })
                .collect(),
            focus_pane: Some("p".repeat(4_096)),
            interactions: (0..128)
                .map(|_| UiInteraction {
                    kind: "k".repeat(4_096),
                    age_ms: 1,
                })
                .collect(),
            rejections: (0..128)
                .map(|_| UiActivityRejection {
                    message: "m".repeat(4_096),
                    count: 1,
                })
                .collect(),
        }
        .bounded();

        assert_eq!(snapshot.at_ms, 42);
        assert_eq!(snapshot.inflight.len(), MAX_ACTIVITY_ENTRIES);
        assert_eq!(snapshot.recent.len(), MAX_ACTIVITY_ENTRIES);
        assert_eq!(snapshot.interactions.len(), MAX_ACTIVITY_ENTRIES);
        assert_eq!(snapshot.rejections.len(), MAX_ACTIVITY_ENTRIES);
        assert_eq!(snapshot.focus_pane.unwrap().chars().count(), 200);
        assert!(snapshot
            .inflight
            .iter()
            .all(|entry| entry.command.chars().count() == 200));
        assert!(snapshot
            .rejections
            .iter()
            .all(|entry| entry.message.chars().count() == 200));
    }

    #[test]
    fn multi_byte_activity_text_is_cut_on_a_character_boundary() {
        let snapshot = UiActivitySnapshot {
            focus_pane: Some("é".repeat(400)),
            ..UiActivitySnapshot::default()
        }
        .bounded();

        let pane = snapshot.focus_pane.unwrap();
        assert_eq!(pane.chars().count(), 200);
        assert_eq!(pane.len(), 400);
    }

    #[test]
    fn the_activity_log_keeps_the_latest_and_a_bounded_history() {
        let log = UiActivityLog::default();
        log.record(UiActivitySnapshot::default());
        assert!(log.snapshot().1.is_empty());
        for index in 1..(ACTIVITY_HISTORY_CAPACITY as u64 + 4) {
            log.record(UiActivitySnapshot {
                at_ms: index,
                ..UiActivitySnapshot::default()
            });
        }

        let (latest, history) = log.snapshot();
        assert_eq!(latest.unwrap().at_ms, ACTIVITY_HISTORY_CAPACITY as u64 + 3);
        assert_eq!(history.len(), ACTIVITY_HISTORY_CAPACITY);
        assert_eq!(history[0].at_ms, 3);
        assert_eq!(
            history[ACTIVITY_HISTORY_CAPACITY - 1].at_ms,
            ACTIVITY_HISTORY_CAPACITY as u64 + 2
        );
    }
}
