use std::fmt;
use std::io;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum BrunoError {
    BadArg(&'static str),
    Http(String),
    Io(io::Error),
}

impl fmt::Display for BrunoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Http(message) => write!(formatter, "http: {message}"),
            Self::Io(error) => write!(formatter, "io: {error}"),
        }
    }
}

impl From<io::Error> for BrunoError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<reqwest::Error> for BrunoError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.to_string())
    }
}

impl From<BrunoError> for PluginError {
    fn from(error: BrunoError) -> Self {
        let category = match &error {
            BrunoError::BadArg(_) => "bad-params",
            BrunoError::Http(_) => "http",
            BrunoError::Io(_) => "io",
        };
        PluginError::new(category, error.to_string())
    }
}

pub type BrunoResult<T> = Result<T, BrunoError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_read_as_they_did_in_core() {
        let wire = |error: BrunoError| {
            let error = PluginError::from(error);
            (error.category, error.message)
        };
        assert_eq!(
            wire(BrunoError::BadArg("invalid URL")),
            ("bad-params".into(), "invalid argument: invalid URL".into())
        );
        assert_eq!(
            wire(BrunoError::Http("response exceeds 32 MiB limit".into())),
            ("http".into(), "http: response exceeds 32 MiB limit".into())
        );
        assert_eq!(
            wire(BrunoError::Io(io::Error::other("denied"))),
            ("io".into(), "io: denied".into())
        );
    }
}
