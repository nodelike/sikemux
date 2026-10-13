// The HTTP side of Slack's Web API: one warm client, the token each workspace
// uses, and Slack's way of answering `ok: false` with an error code turned
// into ours.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::config::{self, Workspace};
use crate::error::{SlackError, SlackResult};

pub const API: &str = "https://slack.com/api";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const TOKEN_TTL: Duration = Duration::from_secs(60);
/// A wait this short is sat out and the call tried again, rather than failing it.
const SHORT_WAIT_SECS: u64 = 3;

pub fn http() -> SlackResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .user_agent("sikemux-slack/0.1")
                .connect_timeout(Duration::from_secs(4))
                .timeout(Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| SlackError::Transport("could not start the HTTP client".into()))
}

struct HeldToken {
    token: String,
    at: Instant,
}

static TOKENS: Mutex<BTreeMap<String, HeldToken>> = Mutex::new(BTreeMap::new());

pub fn forget(workspace: &str) {
    if let Ok(mut held) = TOKENS.lock() {
        held.remove(workspace);
    }
}

pub struct Session {
    pub workspace: Workspace,
    pub token: String,
}

impl Session {
    /// The workspace named, or the default one.
    pub async fn of(data_dir: &Path, workspace: Option<&str>) -> SlackResult<Session> {
        let workspace = config::load(data_dir)
            .workspace(workspace)
            .cloned()
            .ok_or(SlackError::Unconfigured)?;
        let token = token_of(&workspace).await?;
        Ok(Session { workspace, token })
    }
}

async fn token_of(workspace: &Workspace) -> SlackResult<String> {
    if let Ok(held) = TOKENS.lock() {
        if let Some(held) = held
            .get(&workspace.id)
            .filter(|held| held.at.elapsed() < TOKEN_TTL)
        {
            return Ok(held.token.clone());
        }
    }
    let reading = workspace.clone();
    let token = config::blocking(Box::new(move || config::keychain_read(&reading)))
        .await?
        .ok_or(SlackError::Unconfigured)?;
    if let Ok(mut held) = TOKENS.lock() {
        held.insert(
            workspace.id.clone(),
            HeldToken {
                token: token.clone(),
                at: Instant::now(),
            },
        );
    }
    Ok(token)
}

async fn read_body(response: Response) -> SlackResult<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(SlackError::Response("more than 8 MiB came back".into()));
        }
    }
    Ok(bytes)
}

/// Slack says a call failed in the body: `ok: false` and an error code.
pub fn checked(body: Value) -> SlackResult<Value> {
    if body.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(body);
    }
    let code = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown_error")
        .to_string();
    Err(match code.as_str() {
        "not_authed" | "invalid_auth" | "token_revoked" | "token_expired" | "account_inactive" => {
            SlackError::Auth(code.replace('_', " "))
        }
        "missing_scope" => SlackError::MissingScope(
            body.get("needed")
                .and_then(Value::as_str)
                .unwrap_or("needed")
                .to_string(),
        ),
        "ratelimited" => SlackError::RateLimited {
            retry_after_secs: 30,
        },
        _ => SlackError::Api(code),
    })
}

enum Body<'a> {
    Query(&'a [(&'a str, String)]),
    Json(&'a Value),
}

async fn send(token: &str, method: &str, body: Body<'_>) -> SlackResult<Value> {
    let mut waited = false;
    loop {
        let address = format!("{API}/{method}");
        let request = match &body {
            Body::Query(query) => http()?.get(&address).query(query),
            Body::Json(json) => http()?.post(&address).json(json),
        };
        let response = request.bearer_auth(token).send().await?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .unwrap_or(30);
            if wait <= SHORT_WAIT_SECS && !waited {
                waited = true;
                tokio::time::sleep(Duration::from_secs(wait.max(1))).await;
                continue;
            }
            return Err(SlackError::RateLimited {
                retry_after_secs: wait,
            });
        }
        let bytes = read_body(response).await?;
        if !status.is_success() {
            return Err(SlackError::Http {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&bytes).chars().take(200).collect(),
            });
        }
        return checked(serde_json::from_slice(&bytes)?);
    }
}

/// A read, with its arguments in the address.
pub async fn get_with(token: &str, method: &str, query: &[(&str, String)]) -> SlackResult<Value> {
    send(token, method, Body::Query(query)).await
}

/// A write, with its arguments as JSON.
pub async fn post_with(token: &str, method: &str, body: &Value) -> SlackResult<Value> {
    send(token, method, Body::Json(body)).await
}

impl Session {
    pub async fn get(&self, method: &str, query: &[(&str, String)]) -> SlackResult<Value> {
        let answer = get_with(&self.token, method, query).await;
        if matches!(answer, Err(SlackError::Auth(_))) {
            forget(&self.workspace.id);
        }
        answer
    }

    pub async fn post(&self, method: &str, body: &Value) -> SlackResult<Value> {
        let answer = post_with(&self.token, method, body).await;
        if matches!(answer, Err(SlackError::Auth(_))) {
            forget(&self.workspace.id);
        }
        answer
    }

    /// Every page of a list Slack pages by cursor, up to `max_pages`.
    pub async fn get_all<T: DeserializeOwned>(
        &self,
        method: &str,
        key: &str,
        query: &[(&str, String)],
        max_pages: u32,
    ) -> SlackResult<Vec<T>> {
        let mut all = Vec::new();
        let mut cursor = String::new();
        for _ in 0..max_pages.max(1) {
            let mut asked = query.to_vec();
            if !cursor.is_empty() {
                asked.push(("cursor", cursor.clone()));
            }
            let page = self.get(method, &asked).await?;
            if let Some(items) = page.get(key).cloned() {
                all.extend(serde_json::from_value::<Vec<T>>(items)?);
            }
            cursor = page
                .pointer("/response_metadata/next_cursor")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if cursor.is_empty() {
                break;
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_answer_that_is_not_ok_reads_as_the_error_it_names() {
        assert!(checked(json!({ "ok": true, "channels": [] })).is_ok());
        assert!(matches!(
            checked(json!({ "ok": false, "error": "invalid_auth" })),
            Err(SlackError::Auth(_))
        ));
        match checked(json!({ "ok": false, "error": "missing_scope", "needed": "search:read" })) {
            Err(SlackError::MissingScope(scope)) => assert_eq!(scope, "search:read"),
            other => panic!(
                "expected a missing scope, got {}",
                other
                    .map(|_| "ok".to_string())
                    .unwrap_or_else(|e| e.to_string())
            ),
        }
        assert!(
            matches!(checked(json!({ "ok": false, "error": "channel_not_found" })), Err(SlackError::Api(code)) if code == "channel_not_found")
        );
        assert!(matches!(
            checked(json!({ "something": 1 })),
            Err(SlackError::Api(_))
        ));
    }
}
