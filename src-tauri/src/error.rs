// Application-wide error taxonomy. Every Tauri command's Err arm flows
// through a variant of `AppError`; the IntoResponse `Display` impl renders
// what the frontend gets. Callers in the frontend that need to branch on a
// specific failure (auth expired vs cli missing vs network error) read the
// shape via the serde-tagged `category()` rather than substring-matching the
// rendered message.

use std::io;

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("git: {0}")]
    Git(String),

    #[error("lsp: {0}")]
    Lsp(String),

    #[error("lsp server `{bin}` for {language} not found")]
    LspServerMissing { language: String, bin: String },

    #[error("http: {0}")]
    Http(String),

    #[error("{plugin}: {error}")]
    Plugin {
        plugin: String,
        error: sikemux_plugin_api::PluginError,
    },

    #[error("invalid argument: {0}")]
    BadArg(&'static str),

    #[error("pty: {0}")]
    Pty(String),

    #[error("search: {0}")]
    Search(String),

    #[error("fs: {0}")]
    Fs(String),

    #[error("file conflict: {0}")]
    FileConflict(String),

    #[error("watch: {0}")]
    Watch(String),

    #[error("state: {0}")]
    State(String),

    #[error("window: {0}")]
    #[allow(dead_code)]
    Window(String),

    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Http(e.to_string())
    }
}

impl From<git2::Error> for AppError {
    fn from(e: git2::Error) -> Self {
        AppError::Git(e.message().to_string())
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::Other(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::Other(s.to_string())
    }
}

// Tauri's #[command] requires the Err type to be Serialize so it lands as a
// JSON payload on the frontend side. We emit `{ category, message }` so the
// frontend can branch on category without parsing message text.
#[derive(Serialize)]
struct Wire<'a> {
    category: &'a str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin: Option<&'a str>,
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let (category, status, plugin) = match self {
            AppError::Plugin { plugin, error } => {
                (error.category.as_str(), error.status, Some(plugin.as_str()))
            }
            _ => (self.category(), None, None),
        };
        Wire {
            category,
            message: self.to_string(),
            status,
            plugin,
        }
        .serialize(ser)
    }
}

impl AppError {
    pub fn category(&self) -> &'static str {
        match self {
            AppError::Io(_) => "io",
            AppError::Json(_) => "json",
            AppError::Git(_) => "git",
            AppError::Lsp(_) => "lsp",
            AppError::LspServerMissing { .. } => "lsp-server-missing",
            AppError::Http(_) => "http",
            AppError::Plugin { .. } => "plugin",
            AppError::BadArg(_) => "bad-arg",
            AppError::Pty(_) => "pty",
            AppError::Search(_) => "search",
            AppError::Fs(_) => "fs",
            AppError::FileConflict(_) => "file-conflict",
            AppError::Watch(_) => "watch",
            AppError::State(_) => "state",
            AppError::Window(_) => "window",
            AppError::Other(_) => "other",
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_is_stable() {
        // Spot-check categories the frontend branches on. If any of these
        // change, every `err.category === "..."` check in api/* must be
        // updated to match.
        assert_eq!(
            AppError::LspServerMissing {
                language: "go".into(),
                bin: "gopls".into(),
            }
            .category(),
            "lsp-server-missing"
        );
        assert_eq!(AppError::BadArg("x").category(), "bad-arg");
        assert_eq!(AppError::Pty("x".into()).category(), "pty");
        assert_eq!(AppError::Watch("x".into()).category(), "watch");
        assert_eq!(AppError::State("x".into()).category(), "state");
    }

    #[test]
    fn wire_payload_round_trip() {
        let e = AppError::Plugin {
            plugin: "sikemux.rundeck".into(),
            error: sikemux_plugin_api::PluginError::new("http", "boom").with_status(401),
        };
        let s = serde_json::to_string(&e).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["category"], "http");
        assert_eq!(v["status"], 401);
        assert_eq!(v["plugin"], "sikemux.rundeck");
    }
}
