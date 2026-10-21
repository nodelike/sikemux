//! The app's notifications. Clicking one on macOS brings the app forward, and
//! opens the agent it is about.

use std::sync::Mutex;

use sikemux_process::sync::Locked;
use tauri::{AppHandle, Emitter, Manager, State};

/// The agent someone asked to see, kept until the page takes it, since a page
/// still loading misses the event that says one is waiting.
#[derive(Default)]
pub struct WaitingFocus(Mutex<Option<String>>);

/// Brings the window forward and has the page show `agent_id`.
pub(crate) fn focus_agent(app: &AppHandle, agent_id: String) {
    crate::harness::bring_to_front(app);
    *app.state::<WaitingFocus>().0.locked() = Some(agent_id);
    let _ = app.emit_to("main", "focus_agent", ());
}

/// The agent waiting to be shown, once.
#[tauri::command]
pub fn focus_claim(waiting: State<'_, WaitingFocus>) -> Option<String> {
    waiting.0.locked().take()
}

#[tauri::command]
pub fn post_notification(app: AppHandle, title: String, body: String, agent_id: Option<String>) {
    std::thread::spawn(move || {
        if post_and_wait_for_click(&app, &title, &body) {
            match agent_id {
                Some(agent_id) => focus_agent(&app, agent_id),
                None => crate::harness::bring_to_front(&app),
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn post_and_wait_for_click(app: &AppHandle, title: &str, body: &str) -> bool {
    use mac_notification_sys::{Notification, NotificationResponse};
    // A dev build has no installed bundle for macOS to show the notification as.
    let bundle = if tauri::is_dev() {
        "com.apple.Terminal"
    } else {
        app.config().identifier.as_str()
    };
    let _ = mac_notification_sys::set_application(bundle);
    matches!(
        Notification::new()
            .title(title)
            .message(body)
            .wait_for_click(true)
            .send(),
        Ok(NotificationResponse::Click)
    )
}

#[cfg(not(target_os = "macos"))]
fn post_and_wait_for_click(_app: &AppHandle, title: &str, body: &str) -> bool {
    let _ = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show();
    false
}
