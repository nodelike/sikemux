use reqwest::StatusCode;
use sikemux_plugin_api::{Service, ServiceError};

pub struct Vercel;

impl Service for Vercel {
    const ID: &'static str = "vercel";
    const NAME: &'static str = "Vercel";

    fn refused(status: StatusCode, body: &[u8], spent: Option<u64>) -> VercelError {
        crate::client::refused(status, body, spent)
    }
}

pub type VercelError = ServiceError<Vercel>;
pub type VercelResult<T> = Result<T, VercelError>;
