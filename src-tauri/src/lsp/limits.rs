use std::collections::HashMap;
use std::sync::{mpsc, Mutex};

use serde_json::Value;
use sikemux_process::sync::Locked;

use crate::error::{AppError, AppResult};

pub(super) const MAX_LSP_PATH_BYTES: usize = 4_096;
pub(super) const MAX_LSP_LANGUAGE_BYTES: usize = 128;

pub(super) const MAX_LSP_SERVERS: usize = 6;
const MAX_OPEN_DOCUMENTS_PER_SERVER: usize = 512;
const MAX_PENDING_REQUESTS_PER_SERVER: usize = 64;

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

pub(super) fn check_open_document_slot(open_documents: usize) -> AppResult<()> {
    if open_documents >= MAX_OPEN_DOCUMENTS_PER_SERVER {
        return Err(AppError::Lsp(format!(
            "open document limit reached for language server ({MAX_OPEN_DOCUMENTS_PER_SERVER})"
        )));
    }
    Ok(())
}

pub(super) fn insert_pending_request(
    pending: &Mutex<HashMap<i64, mpsc::Sender<Value>>>,
    id: i64,
    sender: mpsc::Sender<Value>,
) -> AppResult<()> {
    let mut pending = pending.locked();
    if pending.len() >= MAX_PENDING_REQUESTS_PER_SERVER {
        return Err(AppError::Lsp(format!(
            "pending request limit reached for language server ({MAX_PENDING_REQUESTS_PER_SERVER})"
        )));
    }
    if pending.contains_key(&id) {
        return Err(AppError::Lsp("duplicate language-server request id".into()));
    }
    pending.insert(id, sender);
    Ok(())
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
    fn pending_requests_are_capped_per_server_and_ids_are_unique() {
        let pending = Mutex::new(HashMap::new());
        let mut receivers = Vec::new();
        for id in 0..MAX_PENDING_REQUESTS_PER_SERVER as i64 {
            let (sender, receiver) = mpsc::channel();
            insert_pending_request(&pending, id, sender).expect("pending slot");
            receivers.push(receiver);
        }
        let (sender, _receiver) = mpsc::channel();
        let error =
            insert_pending_request(&pending, MAX_PENDING_REQUESTS_PER_SERVER as i64, sender)
                .expect_err("per-server pending cap");
        assert_eq!(
            error.to_string(),
            format!(
                "lsp: pending request limit reached for language server ({MAX_PENDING_REQUESTS_PER_SERVER})"
            )
        );
        drop(receivers);

        let pending = Mutex::new(HashMap::new());
        let (sender, _receiver) = mpsc::channel();
        insert_pending_request(&pending, 7, sender).expect("first request id");
        let (sender, _receiver) = mpsc::channel();
        assert_eq!(
            insert_pending_request(&pending, 7, sender)
                .expect_err("duplicate request id")
                .to_string(),
            "lsp: duplicate language-server request id"
        );
    }
}
