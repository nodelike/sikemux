//! Notifications about an agent. Clicking one on macOS opens that agent, not
//! just the app.

use tauri::{AppHandle, Emitter};

#[tauri::command]
pub fn notify_agent(app: AppHandle, agent_id: String, title: String, body: String) {
    std::thread::spawn(move || {
        if post_and_wait_for_click(&app, &title, &body) {
            crate::harness::bring_to_front(&app);
            let _ = app.emit_to("main", "focus_agent", agent_id);
        }
    });
}

#[cfg(target_os = "macos")]
fn post_and_wait_for_click(app: &AppHandle, title: &str, body: &str) -> bool {
    use mac_notification_sys::{Notification, NotificationResponse};
    // Matches the notification plugin: a dev build has no installed bundle for macOS to show.
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
fn post_and_wait_for_click(app: &AppHandle, title: &str, body: &str) -> bool {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
    false
}
