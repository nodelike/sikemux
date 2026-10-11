use std::fmt;

use sikemux_plugin_api::PluginError;

#[derive(Debug)]
pub enum DatabaseError {
    BadArg(String),
    NotFound(String),
    Keychain(String),
    /// The database could not be reached, or refused the sign-in.
    Connect(String),
    /// The database ran the statement and answered with an error.
    Query(String),
    Storage(String),
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::NotFound(message) | Self::Connect(message) | Self::Query(message) => {
                formatter.write_str(message)
            }
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Storage(message) => write!(formatter, "saving connections: {message}"),
        }
    }
}

impl From<serde_json::Error> for DatabaseError {
    fn from(error: serde_json::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<std::io::Error> for DatabaseError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<DatabaseError> for PluginError {
    fn from(error: DatabaseError) -> Self {
        let category = match &error {
            DatabaseError::BadArg(_) => "bad-params",
            DatabaseError::NotFound(_) => "not-found",
            DatabaseError::Keychain(_) => "keychain",
            DatabaseError::Connect(_) => "connect",
            DatabaseError::Query(_) => "query",
            DatabaseError::Storage(_) => "storage",
        };
        PluginError::new(category, error.to_string())
    }
}

pub type DatabaseResult<T> = Result<T, DatabaseError>;
