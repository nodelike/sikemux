//! Calls back after each burst of changes to a folder, such as the person's theme files.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

/// Calls `changed` once the changes to `folder` settle for `settle`; returns when the folder
/// cannot be watched.
pub(crate) async fn watch<Done: Future<Output = ()>>(
    folder: PathBuf,
    settle: Duration,
    mut changed: impl FnMut() -> Done,
) {
    let (sender, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let Ok(mut watcher) =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok() {
                let _ = sender.send(());
            }
        })
    else {
        return;
    };
    if watcher.watch(&folder, RecursiveMode::NonRecursive).is_err() {
        return;
    }
    while changes.recv().await.is_some() {
        tokio::time::sleep(settle).await;
        while changes.try_recv().is_ok() {}
        changed().await;
    }
}
