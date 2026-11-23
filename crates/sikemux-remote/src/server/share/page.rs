//! Pages shared through a link. The page is sealed with the link's key and kept by the accounts
//! server, so the link works while this computer sleeps and the server cannot read it.

use reqwest::{Method, StatusCode};
use serde_json::json;

use super::seal::{self, Seal};
use crate::server::account::api::{host_send, host_send_any, Upload};
use crate::server::{Core, CoreError, CoreResult};

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
) -> CoreResult<()> {
    let seal = Seal::new(id, key).ok_or("the page's link holds no key")?;
    let plain = serde_json::to_vec(&json!({ "title": title, "html": html }))?;
    let sealed = seal.seal(&plain).ok_or("the page could not be sealed")?;
    let upload = Upload {
        bytes: sealed.into_bytes(),
        content_type: "application/octet-stream".into(),
        headers: vec![
            ("x-page-viewer-check", seal.viewer_check().to_owned()),
            ("x-page-anyone", anyone.to_string()),
            ("x-page-expires", expires_at.to_string()),
        ],
    };
    host_send(core, Method::PUT, &format!("/v1/pages/{id}"), Some(upload)).await?;
    Ok(())
}

/// Ends link `id`. One the server no longer holds has ended already.
pub async fn delete(core: &Core, id: &str) -> CoreResult<()> {
    let answer = host_send_any(core, Method::DELETE, &format!("/v1/pages/{id}"), None).await?;
    match answer.status() {
        status if status.is_success() || status == StatusCode::NOT_FOUND => Ok(()),
        status => Err(format!("the accounts server answered {status}").into()),
    }
}
