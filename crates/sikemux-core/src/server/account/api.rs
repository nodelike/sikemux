//! Calling the accounts API as this host, signed with its key.

use crate::server::parts::Parts;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sikemux_process::clock::epoch_secs;
use sikemux_process::sync::Locked;

use crate::accounts::HostRequest;

use super::super::{Core, CoreError, CoreResult};

/// How long the accounts server remembers a signature, refusing it if it comes again.
const SIGNATURE_MEMORY_SECS: u64 = 60;

pub(crate) async fn as_host<T: DeserializeOwned>(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> CoreResult<T> {
    let answer = host_request(core, method, path, body).await?;
    let body = answer.bytes().await.map_err(|error| {
        CoreError::unreachable(
            &format!("the accounts server's answer to {path} broke off"),
            &error,
        )
    })?;
    serde_json::from_slice(&body).map_err(|error| {
        CoreError::from(format!(
            "the accounts server's answer to {path} is not what this build reads: {error}"
        ))
    })
}

/// The second to sign a request with. The same request signed twice in one second would carry
/// the same signature, which the accounts server refuses as a replay, so a repeat takes the
/// next second instead. A core taking over from another may make the request the old one made
/// in its last second, so its signatures start a second after its first.
fn unused_second(request: String) -> u64 {
    static SIGNED: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    static FIRST_FREE: OnceLock<u64> = OnceLock::new();
    let now = epoch_secs();
    let first_free = *FIRST_FREE.get_or_init(|| now + 1);
    let mut signed = SIGNED.get_or_init(Default::default).locked();
    signed.retain(|_, second| *second + SIGNATURE_MEMORY_SECS > now);
    let second = signed
        .get(&request)
        .map_or(now, |last| now.max(last + 1))
        .max(first_free);
    signed.insert(request, second);
    second
}

/// Calls the accounts API as this host, signed with its key, for an answer
/// with nothing to read.
pub(crate) async fn host_request(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> CoreResult<reqwest::Response> {
    let body = body
        .map(|body| serde_json::to_vec(&body))
        .transpose()?
        .map(|bytes| Upload {
            bytes,
            content_type: "application/json".into(),
            headers: Vec::new(),
        });
    host_send(core, method, path, body).await
}

/// A body sent as it is, such as a file's bytes, with what the server reads about it.
pub(crate) struct Upload {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub headers: Vec<(&'static str, String)>,
}

/// Calls the accounts API as this host, signed with its key over `body`, whose hash goes along
/// so the server can check it before reading the body.
pub(crate) async fn host_send(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Upload>,
) -> CoreResult<reqwest::Response> {
    let response = host_send_any(core, method, path, body).await?;
    if !response.status().is_success() {
        let status = response.status();
        let said = response.text().await.unwrap_or_default();
        return Err(format!("the accounts server answered {status}: {said}").into());
    }
    Ok(response)
}

/// Like [`host_send`], answering with whatever the server answered.
pub(crate) async fn host_send_any(
    core: &Core,
    method: Method,
    path: &str,
    body: Option<Upload>,
) -> CoreResult<reqwest::Response> {
    let base = core
        .remote()
        .signed_in_api()
        .ok_or_else(|| CoreError::from("this host is not signed in"))?;
    let body_sha256 = hex::encode(Sha256::digest(
        body.as_ref()
            .map(|body| body.bytes.as_slice())
            .unwrap_or_default(),
    ));
    let time = unused_second(format!("{method} {path} {body_sha256}"));
    let request = HostRequest {
        method: method.as_str().to_owned(),
        path: path.to_owned(),
        time,
        body_sha256,
    };
    let (key, signature) = core.remote().sign_host_request(&request)?;
    let mut sent = crate::accounts::client()
        .map_err(CoreError::from)?
        .request(method, format!("{}{path}", base.trim_end_matches('/')))
        .header(
            "authorization",
            format!("SikemuxHost {key}.{}.{signature}", request.time),
        );
    if let Some(body) = body {
        sent = sent
            .header("content-type", body.content_type)
            .header("x-body-sha256", &request.body_sha256);
        for (name, value) in body.headers {
            sent = sent.header(name, value);
        }
        sent = sent.body(body.bytes);
    }
    sent.send().await.map_err(|error| {
        CoreError::unreachable(
            &format!("the accounts server did not answer {path}"),
            &error,
        )
    })
}

/// How long a refusal asks to wait before asking again, from its Retry-After.
pub(crate) fn retry_after(response: &reqwest::Response) -> Option<std::time::Duration> {
    let seconds = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(std::time::Duration::from_secs(seconds))
}
