use serde::Deserialize;
use sikemux_markdown::{parse, Element, Options};
use tauri::async_runtime::spawn_blocking;

use crate::error::{AppError, AppResult};

#[derive(Deserialize)]
pub struct MarkdownRequest {
    text: String,
    options: Options,
    skip: usize,
}

/// Every message a transcript draws in the same frame arrives as one request.
#[tauri::command]
pub async fn markdown_parse(requests: Vec<MarkdownRequest>) -> AppResult<Vec<Vec<Element>>> {
    spawn_blocking(move || {
        requests
            .iter()
            .map(|request| parse(&request.text, request.options, request.skip))
            .collect()
    })
    .await
    .map_err(|e| AppError::Other(format!("markdown join: {e}")))
}
