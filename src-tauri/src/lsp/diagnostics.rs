use std::collections::HashMap;

use serde_json::Value;
use tauri::Emitter;

use super::limits::{MAX_LSP_LANGUAGE_BYTES, MAX_LSP_PATH_BYTES};
use super::protocol::{bounded_string, parse_lsp_range, uri_to_bounded_path};
use super::server::Server;
use super::types::{LspDiagnostic, LspDiagnosticSeverity, LspDiagnosticsPayload};

const MAX_DIAGNOSTIC_FILES_PER_SERVER: usize = 512;
const MAX_DIAGNOSTICS_PER_PUBLISH: usize = 500;
const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 2_048;
const MAX_DIAGNOSTIC_SOURCE_BYTES: usize = 128;
const MAX_DIAGNOSTIC_CODE_BYTES: usize = 128;

pub const LSP_DIAGNOSTICS_EVENT: &str = "lsp_diagnostics";

fn parse_diagnostic_code(value: Option<&Value>) -> Option<String> {
    let code = match value? {
        Value::String(code) => code.clone(),
        Value::Number(code) => code.to_string(),
        _ => return None,
    };
    Some(bounded_string(&code, MAX_DIAGNOSTIC_CODE_BYTES))
}

fn parse_diagnostic(value: &Value) -> Option<LspDiagnostic> {
    let range = parse_lsp_range(value.get("range")?)?;
    let message = bounded_string(
        value.get("message")?.as_str()?,
        MAX_DIAGNOSTIC_MESSAGE_BYTES,
    );
    let severity = match value.get("severity").and_then(Value::as_u64) {
        Some(1) => Some(LspDiagnosticSeverity::Error),
        Some(2) => Some(LspDiagnosticSeverity::Warning),
        Some(3) => Some(LspDiagnosticSeverity::Information),
        Some(4) => Some(LspDiagnosticSeverity::Hint),
        _ => None,
    };
    let source = value
        .get("source")
        .and_then(Value::as_str)
        .map(|source| bounded_string(source, MAX_DIAGNOSTIC_SOURCE_BYTES));
    Some(LspDiagnostic {
        range,
        severity,
        code: parse_diagnostic_code(value.get("code")),
        source,
        message,
    })
}

fn parse_diagnostics_payload(
    project: &str,
    language: &str,
    params: &Value,
) -> Option<LspDiagnosticsPayload> {
    if project.len() > MAX_LSP_PATH_BYTES || language.len() > MAX_LSP_LANGUAGE_BYTES {
        return None;
    }
    let path = uri_to_bounded_path(params.get("uri")?.as_str()?)?;
    let values = params.get("diagnostics")?.as_array()?;
    let diagnostics = values
        .iter()
        .take(MAX_DIAGNOSTICS_PER_PUBLISH)
        .filter_map(parse_diagnostic)
        .collect();
    Some(LspDiagnosticsPayload {
        project: project.to_owned(),
        language: language.to_owned(),
        path,
        version: params.get("version").and_then(Value::as_i64),
        diagnostics,
    })
}

fn track_diagnostic_publish(
    tracked: &mut HashMap<String, Option<i64>>,
    path: &str,
    version: Option<i64>,
    has_diagnostics: bool,
) -> bool {
    if !has_diagnostics {
        tracked.remove(path);
        return true;
    }
    if !tracked.contains_key(path) && tracked.len() >= MAX_DIAGNOSTIC_FILES_PER_SERVER {
        return false;
    }
    tracked.insert(path.to_owned(), version);
    true
}

fn publish_diagnostics(server: &Server, params: &Value) {
    let Some(payload) = parse_diagnostics_payload(&server.project, &server.language, params) else {
        return;
    };
    let mut tracked = match server.diagnostic_paths.lock() {
        Ok(tracked) => tracked,
        Err(poisoned) => poisoned.into_inner(),
    };
    if server.shutdown.load(std::sync::atomic::Ordering::Acquire)
        || !track_diagnostic_publish(
            &mut tracked,
            &payload.path,
            payload.version,
            !payload.diagnostics.is_empty(),
        )
    {
        return;
    }

    // Keep the tracking lock through emission. Shutdown flips its atomic flag
    // before taking this lock, guaranteeing that a non-empty publish racing
    // teardown is always followed by the corresponding clear event.
    let _ = server.app.emit_to("main", LSP_DIAGNOSTICS_EVENT, payload);
}

