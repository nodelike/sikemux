// Linear's GraphQL API: one warm client, the key or token each workspace uses,
// and Linear's errors turned into ours.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::{Client, Response};
use serde_json::{json, Value};

use crate::config::{self, SignIn, Workspace};
use crate::error::{LinearError, LinearResult};
use crate::oauth;

const API: &str = "https://api.linear.app/graphql";

/// The value at `path` in `value`, or null when there is none.
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    value.pointer(path).unwrap_or(&Value::Null)
}
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const KEY_TTL: Duration = Duration::from_secs(60);

pub fn http() -> LinearResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .user_agent("sikemux-linear/0.1")
                .connect_timeout(Duration::from_secs(4))
                .timeout(Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| LinearError::Transport("could not start the HTTP client".into()))
}

struct HeldKey {
    key: String,
    at: Instant,
}

static KEYS: Mutex<BTreeMap<String, HeldKey>> = Mutex::new(BTreeMap::new());

pub fn forget(workspace: &str) {
    if let Ok(mut held) = KEYS.lock() {
        held.remove(workspace);
    }
}

pub struct Session {
    pub workspace: Workspace,
    /// An API key, or a browser sign-in's access token.
    credential: String,
}

impl Session {
    /// The workspace named, or the default one.
    pub async fn of(data_dir: &Path, workspace: Option<&str>) -> LinearResult<Session> {
        let workspace = config::load(data_dir)
            .workspace(workspace)
            .cloned()
            .ok_or(LinearError::Unconfigured)?;
        let credential = match workspace.sign_in {
            SignIn::Key => key_of(&workspace).await?,
            SignIn::Browser => oauth::access_token(&workspace).await?,
        };
        Ok(Session {
            workspace,
            credential,
        })
    }

    pub async fn query(&self, document: &str, variables: Value) -> LinearResult<Value> {
        match self.workspace.sign_in {
            SignIn::Key => query_with(&self.credential, document, variables).await,
            SignIn::Browser => {
                match query_with(
                    &oauth::bearer(&self.credential),
                    document,
                    variables.clone(),
                )
                .await
                {
                    Err(LinearError::Auth(_)) => {
                        let renewed = oauth::renew(&self.workspace, &self.credential).await?;
                        query_with(&oauth::bearer(&renewed), document, variables).await
                    }
                    answered => answered,
                }
            }
        }
    }
}

async fn key_of(workspace: &Workspace) -> LinearResult<String> {
    if let Ok(held) = KEYS.lock() {
        if let Some(held) = held
            .get(&workspace.id)
            .filter(|held| held.at.elapsed() < KEY_TTL)
        {
            return Ok(held.key.clone());
        }
    }
    let reading = workspace.clone();
    let key = config::blocking(Box::new(move || config::keychain_read(&reading)))
        .await?
        .ok_or(LinearError::Unconfigured)?;
    if let Ok(mut held) = KEYS.lock() {
        held.insert(
            workspace.id.clone(),
            HeldKey {
                key: key.clone(),
                at: Instant::now(),
            },
        );
    }
    Ok(key)
}

pub async fn read_body(response: Response) -> LinearResult<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(LinearError::Response("more than 8 MiB came back".into()));
        }
    }
    Ok(bytes)
}

/// Linear says a query failed in the body's `errors`, often with a 400.
pub fn checked(status: u16, body: Value) -> LinearResult<Value> {
    if let Some(error) = at(&body, "/errors")
        .as_array()
        .and_then(|errors| errors.first())
    {
        let code = at(error, "/extensions/code").as_str().unwrap_or_default();
        let message = at(error, "/extensions/userPresentableMessage")
            .as_str()
            .or_else(|| at(error, "/message").as_str())
            .unwrap_or("Linear refused the request")
            .to_string();
        return Err(match code {
            "AUTHENTICATION_ERROR" => LinearError::Auth(message),
            "RATELIMITED" => LinearError::RateLimited,
            _ if message.to_ascii_lowercase().contains("entity not found") => {
                LinearError::NotFound("linear: no such issue, or this key cannot see it".into())
            }
            _ => LinearError::Api(message),
        });
    }
    if !(200..300).contains(&status) {
        return Err(LinearError::Http {
            status,
            message: body.to_string().chars().take(200).collect(),
        });
    }
    match body.get("data") {
        Some(data) if !data.is_null() => Ok(data.clone()),
        _ => Err(LinearError::Response("Linear sent no data".into())),
    }
}

/// Runs `document` with `authorization`, an API key or a bearer token, answering with its `data`.
pub async fn query_with(
    authorization: &str,
    document: &str,
    variables: Value,
) -> LinearResult<Value> {
    let response = http()?
        .post(API)
        .header("Authorization", authorization)
        .json(&json!({ "query": document, "variables": variables }))
        .send()
        .await?;
    let status = response.status().as_u16();
    let bytes = read_body(response).await?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| LinearError::Http {
        status,
        message: String::from_utf8_lossy(&bytes).chars().take(200).collect(),
    })?;
    checked(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linears_errors_read_in_words() {
        let auth = json!({ "errors": [{ "message": "Authentication required, not authenticated", "extensions": { "code": "AUTHENTICATION_ERROR", "userPresentableMessage": "You need to authenticate to access this operation." } }] });
        assert!(
            matches!(checked(400, auth), Err(LinearError::Auth(message)) if message.contains("authenticate"))
        );
        let limited = json!({ "errors": [{ "message": "Rate limit exceeded", "extensions": { "code": "RATELIMITED" } }] });
        assert!(matches!(
            checked(400, limited),
            Err(LinearError::RateLimited)
        ));
        let missing = json!({ "errors": [{ "message": "Entity not found: Issue", "extensions": { "code": "INVALID_INPUT" } }] });
        assert!(matches!(
            checked(400, missing),
            Err(LinearError::NotFound(_))
        ));
        assert_eq!(
            checked(200, json!({ "data": { "viewer": { "id": "u1" } } })).ok(),
            Some(json!({ "viewer": { "id": "u1" } }))
        );
        assert!(matches!(
            checked(502, json!({})),
            Err(LinearError::Http { status: 502, .. })
        ));
    }

    #[tokio::test]
    async fn a_request_without_a_key_is_refused_as_a_sign_in() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        match query_with("not-a-key", "query { viewer { id } }", json!({})).await {
            Err(LinearError::Auth(_)) | Err(LinearError::Unreachable(_)) => {}
            other => panic!("{other:?}"),
        }
    }
}
