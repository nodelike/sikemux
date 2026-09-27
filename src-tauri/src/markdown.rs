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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::MarkdownRequest;

    #[test]
    fn reads_the_options_the_chat_sends() {
        let request: MarkdownRequest = serde_json::from_value(json!({
            "text": "a",
            "options": { "gfm": true, "htmlAsText": true, "fileLinks": true },
            "skip": 2,
        }))
        .unwrap();
        assert!(request.options.gfm && request.options.html_as_text && request.options.file_links);
        assert_eq!(request.skip, 2);
    }
}
