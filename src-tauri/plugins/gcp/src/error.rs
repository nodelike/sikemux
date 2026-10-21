use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum GcpError {
    /// The `gcloud` CLI is not on the person's PATH.
    NoGcloud,
    /// No account is signed in to gcloud.
    Unconfigured,
    Auth(String),
    /// The API the call needs is turned off in the project.
    ServiceDisabled {
        service: String,
        url: Option<String>,
    },
    /// The project has no billing account, which Google needs before most APIs can be turned on.
    NoBilling,
    Permission(String),
    NotFound(String),
    RateLimited,
    Api {
        status: u16,
        message: String,
    },
    BadArg(String),
    Gcloud(String),
    Transport(String),
    /// The service could not be reached at all, such as when the Mac is offline.
    Unreachable(sikemux_plugin_api::Unreachable),
    Response(String),
}

pub type GcpResult<T> = Result<T, GcpError>;

impl fmt::Display for GcpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoGcloud => formatter.write_str(
                "gcp: the gcloud CLI is not installed; install the Google Cloud SDK, then sign in",
            ),
            Self::Unconfigured => formatter.write_str("gcp: no account is signed in to gcloud"),
            Self::Auth(message) => write!(formatter, "gcp: sign-in failed: {message}"),
            Self::ServiceDisabled { service, url } => match url {
                Some(url) => write!(
                    formatter,
                    "gcp: the {service} API is turned off in this project; turn it on at {url}"
                ),
                None => write!(
                    formatter,
                    "gcp: the {service} API is turned off in this project"
                ),
            },
            Self::NoBilling => formatter.write_str(
                "gcp: billing isn't enabled for this project; link a billing account before turning on this API",
            ),
            Self::Permission(message) => write!(formatter, "gcp: not allowed: {message}"),
            Self::NotFound(message) => write!(formatter, "gcp: {message}"),
            Self::RateLimited => {
                formatter.write_str("gcp: too many requests; try again in a minute")
            }
            Self::Api { status, message } => write!(formatter, "gcp: http {status}: {message}"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Gcloud(message) => write!(formatter, "gcloud: {message}"),
            Self::Transport(message) => write!(formatter, "gcp: {message}"),
            Self::Unreachable(kind) => formatter.write_str(
                &sikemux_plugin_api::network::unreachable_message(*kind, "Google Cloud"),
            ),
            Self::Response(message) => write!(formatter, "gcp: unexpected response: {message}"),
        }
    }
}

impl From<reqwest::Error> for GcpError {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = sikemux_plugin_api::unreachable(&error) {
            eprintln!("gcp: {error}");
            return Self::Unreachable(kind);
        }
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for GcpError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<GcpError> for PluginError {
    fn from(error: GcpError) -> Self {
        let message = error.to_string();
        match error {
            GcpError::NoGcloud => PluginError::new("unconfigured", message),
            GcpError::Unconfigured => PluginError::new("unconfigured", message),
            GcpError::Auth(_) => PluginError::new("auth", message),
            GcpError::ServiceDisabled { .. } => PluginError::new("service-disabled", message),
            GcpError::NoBilling => PluginError::new("billing-disabled", message),
            GcpError::Permission(_) => PluginError::new("forbidden", message).with_status(403),
            GcpError::NotFound(_) => PluginError::new("not-found", message),
            GcpError::RateLimited => PluginError::new("rate-limited", message).with_status(429),
            GcpError::Api { status, .. } => PluginError::http(status, message),
            GcpError::BadArg(_) => PluginError::new("bad-params", message),
            GcpError::Unreachable(kind) => PluginError::unreachable(kind, "Google Cloud"),
            GcpError::Gcloud(_) | GcpError::Transport(_) => PluginError::new("gcp", message),
            GcpError::Response(_) => PluginError::new("response", message),
        }
    }
}
