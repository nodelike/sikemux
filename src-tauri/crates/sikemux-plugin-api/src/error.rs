use std::fmt;

use serde::Serialize;

/// The frontend branches on `category`, so a plugin keeps its categories
/// stable and puts the human-readable detail in `message`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PluginError {
    pub category: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl PluginError {
    pub fn new(category: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            category: category.into(),
            message: message.into(),
            status: None,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Signed in, but this account may not do that. The frontend keeps the
    /// person signed in, unlike category `auth` or `http` with status 401.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new("forbidden", message).with_status(403)
    }

    pub fn http(status: u16, message: impl Into<String>) -> Self {
        if status == 403 {
            return Self::forbidden(message);
        }
        Self::new("http", message).with_status(status)
    }

    pub fn unknown_method(method: &str) -> Self {
        Self::new("unknown-method", format!("no method named `{method}`"))
    }

    pub fn bad_params(message: impl Into<String>) -> Self {
        Self::new("bad-params", message)
    }

    pub fn stream_closed() -> Self {
        Self::new("stream-closed", "nobody is listening to this stream")
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PluginError {}

impl From<serde_json::Error> for PluginError {
    fn from(error: serde_json::Error) -> Self {
        Self::bad_params(error.to_string())
    }
}

pub type PluginResult<T> = Result<T, PluginError>;
