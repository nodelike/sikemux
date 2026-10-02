use std::collections::HashMap;
use std::process::Child;
use std::process::ChildStdin;
use std::sync::{mpsc, Arc, Mutex};

use serde_json::Value;
use tauri::AppHandle;

use super::diagnostics::clear_server_diagnostics;
use super::limits::{
    clear_pending_requests, release_counter_slots, OPEN_DOCUMENT_COUNT, PENDING_REQUEST_COUNT,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OpenDoc {
    pub(super) refs: usize,
    pub(super) version: u32,
}

// Stdin and the pending map are independent — splitting them lets the reader
// thread deliver responses while a writer is mid-flight.
//
// `child` is held so `lsp_stop` can SIGKILL the server process; without it
// the language server would outlive every session that ever opened it.
// `shutdown` flips true once a stop has been issued — readers and writers
// check it so they bail without spamming errors as the child dies.
pub(super) struct Server {
    pub(super) app: AppHandle,
    pub(super) project: String,
    pub(super) language: String,
    pub(super) child: Mutex<Option<Child>>,
    pub(super) stdin: Mutex<Option<ChildStdin>>,
    pub(super) next_id: Mutex<i64>,
    pub(super) pending: Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    // Per-(path) hash of last full didChange/didOpen payload — drops no-op
    // resends. Incremental edits clear the hash because the backend no longer
    // has the full text to compare.
    pub(super) last_change: Mutex<HashMap<String, u64>>,
    // Backend-owned open-document refcounts + monotonically increasing LSP
    // versions. The frontend can have several editor panes pointed at the same
    // URI; the server must still see exactly one didOpen and one didClose.
    pub(super) open_docs: Mutex<HashMap<String, OpenDoc>>,
    // Paths with a currently published non-empty diagnostic set. The UI owns
    // the diagnostic values; native retains only bounded keys/versions so a
    // server shutdown can emit deterministic clear events.
    pub(super) diagnostic_paths: Mutex<HashMap<String, Option<i64>>>,
    pub(super) shutdown: std::sync::atomic::AtomicBool,
    /// Logical LRU stamp, bumped on every message we send. Admission may evict
    /// the least-recently-used idle server before spawning a replacement.
    pub(super) last_used: std::sync::atomic::AtomicU64,
    /// Invalidates a pending idle shutdown whenever a document reopens.
    pub(super) idle_generation: std::sync::atomic::AtomicU64,
}

pub(super) type ServerHandle = Arc<Server>;

/// Internal registry identity. Keeping the two user-controlled fields typed
/// avoids delimiter collisions and makes project-scoped teardown exact. This
/// type is deliberately not serialized; public commands and event payloads
/// retain their existing string fields.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct ServerKey {
    pub(super) project: String,
    pub(super) language: String,
}

impl ServerKey {
    pub(super) fn new(project: &str, language: &str) -> Self {
        Self {
            project: project.to_owned(),
            language: language.to_owned(),
        }
    }
}

/// Monotonic logical clock for LRU ordering — cheaper and jump-proof vs
/// wall-clock time; we only need relative ordering, not real timestamps.
pub(super) fn lsp_tick() -> u64 {
    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    CLOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Kill and reap before teardown waits on stdin or document-state locks. An
/// LSP server that stops reading can leave a writer blocked in `write_all`
/// while it owns both document mutexes; terminating the reader side of the
/// pipe is what lets that writer unwind and release them.
fn kill_and_reap_child(child: &Mutex<Option<Child>>) {
    let child = child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// SIGKILL + reap a server. Shared by `lsp_stop` and admission eviction.
pub(super) fn shutdown_server(server: ServerHandle) {
    server
        .shutdown
        .store(true, std::sync::atomic::Ordering::Release);

    // This must remain ahead of stdin/open_docs/last_change acquisition. See
    // `shutdown_kills_before_waiting_for_a_blocked_document_writer`.
    kill_and_reap_child(&server.child);
    server
        .stdin
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();

    clear_pending_requests(&server.pending, &PENDING_REQUEST_COUNT);
    let released_documents = {
        let mut documents = server
            .open_docs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = documents.len();
        documents.clear();
        count
    };
    release_counter_slots(&OPEN_DOCUMENT_COUNT, released_documents);
    server
        .last_change
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    clear_server_diagnostics(&server);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;
    use std::thread;
    use std::time::Duration;

    #[cfg(unix)]
    #[test]
    fn shutdown_kills_before_waiting_for_a_blocked_document_writer() {
        use std::os::fd::AsRawFd;

        struct ChildCleanup(Arc<Mutex<Option<Child>>>);

        impl Drop for ChildCleanup {
            fn drop(&mut self) {
                kill_and_reap_child(&self.0);
            }
        }

        let mut child = sikemux_process::user_environment::command("/bin/sleep")
            .arg("60")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn non-reading child");
        let stdin = child.stdin.take().expect("child stdin");

        // Fill the pipe without blocking, then restore blocking mode. The next
        // byte written is guaranteed to wait until the child closes its reader.
        let fd = stdin.as_raw_fd();
        // SAFETY: `fd` is a live ChildStdin descriptor owned by this test.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0, "read pipe flags");
        assert!(
            // SAFETY: the descriptor remains live and the flag combination keeps
            // its existing access mode while temporarily adding O_NONBLOCK.
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "set nonblocking pipe"
        );
        let chunk = [0_u8; 4_096];
        loop {
            // SAFETY: `chunk` remains valid for the full write call and `fd`
            // still belongs to `stdin` in this scope.
            let written = unsafe { libc::write(fd, chunk.as_ptr().cast(), chunk.len()) };
            if written > 0 {
                continue;
            }
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock,
                "pipe should become full"
            );
            break;
        }
        assert!(
            // SAFETY: `fd` is unchanged and live; restore the exact original flags.
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } >= 0,
            "restore blocking pipe"
        );

        let child = Arc::new(Mutex::new(Some(child)));
        let _cleanup = ChildCleanup(child.clone());
        let stdin = Arc::new(Mutex::new(Some(stdin)));
        let document_state = Arc::new(Mutex::new(()));
        let (writer_entered_tx, writer_entered_rx) = mpsc::channel();
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        let writer_stdin = stdin.clone();
        let writer_state = document_state.clone();
        let writer = thread::spawn(move || {
            let _documents = writer_state.lock().expect("document state");
            let mut stdin = writer_stdin.lock().expect("stdin state");
            writer_entered_tx.send(()).expect("writer entered");
            let result = stdin
                .as_mut()
                .expect("live stdin")
                .write_all(&[1_u8])
                .map_err(|error| error.kind());
            writer_done_tx.send(result).expect("writer result");
        });
        writer_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("writer acquired document and stdin locks");

        // The old shutdown order waited on `document_state` here forever.
        // Killing first closes the pipe reader, so the blocked writer unwinds.
        kill_and_reap_child(&child);
        assert_eq!(
            writer_done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("blocked writer released"),
            Err(std::io::ErrorKind::BrokenPipe)
        );
        writer.join().expect("writer thread");
        assert!(document_state.try_lock().is_ok());
        stdin.lock().expect("stdin state").take();
    }
}
