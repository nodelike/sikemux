use std::sync::{Arc, Mutex, MutexGuard};

use super::activity::{UiActivityLog, UiActivitySnapshot};
use super::heartbeat::Heartbeat;
use super::watchdog::{start_hang_watchdog, HangWatchdogConfig, HangWatchdogHandle};

const UI_WATCHDOG_SAMPLE_INTERVAL_MS: u64 = 250;

const UI_WATCHDOG_HANG_THRESHOLD_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct UiHeartbeatProgress {
    active: bool,
    last_sequence: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiHeartbeatTransition {
    Suspended,
    Activated,
    Advanced,
    Unchanged,
}

fn classify_ui_heartbeat_update(
    progress: &mut UiHeartbeatProgress,
    visible: bool,
    sequence: u32,
) -> UiHeartbeatTransition {
    if !visible {
        progress.active = false;
        progress.last_sequence = sequence;
        return UiHeartbeatTransition::Suspended;
    }

    if !progress.active {
        progress.active = true;
        progress.last_sequence = sequence;
        return UiHeartbeatTransition::Activated;
    }

    if sequence > progress.last_sequence {
        progress.last_sequence = sequence;
        UiHeartbeatTransition::Advanced
    } else {
        UiHeartbeatTransition::Unchanged
    }
}

pub(super) fn inactive_ui_heartbeat() -> Heartbeat {
    let heartbeat = Heartbeat::new();
    heartbeat.set_armed(false);
    heartbeat.set_visible(false);
    heartbeat
}

fn apply_ui_heartbeat_update(
    heartbeat: &Heartbeat,
    progress: &mut UiHeartbeatProgress,
    visible: bool,
    sequence: u32,
) -> UiHeartbeatTransition {
    let transition = classify_ui_heartbeat_update(progress, visible, sequence);
    match transition {
        UiHeartbeatTransition::Suspended => {
            heartbeat.set_armed(false);
            heartbeat.set_visible(false);
        }
        UiHeartbeatTransition::Activated => {
            // Returning from startup, a hidden window, or a page reload starts
            // with a fresh deadline and accepts the new page's sequence base.
            heartbeat.set_visible(true);
            heartbeat.beat();
            heartbeat.set_armed(true);
        }
        UiHeartbeatTransition::Advanced => {
            heartbeat.beat();
        }
        UiHeartbeatTransition::Unchanged => {}
    }
    transition
}

/// Tauri-managed owner of the process UI heartbeat and watchdog thread.
///
/// The watchdog starts inactive. A visible heartbeat update establishes its
/// first deadline; hiding or reloading the page suspends it. Keeping ownership
/// here guarantees that application-state teardown stops and joins the thread.
pub struct UiWatchdogState {
    heartbeat: Heartbeat,
    progress: Mutex<UiHeartbeatProgress>,
    activity: Arc<UiActivityLog>,
    watchdog: Option<HangWatchdogHandle>,
}

impl UiWatchdogState {
    pub(crate) fn start() -> std::io::Result<Self> {
        let heartbeat = inactive_ui_heartbeat();
        let activity = Arc::new(UiActivityLog::default());
        let watchdog = start_hang_watchdog(
            heartbeat.clone(),
            HangWatchdogConfig {
                name: "ui".to_owned(),
                sample_interval_ms: UI_WATCHDOG_SAMPLE_INTERVAL_MS,
                hang_threshold_ms: UI_WATCHDOG_HANG_THRESHOLD_MS,
                monitor_hidden: false,
            },
            crate::autopsy::listener(activity.clone()),
        )?;
        Ok(Self {
            heartbeat,
            progress: Mutex::new(UiHeartbeatProgress::default()),
            activity,
            watchdog: Some(watchdog),
        })
    }

    pub(crate) fn suspend(&self) {
        let mut progress = self.lock_progress();
        let sequence = progress.last_sequence;
        apply_ui_heartbeat_update(&self.heartbeat, &mut progress, false, sequence);
    }

    fn update(&self, visible: bool, sequence: u32) {
        let mut progress = self.lock_progress();
        apply_ui_heartbeat_update(&self.heartbeat, &mut progress, visible, sequence);
    }

    fn lock_progress(&self) -> MutexGuard<'_, UiHeartbeatProgress> {
        match self.progress.lock() {
            Ok(progress) => progress,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Drop for UiWatchdogState {
    fn drop(&mut self) {
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.stop();
        }
    }
}

/// Updates the managed UI watchdog using bounded scalar-only input.
///
/// `heartbeat` is a page-local monotonically increasing sequence. Duplicate or
/// regressed values do not refresh the deadline. A hidden update disarms the
/// watchdog, and the next visible update establishes a new sequence baseline.
#[tauri::command]
pub fn observability_ui_heartbeat<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, UiWatchdogState>,
    visible: bool,
    heartbeat: u32,
) {
    // The heartbeat is the first thing a new page sends, so this is the
    // earliest a hang report can learn which process to sample.
    crate::autopsy::ensure_web_content_pid(&app);
    state.update(visible, heartbeat);
}

/// Records what the UI is doing so a hang report can say what it was stuck on.
///
/// This also resolves the renderer's process id, which has to be asked for
/// from the main thread and therefore cannot wait until a hang is underway.
#[tauri::command]
pub fn observability_ui_activity<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, UiWatchdogState>,
    activity: UiActivitySnapshot,
) {
    crate::autopsy::ensure_web_content_pid(&app);
    state.activity.record(activity);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_heartbeat_transition_requires_visible_forward_progress() {
        let mut progress = UiHeartbeatProgress::default();
        assert_eq!(
            classify_ui_heartbeat_update(&mut progress, true, 42),
            UiHeartbeatTransition::Activated
        );
        assert_eq!(
            classify_ui_heartbeat_update(&mut progress, true, 42),
            UiHeartbeatTransition::Unchanged
        );
        assert_eq!(
            classify_ui_heartbeat_update(&mut progress, true, 41),
            UiHeartbeatTransition::Unchanged
        );
        assert_eq!(
            classify_ui_heartbeat_update(&mut progress, true, 43),
            UiHeartbeatTransition::Advanced
        );
        assert_eq!(
            classify_ui_heartbeat_update(&mut progress, false, 43),
            UiHeartbeatTransition::Suspended
        );
        assert_eq!(
            classify_ui_heartbeat_update(&mut progress, true, 0),
            UiHeartbeatTransition::Activated
        );
    }

    #[test]
    fn managed_ui_heartbeat_starts_inactive_and_rebases_after_suspend() {
        let heartbeat = inactive_ui_heartbeat();
        let mut progress = UiHeartbeatProgress::default();
        let initial = heartbeat.snapshot();
        assert!(!initial.armed);
        assert!(!initial.visible);

        assert_eq!(
            apply_ui_heartbeat_update(&heartbeat, &mut progress, true, 7),
            UiHeartbeatTransition::Activated
        );
        let activated = heartbeat.snapshot();
        assert_eq!(activated.sequence, 1);
        assert!(activated.armed);
        assert!(activated.visible);

        assert_eq!(
            apply_ui_heartbeat_update(&heartbeat, &mut progress, true, 7),
            UiHeartbeatTransition::Unchanged
        );
        assert_eq!(heartbeat.snapshot().sequence, 1);
        assert_eq!(
            apply_ui_heartbeat_update(&heartbeat, &mut progress, true, 8),
            UiHeartbeatTransition::Advanced
        );
        assert_eq!(heartbeat.snapshot().sequence, 2);

        apply_ui_heartbeat_update(&heartbeat, &mut progress, false, 8);
        let suspended = heartbeat.snapshot();
        assert!(!suspended.armed);
        assert!(!suspended.visible);
        assert_eq!(
            apply_ui_heartbeat_update(&heartbeat, &mut progress, true, 0),
            UiHeartbeatTransition::Activated
        );
        let rebased = heartbeat.snapshot();
        assert_eq!(rebased.sequence, 3);
        assert!(rebased.armed);
        assert!(rebased.visible);
    }

    #[test]
    fn managed_ui_watchdog_drop_stops_its_thread_without_sleeping() {
        let state = UiWatchdogState::start().unwrap();
        drop(state);
    }
}
