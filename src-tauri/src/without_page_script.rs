//! Tauri runs every plugin's page script in every webview, browser tabs
//! included. The dialog and notification scripts replace `alert`, `confirm`
//! and `Notification` with versions that call the app, which a website must
//! never get, so those plugins are registered without their scripts.

use serde_json::Value;
use tauri::ipc::Invoke;
use tauri::plugin::{Plugin, TauriPlugin};
use tauri::webview::PageLoadPayload;
use tauri::{AppHandle, RunEvent, Runtime, Url, Webview, Window};

pub struct WithoutPageScript<R: Runtime>(TauriPlugin<R>);

pub fn without_page_script<R: Runtime>(plugin: TauriPlugin<R>) -> WithoutPageScript<R> {
    WithoutPageScript(plugin)
}

impl<R: Runtime> Plugin<R> for WithoutPageScript<R> {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn initialize(
        &mut self,
        app: &AppHandle<R>,
        config: Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.0.initialize(app, config)
    }

    fn window_created(&mut self, window: Window<R>) {
        self.0.window_created(window)
    }

    fn webview_created(&mut self, webview: Webview<R>) {
        self.0.webview_created(webview)
    }

    fn on_navigation(&mut self, webview: &Webview<R>, url: &Url) -> bool {
        self.0.on_navigation(webview, url)
    }

    fn on_page_load(&mut self, webview: &Webview<R>, payload: &PageLoadPayload<'_>) {
        self.0.on_page_load(webview, payload)
    }

    fn on_event(&mut self, app: &AppHandle<R>, event: &RunEvent) {
        self.0.on_event(app, event)
    }

    fn extend_api(&mut self, invoke: Invoke<R>) -> bool {
        self.0.extend_api(invoke)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapped_plugin_keeps_its_commands_but_injects_nothing() {
        let dialog = without_page_script::<tauri::Wry>(tauri_plugin_dialog::init());
        assert_eq!(dialog.name(), "dialog");
        assert!(dialog.initialization_script_2().is_none());
        let notification = without_page_script::<tauri::Wry>(tauri_plugin_notification::init());
        assert_eq!(notification.name(), "notification");
        assert!(notification.initialization_script_2().is_none());
    }
}
