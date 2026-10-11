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
