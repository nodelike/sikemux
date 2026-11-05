// Linear's GraphQL API: one warm client, the key or token each workspace uses,
// and Linear's errors turned into ours.

use std::path::Path;

use reqwest::StatusCode;
use serde_json::{json, Value};
use sikemux_plugin_api::http::{Http, Read, Redirects};
use sikemux_plugin_api::store::{self, Accounts, Held};
use sikemux_plugin_api::Service;

use crate::config::{self, SignIn, Workspace};
use crate::error::{Linear, LinearError, LinearResult};
use crate::oauth;

const API: &str = "https://api.linear.app/graphql";

/// The value at `path` in `value`, or null when there is none.
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    value.pointer(path).unwrap_or(&Value::Null)
}

pub static HTTP: Http<Linear> = Http::new(Redirects::Follow, 8);
static KEYS: Held<String> = Held::secrets();

pub fn forget(workspace: &str) {
    KEYS.forget(Some(workspace));
}

pub struct Session {
    pub workspace: Workspace,
    /// The API key, for a workspace not signed in through the browser.
    key: Option<String>,
}

impl Session {
    /// The workspace named, or the default one.
    pub async fn of(data_dir: &Path, workspace: Option<&str>) -> LinearResult<Session> {
        let workspace = config::load(data_dir)?
            .account(workspace)
            .cloned()
            .ok_or(LinearError::Unconfigured)?;
        let key = match workspace.sign_in {
            SignIn::Key => Some(key_of(&workspace).await?),
            SignIn::Browser => None,
        };
        Ok(Session { workspace, key })
    }

    pub async fn query(&self, document: &str, variables: Value) -> LinearResult<Value> {
        match &self.key {
            Some(key) => query_with(key, document, variables).await,
            None => {
                oauth::ACCESS
                    .with_access(
                        &self.workspace.id,
                        || oauth::entry(&self.workspace),
                        |access| {
                            let variables = variables.clone();
                            async move {
                                query_with(&oauth::bearer(&access.token), document, variables).await
                            }
                        },
                    )
                    .await
            }
        }
    }
}

async fn key_of(workspace: &Workspace) -> LinearResult<String> {
    if let Some(key) = KEYS.get(&workspace.id) {
        return Ok(key);
    }
    let reading = workspace.id.clone();
    let key = store::blocking(move || Ok(config::KEYCHAIN.read(&reading)?))
        .await?
        .ok_or(LinearError::Unconfigured)?;
    KEYS.put(&workspace.id, key.clone());
    Ok(key)
}

/// Linear says a query failed in the body's `errors`, often with a 400.
pub fn checked(status: StatusCode, bytes: &[u8]) -> LinearResult<Value> {
    let Ok(body) = serde_json::from_slice::<Value>(bytes) else {
        return Err(Linear::refused(status, bytes, None));
    };
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
            "RATELIMITED" => LinearError::RateLimited {
                resets_in_secs: None,
            },
            _ if message.to_ascii_lowercase().contains("entity not found") => {
                LinearError::NotFound("linear: no such issue, or this key cannot see it".into())
            }
            _ => LinearError::Api(message),
        });
    }
    if !status.is_success() {
        return Err(Linear::refused(status, bytes, None));
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
    let request = HTTP
        .client()?
        .post(API)
        .header("Authorization", authorization)
        .json(&json!({ "query": document, "variables": variables }));
    let answer = HTTP.ask(request, Read::ALL).await?;
    checked(answer.status, &answer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linears_errors_read_in_words() {
        let checked = |status: u16, body: Value| {
            super::checked(
                StatusCode::from_u16(status).expect("a status"),
                body.to_string().as_bytes(),
            )
        };
        let auth = json!({ "errors": [{ "message": "Authentication required, not authenticated", "extensions": { "code": "AUTHENTICATION_ERROR", "userPresentableMessage": "You need to authenticate to access this operation." } }] });
        assert!(
            matches!(checked(400, auth), Err(LinearError::Auth(message)) if message.contains("authenticate"))
        );
        let limited = json!({ "errors": [{ "message": "Rate limit exceeded", "extensions": { "code": "RATELIMITED" } }] });
        assert!(matches!(
            checked(400, limited),
            Err(LinearError::RateLimited {
                resets_in_secs: None
            })
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
}
