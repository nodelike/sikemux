use sikemux_plugin_api::{http, Service, ServiceError};

pub struct Jira;

impl Service for Jira {
    const ID: &'static str = "jira";
    const NAME: &'static str = "Jira";
    const SIGNED_OUT: &'static str = "no site is signed in";

    fn said(body: &[u8]) -> Option<String> {
        serde_json::from_slice(body)
            .ok()
            .as_ref()
            .and_then(crate::client::error_message)
            .or_else(|| http::quoted(body))
    }
}

pub type JiraError = ServiceError<Jira>;
pub type JiraResult<T> = Result<T, JiraError>;
