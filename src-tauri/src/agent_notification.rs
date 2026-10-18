//! The app's notifications. Clicking one on macOS brings the app forward, and
//! opens the agent it is about.

use tauri::{AppHandle, Emitter};

#[tauri::command]
pub fn post_notification(app: AppHandle, title: String, body: String, agent_id: Option<String>) {
    std::thread::spawn(move || {
        if post_and_wait_for_click(&app, &title, &body) {
            crate::harness::bring_to_front(&app);
            if let Some(agent_id) = agent_id {
                let _ = app.emit_to("main", "focus_agent", agent_id);
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
