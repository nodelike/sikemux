//! `sikemux://` links opened from anywhere on the Mac.
//!
//! macOS hands them to the running app, launching it first if it has to. The
//! window may not have loaded yet when one arrives, so links wait here and the
//! window collects them once it is ready, and again each time it is told more
//! have come in.

use std::sync::{Mutex, MutexGuard};

use tauri::{AppHandle, Emitter, Manager, Runtime, Url};

const DEEP_LINK_EVENT: &str = "deep-link-available";
const SCHEME: &str = "sikemux";

#[derive(Default)]
pub struct DeepLinks(Mutex<Vec<String>>);

impl DeepLinks {
    fn queue(&self) -> MutexGuard<'_, Vec<String>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn push(&self, links: impl IntoIterator<Item = String>) {
        self.queue().extend(links);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.queue())
    }
}

fn ours(urls: &[Url]) -> Vec<String> {
    urls.iter()
        .filter(|url| url.scheme() == SCHEME)
        .map(Url::to_string)
        .collect()
}

pub fn receive<R: Runtime>(app: &AppHandle<R>, urls: &[Url]) {
    let links = ours(urls);
    if links.is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<DeepLinks>() {
        state.push(links);
    }
    if let Some(window) = app.get_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let _ = app.emit_to("main", DEEP_LINK_EVENT, ());
}

#[tauri::command]
pub fn take_deep_links(state: tauri::State<'_, DeepLinks>) -> Vec<String> {
    state.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_sikemux_links() {
        let urls = [
            Url::parse("sikemux://agent/claude/abc?project=%2Ftmp").unwrap(),
            Url::parse("https://example.com/").unwrap(),
            Url::parse("file:///tmp/notes.md").unwrap(),
        ];
        assert_eq!(
            ours(&urls),
            vec!["sikemux://agent/claude/abc?project=%2Ftmp".to_owned()]
        );
    }

    #[test]
    fn taking_links_empties_the_queue() {
        let links = DeepLinks::default();
        links.push(["sikemux://agent/codex/1?project=%2Fa".to_owned()]);
        assert_eq!(links.take().len(), 1);
        assert!(links.take().is_empty());
    }
}
