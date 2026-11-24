//! Calls back after each burst of changes to a folder, such as the person's theme files.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};

/// Calls `changed` once the changes to `folder` settle for `settle`; returns when the folder
/// cannot be watched. Linux also reports reads, which must not count: `changed` may read the
/// folder itself, and would then be called without end.
pub(crate) async fn watch<Done: Future<Output = ()>>(
    folder: PathBuf,
    settle: Duration,
    mut changed: impl FnMut() -> Done,
) {
    let (sender, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let Ok(mut watcher) =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|event| !matches!(event.kind, EventKind::Access(_))) {
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn reading_the_folder_is_no_change_to_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().to_owned();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let read = move || std::fs::read_dir(&folder).expect("list").count();
        let reread = read.clone();
        tokio::spawn(super::watch(
            dir.path().to_owned(),
            Duration::from_millis(10),
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                reread();
                async {}
            },
        ));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline, "no change was heard");
            std::fs::write(dir.path().join("theme.json"), b"{}").expect("write");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let settled = calls.load(Ordering::SeqCst);
        read();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(calls.load(Ordering::SeqCst), settled);
    }
}
