use sikemux_plugin_api::{Service, ServiceError};

pub struct Linear;

impl Service for Linear {
    const ID: &'static str = "linear";
    const NAME: &'static str = "Linear";
    const SIGNED_OUT: &'static str = "no workspace is signed in";
}

pub type LinearError = ServiceError<Linear>;
pub type LinearResult<T> = Result<T, LinearError>;
