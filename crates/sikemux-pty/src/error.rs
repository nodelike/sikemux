use thiserror::Error;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("invalid argument: {0}")]
    BadArg(&'static str),

    #[error("pty: {0}")]
    Pty(String),
}

pub type PtyResult<T> = Result<T, PtyError>;
