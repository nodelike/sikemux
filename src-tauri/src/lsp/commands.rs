use std::sync::atomic::Ordering;

use serde_json::json;
use tauri::AppHandle;
use tokio::task;

use super::discovery::install_gopls;
use super::documents::{
    change_document, change_document_incremental, open_document, restore_last_change,
};
use super::limits::{
    release_counter_slots, server_limit_error, validate_document_language_id,
    validate_document_path, validate_server_identity, MAX_LSP_PATH_BYTES, MAX_LSP_SERVERS,
    OPEN_DOCUMENT_COUNT,
};
use super::lsp;
use super::process::spawn_server;
use super::protocol::{parse_document_symbols, parse_locations, path_to_uri};
use super::registry::{
    live_server_exists, prepare_server_start, registry, schedule_idle_shutdown, server_for,
    server_keys_for_project, start_lock, PreparedServerStart,
};
use super::server::{shutdown_server, ServerKey};
use super::transport::{notify, request};
use super::types::{LspDocumentSymbol, LspKind, LspLocation, LspTextChange};
use crate::error::{AppError, AppResult};
use crate::observability::{global_observability, Metadata, SpanOutcome};

#[tauri::command]
pub async fn lsp_install_server(language: String) -> AppResult<String> {
    match language.as_str() {
        "go" => task::spawn_blocking(install_gopls)
            .await
            .map_err(|e| AppError::Lsp(format!("join: {e}")))?,
        _ => Err(AppError::Lsp(format!(
            "no language-server installer configured for `{language}`"
        ))),
    }
}

#[tauri::command]
pub async fn lsp_start(app: AppHandle, project: String, language: String) -> AppResult<()> {
    let span = global_observability().begin_span("lsp.start", None, Metadata::new());
    let result = lsp_start_inner(app, project, language).await;
    span.finish(if result.is_ok() {
        SpanOutcome::Success
    } else {
        SpanOutcome::Error
    });
    result
}

