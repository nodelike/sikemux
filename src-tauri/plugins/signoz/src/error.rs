use sikemux_plugin_api::{Service, ServiceError};

pub struct Signoz;

impl Service for Signoz {
    const ID: &'static str = "signoz";
    const NAME: &'static str = "SigNoz";
    const SIGNED_OUT: &'static str = "not configured";
}

pub type SignozError = ServiceError<Signoz>;
pub type SignozResult<T> = Result<T, SignozError>;
