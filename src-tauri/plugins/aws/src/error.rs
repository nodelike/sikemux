use std::fmt;
use std::io;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum AwsError {
    Aws(String),
    CliMissing(String),
    TokenExpired,
    NoCredentials,
    BadArg(&'static str),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for AwsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aws(message) => write!(formatter, "aws: {message}"),
            Self::CliMissing(bin) => write!(formatter, "aws cli not on PATH: {bin}"),
            Self::TokenExpired => formatter.write_str("aws sso/session token expired"),
            Self::NoCredentials => formatter.write_str("aws: no credentials configured"),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Io(error) => write!(formatter, "io: {error}"),
            Self::Json(error) => write!(formatter, "json: {error}"),
        }
    }
}

impl From<io::Error> for AwsError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for AwsError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<AwsError> for PluginError {
    fn from(error: AwsError) -> Self {
        let category = match &error {
            AwsError::Aws(_) | AwsError::Io(_) | AwsError::Json(_) => "aws",
            AwsError::CliMissing(_) => "aws-cli-missing",
            AwsError::TokenExpired => "aws-token-expired",
            AwsError::NoCredentials => "aws-no-credentials",
            AwsError::BadArg(_) => "bad-params",
        };
        PluginError::new(category, error.to_string())
    }
}

pub type AwsResult<T> = Result<T, AwsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_are_the_ones_the_frontend_reads() {
        let category = |error: AwsError| PluginError::from(error).category;
        assert_eq!(category(AwsError::Aws("x".into())), "aws");
        assert_eq!(
            category(AwsError::CliMissing("aws".into())),
            "aws-cli-missing"
        );
        assert_eq!(category(AwsError::TokenExpired), "aws-token-expired");
        assert_eq!(category(AwsError::NoCredentials), "aws-no-credentials");
        assert_eq!(category(AwsError::BadArg("x")), "bad-params");
    }
}
