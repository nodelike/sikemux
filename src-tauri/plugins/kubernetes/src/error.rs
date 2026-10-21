use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum KubeError {
    /// kubectl isn't installed, or isn't on the person's PATH.
    CliMissing,
    /// helm isn't installed, which only the Helm page and tool need.
    HelmMissing,
    /// The kubeconfig names no context, or none was found.
    NoConfig(String),
    /// The cluster didn't answer: it's stopped, or the network can't reach it.
    Unreachable(String),
    /// The sign-in the context uses was refused or has expired.
    Auth(String),
    Forbidden(String),
    NotFound(String),
    BadArg(String),
    Kubectl(String),
    Response(String),
}

pub type KubeResult<T> = Result<T, KubeError>;

impl fmt::Display for KubeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CliMissing => formatter.write_str(
                "kubernetes: kubectl isn't installed. Install it (brew install kubernetes-cli), then try again.",
            ),
            Self::HelmMissing => formatter.write_str(
                "kubernetes: helm isn't installed. Install it (brew install helm) to see Helm releases.",
            ),
            Self::NoConfig(message)
            | Self::Unreachable(message)
            | Self::Auth(message)
            | Self::Forbidden(message)
            | Self::NotFound(message)
            | Self::Kubectl(message) => write!(formatter, "kubernetes: {message}"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Response(message) => write!(formatter, "kubernetes: unexpected answer: {message}"),
        }
    }
}

impl From<serde_json::Error> for KubeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl From<KubeError> for PluginError {
    fn from(error: KubeError) -> Self {
        let message = error.to_string();
        let category = match error {
            KubeError::CliMissing => "cli-missing",
            KubeError::HelmMissing => "helm-missing",
            KubeError::NoConfig(_) => "unconfigured",
            KubeError::Unreachable(_) => "unreachable",
            KubeError::Auth(_) => "auth",
            KubeError::Forbidden(_) => "forbidden",
            KubeError::NotFound(_) => "not-found",
            KubeError::BadArg(_) => "bad-params",
            KubeError::Kubectl(_) => "kubernetes",
            KubeError::Response(_) => "response",
        };
        PluginError::new(category, message)
    }
}
