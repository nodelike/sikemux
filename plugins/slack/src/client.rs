// The HTTP side of Slack's Web API: one warm client, the token each workspace
// uses, and Slack's way of answering `ok: false` with an error code turned
// into ours.

use std::path::Path;
use std::time::Duration;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_plugin_api::http::{self, Deadline, Http, Redirects};
use sikemux_plugin_api::store::{self, Accounts, Held};

use crate::config::{self, Workspace};
use crate::error::{Slack, SlackError, SlackResult};

pub const API: &str = "https://slack.com/api";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// A wait this short is sat out and the call tried again, rather than failing it.
const SHORT_WAIT_SECS: u64 = 3;

static HTTP: Http<Slack> = Http::new(Redirects::Follow, Deadline::WholeAnswer, 8);
static TOKENS: Held<String> = Held::secrets();

pub fn forget(workspace: &str) {
    TOKENS.forget(Some(workspace));
}

pub struct Session {
    pub workspace: Workspace,
    pub token: String,
}

impl Session {
    /// The workspace named, or the default one.
    pub async fn of(data_dir: &Path, workspace: Option<&str>) -> SlackResult<Session> {
        let workspace = config::load(data_dir)?
            .account(workspace)
            .cloned()
            .ok_or(SlackError::Unconfigured)?;
        let token = token_of(&workspace).await?;
        Ok(Session { workspace, token })
    }
}

async fn token_of(workspace: &Workspace) -> SlackResult<String> {
    if let Some(token) = TOKENS.get(&workspace.id) {
        return Ok(token);
    }
    let reading = workspace.id.clone();
    let token = store::blocking(move || Ok(config::KEYCHAIN.read(&reading)?))
        .await?
        .ok_or(SlackError::Unconfigured)?;
    TOKENS.put(&workspace.id, token.clone());
    Ok(token)
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
        "missing_scope" => crate::error::missing_scope(
            body.get("needed")
                .and_then(Value::as_str)
                .unwrap_or("needed"),
        ),
        "ratelimited" => SlackError::RateLimited {
            resets_in_secs: Some(30),
        },
        _ => SlackError::Api(crate::error::in_words(&code)),
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
            Body::Query(query) => HTTP.client()?.get(&address).query(query),
            Body::Json(json) => HTTP.client()?.post(&address).json(json),
        };
        let response = HTTP.send(request.bearer_auth(token)).await?;
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
                resets_in_secs: Some(wait),
            });
        }
        let bytes = http::read_body(response, MAX_RESPONSE_BYTES).await?;
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
        http::collect_pages(String::new(), max_pages, |cursor| async move {
            let mut asked = query.to_vec();
            if !cursor.is_empty() {
                asked.push(("cursor", cursor));
            }
            let page = self.get(method, &asked).await?;
            let items = match page.get(key).cloned() {
                Some(items) => serde_json::from_value::<Vec<T>>(items)?,
                None => Vec::new(),
            };
            let next = page
                .pointer("/response_metadata/next_cursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_string);
            Ok((items, next))
        })
        .await
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
        assert!(
            matches!(checked(json!({ "ok": false, "error": "missing_scope", "needed": "search:read" })), Err(SlackError::Forbidden(message)) if message.contains("search:read"))
        );
        assert!(
            matches!(checked(json!({ "ok": false, "error": "channel_not_found" })), Err(SlackError::Api(message)) if message == "that channel does not exist, or this account cannot see it")
        );
        assert!(matches!(
            checked(json!({ "something": 1 })),
            Err(SlackError::Api(_))
        ));
    }
}