pub(super) fn clear_server_diagnostics(server: &Server) {
    let tracked = {
        let mut tracked = match server.diagnostic_paths.lock() {
            Ok(tracked) => tracked,
            Err(poisoned) => poisoned.into_inner(),
        };
        tracked.drain().collect::<Vec<_>>()
    };
    for (path, version) in tracked {
        let _ = server.app.emit_to(
            "main",
            LSP_DIAGNOSTICS_EVENT,
            LspDiagnosticsPayload {
                project: server.project.clone(),
                language: server.language.clone(),
                path,
                version,
                diagnostics: Vec::new(),
            },
        );
    }
}

pub(super) fn handle_server_notification(server: &Server, method: &str, params: &Value) {
    if method == "textDocument/publishDiagnostics" {
        publish_diagnostics(server, params);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use url::Url;

    fn test_range() -> Value {
        json!({
            "start": { "line": 1, "character": 2 },
            "end": { "line": 3, "character": 4 }
        })
    }

    #[test]
    fn diagnostics_payload_is_typed_bounded_and_empty_publish_clears() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("main.rs");
        let uri = Url::from_file_path(&path).unwrap().to_string();
        let mut diagnostics = vec![json!({
            "range": test_range(),
            "severity": 1,
            "code": 42,
            "source": "s".repeat(MAX_DIAGNOSTIC_SOURCE_BYTES + 20),
            "message": "é".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES)
        })];
        diagnostics.extend((1..MAX_DIAGNOSTICS_PER_PUBLISH + 10).map(|index| {
            json!({
                "range": test_range(),
                "severity": 2,
                "message": format!("warning {index}")
            })
        }));

        let payload = parse_diagnostics_payload(
            temp.path().to_string_lossy().as_ref(),
            "rust",
            &json!({ "uri": uri, "version": 7, "diagnostics": diagnostics }),
        )
        .unwrap();
        assert_eq!(payload.path, path.to_string_lossy());
        assert_eq!(payload.version, Some(7));
        assert_eq!(payload.diagnostics.len(), MAX_DIAGNOSTICS_PER_PUBLISH);
        assert_eq!(
            payload.diagnostics[0].severity,
            Some(LspDiagnosticSeverity::Error)
        );
        assert_eq!(payload.diagnostics[0].code.as_deref(), Some("42"));
        assert!(payload.diagnostics[0].message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
        assert!(payload.diagnostics[0]
            .message
            .is_char_boundary(payload.diagnostics[0].message.len()));
        assert!(payload.diagnostics[0]
            .source
            .as_ref()
            .is_some_and(|source| source.len() == MAX_DIAGNOSTIC_SOURCE_BYTES));

        let cleared = parse_diagnostics_payload(
            temp.path().to_string_lossy().as_ref(),
            "rust",
            &json!({ "uri": Url::from_file_path(&path).unwrap(), "diagnostics": [] }),
        )
        .unwrap();
        assert!(cleared.diagnostics.is_empty());
        assert!(parse_diagnostics_payload(
            temp.path().to_string_lossy().as_ref(),
            "rust",
            &json!({ "uri": "https://example.com/main.rs", "diagnostics": [] })
        )
        .is_none());
    }

    #[test]
    fn diagnostic_tracking_replaces_clears_and_caps_paths() {
        let mut tracked = HashMap::new();
        assert!(track_diagnostic_publish(
            &mut tracked,
            "main.rs",
            Some(1),
            true
        ));
        assert!(track_diagnostic_publish(
            &mut tracked,
            "main.rs",
            Some(2),
            true
        ));
        assert_eq!(tracked.get("main.rs"), Some(&Some(2)));
        assert!(track_diagnostic_publish(
            &mut tracked,
            "main.rs",
            Some(2),
            false
        ));
        assert!(!tracked.contains_key("main.rs"));

        for index in 0..MAX_DIAGNOSTIC_FILES_PER_SERVER {
            assert!(track_diagnostic_publish(
                &mut tracked,
                &format!("file-{index}"),
                None,
                true
            ));
        }
        assert!(!track_diagnostic_publish(
            &mut tracked,
            "one-too-many",
            None,
            true
        ));
        assert_eq!(tracked.len(), MAX_DIAGNOSTIC_FILES_PER_SERVER);
    }
}
