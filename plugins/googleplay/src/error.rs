use reqwest::StatusCode;
use serde_json::Value;
use sikemux_plugin_api::json::at;
use sikemux_plugin_api::{http, Service, ServiceError};

pub struct GooglePlay;

impl Service for GooglePlay {
    const ID: &'static str = "googleplay";
    const NAME: &'static str = "Google Play";

    fn refused(status: StatusCode, body: &[u8], spent: Option<u64>) -> PlayError {
        refused(status, body, spent)
    }
}

pub type PlayError = ServiceError<GooglePlay>;
pub type PlayResult<T> = Result<T, PlayError>;

const DRAFT_APP: &str = "Only releases with status draft may be created on draft app";

/// Google answers `{"error": {"message": …}}` from the API and
/// `{"error": "…", "error_description": …}` from its token endpoint.
fn refused(status: StatusCode, body: &[u8], spent: Option<u64>) -> PlayError {
    let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let error = at(&body, "/error");
    let message = at(error, "/message")
        .as_str()
        .or_else(|| at(&body, "/error_description").as_str())
        .or_else(|| error.as_str())
        .unwrap_or("Google Play refused the request")
        .to_string();
    if message.contains(DRAFT_APP) {
        return PlayError::BadArg(
            "this app is still a draft in Play Console, so it takes only status draft; call again with it".into(),
        );
    }
    match status.as_u16() {
        400 if at(&body, "/error").as_str() == Some("invalid_grant") => PlayError::Auth(message),
        401 => PlayError::Auth(message),
        403 => PlayError::Forbidden(format!(
            "{message}. Invite the service account in Play Console, under Users and permissions, with release rights for this app"
        )),
        400 | 409 => PlayError::Api(message),
        _ => http::classify(status, Some(message), spent),
    }
}
