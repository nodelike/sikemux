use std::fmt;

use serde::{Deserialize, Serialize};

/// What kind of failure a call met. Clients branch on it, so it stays stable
/// while the words in a failure's message may change.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureCategory {
    /// Not signed in.
    Unconfigured,
    /// The service turned the sign-in down, so signing in again is what helps.
    Auth,
    /// Signed in, but this account may not do that.
    Forbidden,
    RateLimited,
    Http,
    BadParams,
    NotFound,
    Unsupported,
    MergeMethod,
    Keychain,
    /// The service answered in a shape the plugin did not expect.
    Response,
    UnknownMethod,
    StreamClosed,
    Internal,
    Manifest,
    TimedOut,
    NotInstalled,
    Disabled,
    Stopped,
    /// The service could not be reached at all.
    Offline,
    /// The service was reached but did not answer in time.
    Timeout,
    /// The core could not make sense of what it was asked or answered.
    Invalid,
    /// A service's own error, named by the service.
    #[serde(untagged)]
    Own(String),
}

impl FailureCategory {
    pub fn own(name: &str) -> Self {
        Self::Own(name.to_owned())
    }
}

/// Clients branch on `category`, so a plugin keeps its categories stable and
/// puts the human-readable detail in `message`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PluginError {
    pub category: FailureCategory,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl PluginError {
    pub fn new(category: FailureCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
            status: None,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Signed in, but this account may not do that. A client keeps the person
    /// signed in, unlike category `auth` or `http` with status 401.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::Forbidden, message).with_status(403)
    }

    pub fn http(status: u16, message: impl Into<String>) -> Self {
        if status == 403 {
            return Self::forbidden(message);
        }
        Self::new(FailureCategory::Http, message).with_status(status)
    }

    pub fn unknown_method(method: &str) -> Self {
        Self::new(
            FailureCategory::UnknownMethod,
            format!("no method named `{method}`"),
        )
    }

    pub fn bad_params(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::BadParams, message)
    }

    pub fn stream_closed() -> Self {
        Self::new(
            FailureCategory::StreamClosed,
            "nobody is listening to this stream",
        )
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
