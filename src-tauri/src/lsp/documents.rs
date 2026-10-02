use std::collections::HashMap;
use std::sync::atomic::Ordering;

use serde_json::{json, Value};

use super::limits::{release_counter_slots, reserve_open_document_slot, OPEN_DOCUMENT_COUNT};
use super::lsp;
use super::protocol::path_to_uri;
use super::server::{OpenDoc, ServerHandle};
use super::transport::notify;
use super::types::LspTextChange;
use crate::error::{AppError, AppResult};

fn content_hash(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn restore_last_change(
    last_change: &mut HashMap<String, u64>,
    path: &str,
    value: Option<u64>,
) {
    if let Some(value) = value {
        last_change.insert(path.to_owned(), value);
    } else {
        last_change.remove(path);
    }
}

/// Takes the document text by value: the buffer is handed straight to the
/// outbound frame instead of being copied into it, and an editor document can
/// be megabytes.
pub(super) fn open_document(
    server: &ServerHandle,
    path: &str,
    content: String,
    language_id: &str,
) -> AppResult<()> {
    if server.shutdown.load(Ordering::Acquire) {
        return Err(AppError::Lsp("server shut down".into()));
    }
    let hash = content_hash(&content);
    let mut documents = server.open_docs.lock().map_err(lsp)?;

    if documents.contains_key(path) {
        let mut last_change = server.last_change.lock().map_err(lsp)?;
        let previous = *documents
            .get(path)
            .expect("document disappeared while its map is locked");
        let previous_hash = last_change.get(path).copied();
        if previous.refs == usize::MAX {
            return Err(AppError::Lsp(
                "open document reference count exhausted".into(),
            ));
        }
        if previous_hash == Some(hash) {
            documents
                .get_mut(path)
                .expect("document disappeared while its map is locked")
                .refs += 1;
            return Ok(());
        }

        let next_version = previous.version.saturating_add(1);
        *documents
            .get_mut(path)
            .expect("document disappeared while its map is locked") = OpenDoc {
            refs: previous.refs + 1,
            version: next_version,
        };
        last_change.insert(path.to_owned(), hash);
        let result = notify(
            server,
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": next_version },
                "contentChanges": [{ "text": Value::String(content) }]
            }),
        );
        if result.is_err() {
            documents.insert(path.to_owned(), previous);
            restore_last_change(&mut last_change, path, previous_hash);
        }
        return result;
    }

    reserve_open_document_slot(documents.len(), &OPEN_DOCUMENT_COUNT)?;
    let mut last_change = match server.last_change.lock() {
        Ok(last_change) => last_change,
        Err(error) => {
            release_counter_slots(&OPEN_DOCUMENT_COUNT, 1);
            return Err(lsp(error));
        }
    };
    let previous_hash = last_change.insert(path.to_owned(), hash);
    documents.insert(
        path.to_owned(),
        OpenDoc {
            refs: 1,
            version: 1,
        },
    );
    let result = notify(
        server,
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": path_to_uri(path),
                "languageId": language_id,
                "version": 1,
                "text": Value::String(content)
            }
        }),
    );
    if result.is_err() {
        documents.remove(path);
        restore_last_change(&mut last_change, path, previous_hash);
        release_counter_slots(&OPEN_DOCUMENT_COUNT, 1);
    }
    result
}

pub(super) fn change_document(
    server: &ServerHandle,
    path: &str,
    content: String,
    requested_version: u32,
) -> AppResult<()> {
    if server.shutdown.load(Ordering::Acquire) {
        return Err(AppError::Lsp("server shut down".into()));
    }
    let hash = content_hash(&content);
    let mut documents = server.open_docs.lock().map_err(lsp)?;
    let previous = *documents
        .get(path)
        .ok_or_else(|| AppError::Lsp("document not open".into()))?;
    let mut last_change = server.last_change.lock().map_err(lsp)?;
    let previous_hash = last_change.get(path).copied();
    if previous_hash == Some(hash) {
        return Ok(());
    }
    let version = requested_version.max(previous.version.saturating_add(1));
    documents
        .get_mut(path)
        .expect("document disappeared while its map is locked")
        .version = version;
    last_change.insert(path.to_owned(), hash);
    let result = notify(
        server,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": path_to_uri(path), "version": version },
            "contentChanges": [{ "text": Value::String(content) }]
        }),
    );
    if result.is_err() {
        documents.insert(path.to_owned(), previous);
        restore_last_change(&mut last_change, path, previous_hash);
    }
    result
}

pub(super) fn change_document_incremental(
    server: &ServerHandle,
    path: &str,
    changes: Vec<LspTextChange>,
    requested_version: u32,
) -> AppResult<()> {
    if server.shutdown.load(Ordering::Acquire) {
        return Err(AppError::Lsp("server shut down".into()));
    }
    let mut documents = server.open_docs.lock().map_err(lsp)?;
    let previous = *documents
        .get(path)
        .ok_or_else(|| AppError::Lsp("document not open".into()))?;
    let mut last_change = server.last_change.lock().map_err(lsp)?;
    let previous_hash = last_change.remove(path);
    let version = requested_version.max(previous.version.saturating_add(1));
    documents
        .get_mut(path)
        .expect("document disappeared while its map is locked")
        .version = version;
    let changes = changes
        .into_iter()
        .map(|change| {
            json!({
                "range": change.range,
                "rangeLength": change.range_length,
                // Moved, not copied: a paste arrives here as one edit.
                "text": Value::String(change.text),
            })
        })
        .collect::<Vec<_>>();
    let result = notify(
        server,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": path_to_uri(path), "version": version },
            "contentChanges": changes
        }),
    );
    if result.is_err() {
        documents.insert(path.to_owned(), previous);
        restore_last_change(&mut last_change, path, previous_hash);
    }
    result
}