async fn lsp_start_inner(app: AppHandle, project: String, language: String) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    let server_key = ServerKey::new(&project, &language);
    // The initialize handshake blocks up to 20 s on slow servers; off the
    // Tauri worker pool so unrelated IPC isn't starved. The start lock spans
    // admission, process initialization, and registry insertion, making the
    // cap a hard pre-spawn admission limit rather than an eventual backstop.
    task::spawn_blocking(move || -> AppResult<()> {
        let _guard = start_lock().lock().map_err(lsp)?;
        if live_server_exists(&server_key)? {
            let existing = registry().lock().ok().and_then(|registry| {
                registry
                    .get(&server_key)
                    .filter(|server| !server.shutdown.load(Ordering::Acquire))
                    .cloned()
            });
            if let Some(server) = existing {
                server.idle_generation.fetch_add(1, Ordering::AcqRel);
                return Ok(());
            }
        }
        if prepare_server_start(&server_key)? == PreparedServerStart::Existing {
            return Ok(());
        }

        let server = spawn_server(&project, &language, app)?;
        let insertion = {
            let mut registry = registry().lock().map_err(lsp)?;
            if registry.len() >= MAX_LSP_SERVERS {
                Err(server_limit_error())
            } else {
                registry.insert(server_key, server.clone());
                Ok(())
            }
        };
        if let Err(error) = insertion {
            shutdown_server(server);
            return Err(error);
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

/// Shut every server owned by this project down. Called from the close-
/// session path so a long-running rust-analyzer doesn't hang around with
/// 500 MB resident after the user moves on. Idempotent.
#[tauri::command]
pub async fn lsp_stop(project: String) -> AppResult<()> {
    let span = global_observability().begin_span("lsp.stop", None, Metadata::new());
    let result = lsp_stop_inner(project).await;
    span.finish(if result.is_ok() {
        SpanOutcome::Success
    } else {
        SpanOutcome::Error
    });
    result
}

async fn lsp_stop_inner(project: String) -> AppResult<()> {
    if project.len() > MAX_LSP_PATH_BYTES {
        return Err(AppError::Lsp(format!(
            "project path exceeds {MAX_LSP_PATH_BYTES} bytes"
        )));
    }
    // Serialize against start so a stop racing a 20-second initialize cannot
    // miss the process and let it appear in the registry after stop returns.
    task::spawn_blocking(move || -> AppResult<()> {
        let _guard = start_lock().lock().map_err(lsp)?;
        let to_kill = {
            let mut registry = registry().lock().map_err(lsp)?;
            server_keys_for_project(&registry, &project)
                .into_iter()
                .filter_map(|key| registry.remove(&key))
                .collect::<Vec<_>>()
        };
        for server in to_kill {
            shutdown_server(server);
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_open(
    project: String,
    language: String,
    path: String,
    content: String,
    language_id: Option<String>,
) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    if let Some(language_id) = language_id.as_deref() {
        validate_document_language_id(language_id)?;
    }
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    server
        .idle_generation
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    task::spawn_blocking(move || {
        let language_id = language_id.unwrap_or(language);
        open_document(&server, &path, content, &language_id)
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_change(
    project: String,
    language: String,
    path: String,
    content: String,
    version: u32,
) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    task::spawn_blocking(move || change_document(&server, &path, content, version))
        .await
        .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_change_incremental(
    project: String,
    language: String,
    path: String,
    changes: Vec<LspTextChange>,
    version: u32,
) -> AppResult<()> {
    if changes.is_empty() {
        return Ok(());
    }
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    task::spawn_blocking(move || change_document_incremental(&server, &path, changes, version))
        .await
        .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_save(
    project: String,
    language: String,
    path: String,
    content: Option<String>,
) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let Some(server) = server_for(&project, &language) else {
        return Ok(());
    };
    task::spawn_blocking(move || {
        let mut params = json!({ "textDocument": { "uri": path_to_uri(&path) } });
        if let Some(text) = content {
            params["text"] = json!(text);
        }
        notify(&server, "textDocument/didSave", params)
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))?
}

#[tauri::command]
pub async fn lsp_close(project: String, language: String, path: String) -> AppResult<()> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let Some(server) = server_for(&project, &language) else {
        return Ok(());
    };
    let server_key = ServerKey::new(&project, &language);
    let idle_server = server.clone();
    let became_idle = task::spawn_blocking(move || -> AppResult<bool> {
        let mut documents = server.open_docs.lock().map_err(lsp)?;
        let Some(document) = documents.get(&path).copied() else {
            return Ok(false);
        };
        if document.refs > 1 {
            documents
                .get_mut(&path)
                .expect("document disappeared while its map is locked")
                .refs -= 1;
            return Ok(false);
        }
        let mut last_change = server.last_change.lock().map_err(lsp)?;
        let previous_hash = last_change.remove(&path);
        documents.remove(&path);
        let result = notify(
            &server,
            "textDocument/didClose",
            json!({ "textDocument": { "uri": path_to_uri(&path) } }),
        );
        if let Err(error) = result {
            documents.insert(path.clone(), document);
            restore_last_change(&mut last_change, &path, previous_hash);
            return Err(error);
        }
        release_counter_slots(&OPEN_DOCUMENT_COUNT, 1);
        Ok(documents.is_empty())
    })
    .await
    .map_err(|e| AppError::Lsp(format!("join: {e}")))??;
    if became_idle {
        schedule_idle_shutdown(server_key, idle_server);
    }
    Ok(())
}

#[tauri::command]
pub async fn lsp_locations(
    project: String,
    language: String,
    path: String,
    line: u32,
    character: u32,
    kind: LspKind,
) -> AppResult<Vec<LspLocation>> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    let (method, with_context) = match kind {
        LspKind::Definition => ("textDocument/definition", false),
        LspKind::Declaration => ("textDocument/declaration", false),
        LspKind::TypeDefinition => ("textDocument/typeDefinition", false),
        LspKind::Implementation => ("textDocument/implementation", false),
        LspKind::References => ("textDocument/references", true),
    };
    let mut params = json!({
        "textDocument": { "uri": path_to_uri(&path) },
        "position": { "line": line, "character": character }
    });
    if with_context {
        params["context"] = json!({ "includeDeclaration": false });
    }
    let result = task::spawn_blocking(move || request(&server, method, params))
        .await
        .map_err(|e| AppError::Lsp(format!("join: {e}")))??;
    Ok(parse_locations(&result))
}

#[tauri::command]
pub async fn lsp_document_symbols(
    project: String,
    language: String,
    path: String,
) -> AppResult<Vec<LspDocumentSymbol>> {
    validate_server_identity(&project, &language)?;
    validate_document_path(&path)?;
    let server =
        server_for(&project, &language).ok_or(AppError::Lsp("server not started".into()))?;
    let params = json!({ "textDocument": { "uri": path_to_uri(&path) } });
    // Use the same bounded blocking request path as locations: shutdown drops
    // the response sender, and timeout removes the pending request in 4 s.
    let result =
        task::spawn_blocking(move || request(&server, "textDocument/documentSymbol", params))
            .await
            .map_err(|error| AppError::Lsp(format!("join: {error}")))??;
    Ok(parse_document_symbols(&result))
}
