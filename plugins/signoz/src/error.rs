use serde_json::Value;
use sikemux_plugin_api::{http, Service, ServiceError};

pub struct Signoz;

impl Service for Signoz {
    const ID: &'static str = "signoz";
    const NAME: &'static str = "SigNoz";
    const SIGNED_OUT: &'static str = "not configured";

    fn said(body: &[u8]) -> Option<String> {
        serde_json::from_slice::<Value>(body)
            .ok()
            .as_ref()
            .and_then(crate::client::error_message)
            .or_else(|| http::quoted(body))
    }
}

pub type SignozError = ServiceError<Signoz>;
pub type SignozResult<T> = Result<T, SignozError>;
