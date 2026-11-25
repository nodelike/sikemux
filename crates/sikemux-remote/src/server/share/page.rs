//! Pages shared through a link. The page is sealed with the link's key and kept by the accounts
//! server, so the link works while this computer sleeps and the server cannot read it.

use std::fmt;
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;

use super::seal::{self, Seal};
use crate::server::account::api::{host_send_any, retry_after, Upload};
use crate::server::remote_state::RemoteParts;
use crate::server::{Core, CoreError, CoreResult};

pub const SIGNED_OUT_PROBLEM: &str = "Sign in to Sikemux to update this shared artifact.";
const TOO_LARGE_PROBLEM: &str = "This artifact is too large to share (limit is 25 MiB).";
const CLOCK_SLACK_MS: u64 = 120_000;

/// Why putting a page behind its link failed, split by whether trying again can succeed.
#[derive(Debug)]
pub enum PutError {
    Transient {
        message: String,
        retry_after: Option<Duration>,
        unreachable: bool,
    },
    SignedOut,
    Ended,
    Refused(String),
}

impl fmt::Display for PutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transient { message, .. } | Self::Refused(message) => f.write_str(message),
            Self::SignedOut => f.write_str(SIGNED_OUT_PROBLEM),
            Self::Ended => f.write_str("That share has ended."),
        }
    }
}

impl From<PutError> for CoreError {
    fn from(error: PutError) -> Self {
        match error {
            PutError::Transient {
                message,
                unreachable: true,
                ..
            } => CoreError {
                unreachable: true,
                ..CoreError::from(message)
            },
            other => CoreError::from(other.to_string()),
        }
    }
}

fn error_message(status: StatusCode, body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                format!("the accounts server answered {status}")
            } else {
                format!("the accounts server answered {status}: {trimmed}")
            }
        })
}

/// A new link's id and key.
pub fn new_link() -> CoreResult<(String, String)> {
    let random = |n| seal::random_text(n).ok_or_else(|| CoreError::from("no randomness"));
    Ok((random(16)?, random(seal::KEY_LEN)?))
}

/// Where a viewer opens the page: the key rides after `#`, so it never reaches the server.
pub fn url(id: &str, key: &str) -> String {
    format!("{}/p/{id}#{key}", crate::accounts::app_base())
}

/// Puts `html` under link `id`, replacing what it showed before.
pub async fn put(
    core: &Core,
    (id, key): (&str, &str),
    title: &str,
    html: &str,
    anyone: bool,
    expires_at: u64,
) -> Result<(), PutError> {
    if core.remote().signed_in_api().is_none() {
        return Err(PutError::SignedOut);
    }
    let seal = Seal::new(id, key)
        .ok_or_else(|| PutError::Refused("the page's link holds no key".into()))?;
    let plain = serde_json::to_vec(&json!({ "title": title, "html": html }))
        .map_err(|error| PutError::Refused(error.to_string()))?;
    let sealed = seal
        .seal(&plain)
        .ok_or_else(|| PutError::Refused("the page could not be sealed".into()))?;
    let upload = Upload {
        bytes: sealed.into_bytes(),
        content_type: "application/octet-stream".into(),
        headers: vec![
            ("x-page-viewer-check", seal.viewer_check().to_owned()),
            ("x-page-anyone", anyone.to_string()),
            ("x-page-expires", expires_at.to_string()),
        ],
    };
    let response =
        match host_send_any(core, Method::PUT, &format!("/v1/pages/{id}"), Some(upload)).await {
            Ok(response) => response,
            Err(error) if error.is_unreachable() => {
                return Err(PutError::Transient {
                    message: error.message,
                    retry_after: None,
                    unreachable: true,
                });
            }
            Err(_) if core.remote().signed_in_api().is_none() => return Err(PutError::SignedOut),
            Err(error) => return Err(PutError::Refused(error.message)),
        };
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let hint = retry_after(&response);
    let body = response.text().await.unwrap_or_default();
    let message = error_message(status, &body);
    match status {
        StatusCode::UNAUTHORIZED => Err(PutError::SignedOut),
        StatusCode::NOT_FOUND | StatusCode::GONE => Err(PutError::Ended),
        StatusCode::PAYLOAD_TOO_LARGE => Err(PutError::Refused(TOO_LARGE_PROBLEM.into())),
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS => Err(PutError::Transient {
            message,
            retry_after: hint,
            unreachable: false,
        }),
        status if status.is_server_error() => Err(PutError::Transient {
            message,
            retry_after: hint,
            unreachable: false,
        }),
        StatusCode::BAD_REQUEST if expires_at <= epoch_ms().saturating_add(CLOCK_SLACK_MS) => {
            Err(PutError::Ended)
        }
        _ => Err(PutError::Refused(message)),
    }
}

/// Ends link `id`. One the server no longer holds has ended already.
pub async fn delete(core: &Core, id: &str) -> CoreResult<()> {
    let answer = host_send_any(core, Method::DELETE, &format!("/v1/pages/{id}"), None).await?;
    match answer.status() {
        status
            if status.is_success()
                || matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE) =>
        {
            Ok(())
        }
        status => Err(format!("the accounts server answered {status}").into()),
    }
}
