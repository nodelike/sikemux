//! The system's open, save and alert panels.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use rfd::{
    AsyncFileDialog, AsyncMessageDialog, FileHandle, MessageButtons, MessageDialogResult,
    MessageLevel,
};
use serde::Deserialize;
use tauri::{AppHandle, Window};

#[derive(Deserialize)]
pub struct Filter {
    name: String,
    extensions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickOptions {
    #[serde(default)]
    directory: bool,
    #[serde(default)]
    multiple: bool,
    title: Option<String>,
    default_path: Option<PathBuf>,
    #[serde(default)]
    filters: Vec<Filter>,
}

type Answer<T> = Pin<Box<dyn Future<Output = Option<T>> + Send>>;

/// AppKit panels have to be made on the main thread; the answer is awaited off it.
async fn on_main<T: Send + 'static>(
    app: &AppHandle,
    show: impl FnOnce() -> Answer<T> + Send + 'static,
) -> Option<T> {
    let (sender, answer) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let pending = show();
        tauri::async_runtime::spawn(async move {
            let _ = sender.send(pending.await);
        });
    })
    .ok()?;
    answer.await.ok().flatten()
}

fn file_dialog(window: &Window, title: Option<String>, filters: Vec<Filter>) -> AsyncFileDialog {
    let mut dialog = AsyncFileDialog::new()
        .set_parent(window)
        .set_can_create_directories(true);
    if let Some(title) = title {
        dialog = dialog.set_title(title);
    }
    for filter in filters {
        dialog = dialog.add_filter(filter.name, &filter.extensions);
    }
    dialog
}

fn paths(handles: Vec<FileHandle>) -> Vec<String> {
    handles
        .iter()
        .map(|handle| handle.path().to_string_lossy().into_owned())
        .collect()
}

/// The files or folders the person picks; `None` when they cancel.
#[tauri::command]
pub async fn pick_paths(
    app: AppHandle,
    window: Window,
    options: PickOptions,
) -> Option<Vec<String>> {
    let picked = on_main(&app, move || {
        let mut dialog = file_dialog(&window, options.title, options.filters);
        if let Some(path) = options.default_path {
            dialog = dialog.set_directory(path);
        }
        let answer: Answer<Vec<FileHandle>> = match (options.directory, options.multiple) {
            (true, true) => Box::pin(dialog.pick_folders()),
            (false, true) => Box::pin(dialog.pick_files()),
            (true, false) => {
                let one = dialog.pick_folder();
                Box::pin(async move { one.await.map(|handle| vec![handle]) })
            }
            (false, false) => {
                let one = dialog.pick_file();
                Box::pin(async move { one.await.map(|handle| vec![handle]) })
            }
        };
        answer
    })
    .await?;
    Some(paths(picked))
}

/// Where the person chooses to save a file named `file_name`; `None` when they cancel.
#[tauri::command]
pub async fn pick_save_path(
    app: AppHandle,
    window: Window,
    title: Option<String>,
    file_name: String,
    filters: Vec<Filter>,
) -> Option<String> {
    let picked = on_main(&app, move || {
        let pending = file_dialog(&window, title, filters)
            .set_file_name(file_name)
            .save_file();
        Box::pin(pending) as Answer<FileHandle>
    })
    .await?;
    Some(picked.path().to_string_lossy().into_owned())
}

/// A warning with `ok` and `cancel` buttons; `on_ok` runs when the person picks `ok`.
pub fn warn(
    app: &AppHandle,
    title: &str,
    message: String,
    ok: &str,
    cancel: &str,
    on_ok: impl FnOnce() + Send + 'static,
) {
    let app = app.clone();
    let (title, ok, cancel) = (title.to_owned(), ok.to_owned(), cancel.to_owned());
    tauri::async_runtime::spawn(async move {
        let label = ok.clone();
        let answer = on_main(&app, move || {
            let shown = AsyncMessageDialog::new()
                .set_title(title)
                .set_description(message)
                .set_level(MessageLevel::Warning)
                .set_buttons(MessageButtons::OkCancelCustom(ok, cancel))
                .show();
            Box::pin(async move { Some(shown.await) }) as Answer<MessageDialogResult>
        })
        .await;
        // Linux answers Ok for the first custom button; macOS names it.
        let chose_ok = match answer {
            Some(MessageDialogResult::Ok) => true,
            Some(MessageDialogResult::Custom(chosen)) => chosen == label,
            _ => false,
        };
        if chose_ok {
            on_ok();
        }
    });
}
