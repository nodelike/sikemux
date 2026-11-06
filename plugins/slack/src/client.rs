// The HTTP side of Slack's Web API: one warm client, the token each workspace
// uses, and Slack's way of answering `ok: false` with an error code turned
// into ours.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_plugin_api::http::{self, Http, Read, Redirects};
use sikemux_plugin_api::ratelimit::LimitHeaders;
use sikemux_plugin_api::store::{Accounts, Held};

use crate::config::{self, Workspace};
use crate::error::{Slack, SlackError, SlackResult};

pub const API: &str = "https://slack.com/api";

/// Slack says how long to hold off in `retry-after` alone.
static HTTP: Http<Slack> = Http::with_limits(
    Redirects::Follow,
    8,
    LimitHeaders {
        remaining: None,
        reset: None,
    },
);
static TOKENS: Held<String> = Held::secrets();

pub fn forget(workspace: &str) {
    TOKENS.forget(Some(workspace));
}

pub struct Session {
    pub workspace: Workspace,
    pub token: String,
}

impl Session {
    pub async fn of(data_dir: &Path, workspace: Option<&str>) -> SlackResult<Session> {
        let workspace = config::load(data_dir)?
            .account(workspace)
            .cloned()
            .ok_or(SlackError::Unconfigured)?;
        let token = TOKENS
            .secret(config::KEYCHAIN, &workspace.id, workspace.id.clone())
            .await?;
        Ok(Session { workspace, token })
    }
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

async fn send(workspace: &str, token: &str, method: &str, body: Body<'_>) -> SlackResult<Value> {
    let address = format!("{API}/{method}");
    let request = match &body {
        Body::Query(query) => HTTP.client()?.get(&address).query(query),
        Body::Json(json) => HTTP.client()?.post(&address).json(json),
    };
    let answer = HTTP
        .exchange(workspace, request.bearer_auth(token), Read::ALL)
        .await?
        .succeeded()?;
    checked(answer.parsed()?)
}

/// A read with a token that belongs to no workspace yet.
pub async fn get_with(token: &str, method: &str, query: &[(&str, String)]) -> SlackResult<Value> {
    send("", token, method, Body::Query(query)).await
}

impl Session {
    pub async fn get(&self, method: &str, query: &[(&str, String)]) -> SlackResult<Value> {
        let answer = send(&self.workspace.id, &self.token, method, Body::Query(query)).await;
        if matches!(answer, Err(SlackError::Auth(_))) {
            forget(&self.workspace.id);
        }
        answer
    }

    pub async fn post(&self, method: &str, body: &Value) -> SlackResult<Value> {
        let answer = send(&self.workspace.id, &self.token, method, Body::Json(body)).await;
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
