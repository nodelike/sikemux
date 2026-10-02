use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};

use serde_json::Value;

use super::lsp;
use crate::error::{AppError, AppResult};

pub(super) const MAX_LSP_PATH_BYTES: usize = 4_096;
pub(super) const MAX_LSP_LANGUAGE_BYTES: usize = 128;

pub(super) const MAX_LSP_SERVERS: usize = 6;
const MAX_OPEN_DOCUMENTS_PER_SERVER: usize = 512;
const MAX_OPEN_DOCUMENTS_GLOBAL: usize = 2_048;
const MAX_PENDING_REQUESTS_PER_SERVER: usize = 64;
const MAX_PENDING_REQUESTS_GLOBAL: usize = 256;

pub(super) static OPEN_DOCUMENT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(super) static PENDING_REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn validate_server_identity(project: &str, language: &str) -> AppResult<()> {
    if project.len() > MAX_LSP_PATH_BYTES {
        return Err(AppError::Lsp(format!(
            "project path exceeds {MAX_LSP_PATH_BYTES} bytes"
        )));
    }
    if language.len() > MAX_LSP_LANGUAGE_BYTES {
        return Err(AppError::Lsp(format!(
            "language identifier exceeds {MAX_LSP_LANGUAGE_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(super) fn validate_document_path(path: &str) -> AppResult<()> {
    if path.len() > MAX_LSP_PATH_BYTES {
        return Err(AppError::Lsp(format!(
            "document path exceeds {MAX_LSP_PATH_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(super) fn validate_document_language_id(language_id: &str) -> AppResult<()> {
    if language_id.len() > MAX_LSP_LANGUAGE_BYTES {
        return Err(AppError::Lsp(format!(
            "document language identifier exceeds {MAX_LSP_LANGUAGE_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(super) fn server_limit_error() -> AppError {
    AppError::Lsp(format!(
        "language server limit reached ({MAX_LSP_SERVERS}); close a project before starting another"
    ))
}

fn reserve_counter_slot(
    counter: &AtomicUsize,
    limit: usize,
    error: impl FnOnce() -> AppError,
) -> AppResult<()> {
    counter
        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < limit).then_some(current + 1)
        })
        .map(|_| ())
        .map_err(|_| error())
}

pub(super) fn release_counter_slots(counter: &AtomicUsize, count: usize) {
    if count == 0 {
        return;
    }
    let released = counter.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        current.checked_sub(count)
    });
    debug_assert!(released.is_ok(), "LSP resource counter underflow");
}

pub(super) fn reserve_open_document_slot(
    per_server_count: usize,
    global_counter: &AtomicUsize,
) -> AppResult<()> {
    if per_server_count >= MAX_OPEN_DOCUMENTS_PER_SERVER {
        return Err(AppError::Lsp(format!(
            "open document limit reached for language server ({MAX_OPEN_DOCUMENTS_PER_SERVER})"
        )));
    }
    reserve_counter_slot(global_counter, MAX_OPEN_DOCUMENTS_GLOBAL, || {
        AppError::Lsp(format!(
            "global open document limit reached ({MAX_OPEN_DOCUMENTS_GLOBAL})"
        ))
    })
}

pub(super) fn insert_pending_request(
    pending: &Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    global_counter: &AtomicUsize,
    id: i64,
    sender: mpsc::Sender<Value>,
) -> AppResult<()> {
    let mut pending = pending.lock().map_err(lsp)?;
    if pending.len() >= MAX_PENDING_REQUESTS_PER_SERVER {
        return Err(AppError::Lsp(format!(
            "pending request limit reached for language server ({MAX_PENDING_REQUESTS_PER_SERVER})"
        )));
    }
    if pending.contains_key(&id) {
        return Err(AppError::Lsp("duplicate language-server request id".into()));
    }
    reserve_counter_slot(global_counter, MAX_PENDING_REQUESTS_GLOBAL, || {
        AppError::Lsp(format!(
            "global pending request limit reached ({MAX_PENDING_REQUESTS_GLOBAL})"
        ))
    })?;
    pending.insert(id, sender);
    Ok(())
}

pub(super) fn take_pending_request(
    pending: &Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    global_counter: &AtomicUsize,
    id: i64,
) -> Option<mpsc::Sender<Value>> {
    let sender = pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id);
    if sender.is_some() {
        release_counter_slots(global_counter, 1);
    }
    sender
}

pub(super) fn clear_pending_requests(
    pending: &Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    global_counter: &AtomicUsize,
) {
    let count = {
        let mut pending = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = pending.len();
        pending.clear();
        count
    };
    release_counter_slots(global_counter, count);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_document_language_id_is_bounded_before_serialization() {
        validate_document_language_id(&"x".repeat(MAX_LSP_LANGUAGE_BYTES))
            .expect("boundary language id");
        assert_eq!(
            validate_document_language_id(&"x".repeat(MAX_LSP_LANGUAGE_BYTES + 1))
                .expect_err("oversized language id")
                .to_string(),
            format!("lsp: document language identifier exceeds {MAX_LSP_LANGUAGE_BYTES} bytes")
        );
        assert!(
            validate_document_language_id(&"é".repeat(MAX_LSP_LANGUAGE_BYTES / 2 + 1)).is_err()
        );
    }

    #[test]
    fn open_document_limits_reject_without_leaking_global_slots() {
        let counter = AtomicUsize::new(0);
        let per_server_error = reserve_open_document_slot(MAX_OPEN_DOCUMENTS_PER_SERVER, &counter)
            .expect_err("per-server cap");
        assert_eq!(
            per_server_error.to_string(),
            format!(
                "lsp: open document limit reached for language server ({MAX_OPEN_DOCUMENTS_PER_SERVER})"
            )
        );
        assert_eq!(counter.load(Ordering::Acquire), 0);

        let counter = AtomicUsize::new(MAX_OPEN_DOCUMENTS_GLOBAL);
        let global_error =
            reserve_open_document_slot(0, &counter).expect_err("global document cap");
        assert_eq!(
            global_error.to_string(),
            format!("lsp: global open document limit reached ({MAX_OPEN_DOCUMENTS_GLOBAL})")
        );
        assert_eq!(counter.load(Ordering::Acquire), MAX_OPEN_DOCUMENTS_GLOBAL);

        let counter = AtomicUsize::new(0);
        reserve_open_document_slot(0, &counter).expect("available slot");
        assert_eq!(counter.load(Ordering::Acquire), 1);
        release_counter_slots(&counter, 1);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn pending_request_limits_and_cleanup_are_exact() {
        let pending = Mutex::new(HashMap::new());
        let counter = AtomicUsize::new(0);
        let mut receivers = Vec::new();
        for id in 0..MAX_PENDING_REQUESTS_PER_SERVER as i64 {
            let (sender, receiver) = mpsc::channel();
            insert_pending_request(&pending, &counter, id, sender).expect("pending slot");
            receivers.push(receiver);
        }
        let (sender, _receiver) = mpsc::channel();
        let error = insert_pending_request(
            &pending,
            &counter,
            MAX_PENDING_REQUESTS_PER_SERVER as i64,
            sender,
        )
        .expect_err("per-server pending cap");
        assert_eq!(
            error.to_string(),
            format!(
                "lsp: pending request limit reached for language server ({MAX_PENDING_REQUESTS_PER_SERVER})"
            )
        );
        assert_eq!(
            pending.lock().expect("pending map").len(),
            MAX_PENDING_REQUESTS_PER_SERVER
        );
        assert_eq!(
            counter.load(Ordering::Acquire),
            MAX_PENDING_REQUESTS_PER_SERVER
        );

        assert!(take_pending_request(&pending, &counter, 0).is_some());
        assert_eq!(
            counter.load(Ordering::Acquire),
            MAX_PENDING_REQUESTS_PER_SERVER - 1
        );
        clear_pending_requests(&pending, &counter);
        assert!(pending.lock().expect("pending map").is_empty());
        assert_eq!(counter.load(Ordering::Acquire), 0);
        drop(receivers);

        let pending = Mutex::new(HashMap::new());
        let counter = AtomicUsize::new(MAX_PENDING_REQUESTS_GLOBAL);
        let (sender, _receiver) = mpsc::channel();
        let error =
            insert_pending_request(&pending, &counter, 1, sender).expect_err("global pending cap");
        assert_eq!(
            error.to_string(),
            format!("lsp: global pending request limit reached ({MAX_PENDING_REQUESTS_GLOBAL})")
        );
        assert!(pending.lock().expect("pending map").is_empty());
        assert_eq!(counter.load(Ordering::Acquire), MAX_PENDING_REQUESTS_GLOBAL);

        let pending = Mutex::new(HashMap::new());
        let counter = AtomicUsize::new(0);
        let (sender, _receiver) = mpsc::channel();
        insert_pending_request(&pending, &counter, 7, sender).expect("first request id");
        let (sender, _receiver) = mpsc::channel();
        assert_eq!(
            insert_pending_request(&pending, &counter, 7, sender)
                .expect_err("duplicate request id")
                .to_string(),
            "lsp: duplicate language-server request id"
        );
        assert_eq!(pending.lock().expect("pending map").len(), 1);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        clear_pending_requests(&pending, &counter);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }
}
