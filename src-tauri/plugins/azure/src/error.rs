use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum AzureError {
    /// The Azure CLI isn't installed.
    NoCli,
    /// The Azure CLI has no account signed in.
    Unconfigured,
    Auth(String),
    Forbidden(String),
    NotFound(String),
    RateLimited,
    /// Azure answered with an error code and message.
    Api {
        status: u16,
        code: String,
        message: String,
    },
    Cli(String),
    BadArg(String),
    Transport(String),
    /// Azure could not be reached at all, such as when the Mac is offline.
    Unreachable(sikemux_plugin_api::Unreachable),
    Response(String),
}

pub type AzureResult<T> = Result<T, AzureError>;

impl fmt::Display for AzureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCli => formatter.write_str(
                "azure: the Azure CLI isn't installed. Install it (brew install azure-cli), then sign in with az login.",
            ),
            Self::Unconfigured => formatter.write_str("azure: not signed in; run az login"),
            Self::Auth(message) => write!(formatter, "azure: sign-in expired: {message}"),
            Self::Forbidden(message) => write!(formatter, "azure: not allowed: {message}"),
            Self::NotFound(message) => write!(formatter, "azure: {message}"),
            Self::RateLimited => {
                formatter.write_str("azure: too many requests; try again in a minute")
            }
            Self::Api {
                status,
                code,
                message,
            } => write!(formatter, "azure: {code} ({status}): {message}"),
            Self::Cli(message) => write!(formatter, "azure cli: {message}"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Transport(message) => write!(formatter, "azure: {message}"),
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "Azure"),
            ),
            Self::Response(message) => write!(formatter, "azure: unexpected response: {message}"),
        }
    }
}

impl From<reqwest::Error> for AzureError {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("azure: {error}");
            return Self::Unreachable(kind);
        }
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for AzureError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<AzureError> for PluginError {
    fn from(error: AzureError) -> Self {
        let message = error.to_string();
        match error {
            AzureError::NoCli => PluginError::new("cli-missing", message),
            AzureError::Unconfigured => PluginError::new("unconfigured", message),
            AzureError::Auth(_) => PluginError::new("auth", message),
            AzureError::Forbidden(_) => PluginError::new("forbidden", message).with_status(403),
            AzureError::NotFound(_) => PluginError::new("not-found", message),
            AzureError::RateLimited => PluginError::new("rate-limited", message).with_status(429),
            AzureError::Unreachable(kind) => PluginError::unreachable(kind, "Azure"),
            AzureError::Api { status, .. } => PluginError::http(status, message),
            AzureError::BadArg(_) => PluginError::new("bad-params", message),
            AzureError::Cli(_) | AzureError::Transport(_) => PluginError::new("azure", message),
            AzureError::Response(_) => PluginError::new("response", message),
        }
    }
}
