use sikemux_plugin_api::{Service, ServiceError};

pub struct Vercel;

impl Service for Vercel {
    const ID: &'static str = "vercel";
    const NAME: &'static str = "Vercel";
}

pub type VercelError = ServiceError<Vercel>;
pub type VercelResult<T> = Result<T, VercelError>;
