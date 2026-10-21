// Vercel's REST API: one warm client, the token signed in here, the team a call
// is for, and Vercel's errors turned into ours.

use std::path::Path;

use reqwest::Method;
use serde_json::Value;
use sikemux_plugin_api::http::{self, Deadline, Http, Redirects};
use sikemux_plugin_api::store::{self, Held};

use crate::config::{self, Account, SignIn};
use crate::error::{Vercel, VercelError, VercelResult};

const API: &str = "https://api.vercel.com";
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub static HTTP: Http<Vercel> = Http::new(Redirects::Follow, Deadline::WholeAnswer, 8);
/// For answers that keep coming, such as runtime logs, so only a long silence ends them.
static STREAMING: Http<Vercel> = Http::new(Redirects::Follow, Deadline::QuietTransfer, 4);
static TOKENS: Held<String> = Held::secrets();

/// The value at `path` in `value`, or null when there is none.
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    value.pointer(path).unwrap_or(&Value::Null)
}

pub fn text(value: &Value, path: &str) -> String {
    at(value, path).as_str().unwrap_or_default().to_string()
}

pub fn forget() {
    TOKENS.forget(None);
}

/// The token kept for the account, or for its install on `team`.
async fn token_of(account: &Account, team: Option<&str>) -> VercelResult<String> {
    let entry = config::entry(account, team);
    if let Some(token) = TOKENS.get(&entry) {
        return Ok(token);
    }
    let reading = account.clone();
    let scope = team.map(str::to_string);
    let token = store::blocking(move || config::keychain_read(&reading, scope.as_deref()))
        .await?
        .ok_or(VercelError::Unconfigured)?;
    TOKENS.put(&entry, token.clone());
    Ok(token)
}

/// The query parameter that points a call at a team: its id, or its slug.
pub fn team_param(team: &str) -> (&'static str, String) {
    if team.starts_with("team_") {
        ("teamId", team.to_string())
    } else {
        ("slug", team.to_string())
    }
}

pub struct Session {
    token: String,
    /// The team the call is for; none for the account's own projects.
    pub team: Option<String>,
}

impl Session {
    /// The account signed in, for the team named, or the one chosen in Sikemux.
    pub async fn of(data_dir: &Path, team: Option<&str>) -> VercelResult<Session> {
        let config = config::load(data_dir)?;
        let account = config.account.clone().ok_or(VercelError::Unconfigured)?;
        let team = match team.map(str::trim) {
            Some("") | Some("personal") => None,
            Some(team) => Some(team.to_string()),
            None => config.team.clone(),
        };
        match account.sign_in {
            SignIn::Token => Ok(Session {
                token: token_of(&account, None).await?,
                team,
            }),
            SignIn::Browser => {
                let install = config.install(team.as_deref()).ok_or_else(|| {
                    VercelError::NotFound(format!(
                        "Sikemux is not installed on {}; add it from the team menu in the Vercel pane",
                        team.as_deref().unwrap_or("the personal account")
                    ))
                })?;
                Ok(Session {
                    token: token_of(&account, install.team.as_deref()).await?,
                    team: install.team.clone(),
                })
            }
        }
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> VercelResult<Value> {
        self.send(Method::GET, path, query, None).await
    }

    /// A POST with one extra header, for the APIs that ask why they are being called.
    pub async fn post_with_header(
        &self,
        path: &str,
        query: &[(&str, String)],
        header: (&str, &str),
        body: Value,
    ) -> VercelResult<Value> {
        let builder = HTTP
            .client()?
            .post(format!("{API}{path}"))
            .bearer_auth(&self.token)
            .query(query)
            .header(header.0, header.1)
            .json(&body);
        let response = HTTP.send(builder).await?;
        let status = response.status().as_u16();
        let bytes = http::read_body(response, MAX_RESPONSE_BYTES).await?;
        let body = if bytes.iter().all(u8::is_ascii_whitespace) {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| VercelError::Http {
                status,
                message: String::from_utf8_lossy(&bytes).chars().take(200).collect(),
            })?
        };
        checked(status, body)
    }

    pub async fn send_unchecked(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> VercelResult<(u16, Value)> {
        let query: Vec<(&str, String)> = self.team.iter().map(|team| team_param(team)).collect();
        request_unchecked(&self.token, method, path, &query, body).await
    }

    /// Sends a file's bytes for a deployment, named by their SHA-1.
    pub async fn upload(&self, sha: &str, bytes: Vec<u8>) -> VercelResult<()> {
        let query: Vec<(&str, String)> = self.team.iter().map(|team| team_param(team)).collect();
        let builder = STREAMING
            .client()?
            .post(format!("{API}/v2/files"))
            .bearer_auth(&self.token)
            .query(&query)
            .header("Content-Type", "application/octet-stream")
            .header("x-vercel-digest", sha)
            .body(bytes);
        let response = STREAMING.send(builder).await?;
        let status = response.status().as_u16();
        let bytes = http::read_body(response, MAX_RESPONSE_BYTES).await?;
        checked(
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
        .map(drop)
    }

    pub async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> VercelResult<Value> {
        let mut query = query.to_vec();
        if let Some(team) = &self.team {
            query.push(team_param(team));
        }
        request(&self.token, method, path, &query, body).await
    }
}

/// Vercel says a call failed with a status and an `error` holding a code and message.
pub fn checked(status: u16, body: Value) -> VercelResult<Value> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let error = at(&body, "/error");
    let message = at(error, "/message")
        .as_str()
        .unwrap_or("Vercel refused the request")
        .to_string();
    let invalid_token = at(error, "/invalidToken").as_bool() == Some(true)
        || at(error, "/code").as_str() == Some("invalid_token");
    Err(match status {
        401 => VercelError::Auth(message),
        403 if invalid_token => VercelError::Auth(message),
        403 => VercelError::Api(format!("not allowed: {message}")),
        404 => VercelError::NotFound(message),
        429 => VercelError::RateLimited {
            resets_in_secs: None,
        },
        400 | 409 | 422 => VercelError::Api(message),
        _ => VercelError::Http {
            status,
            message: message.chars().take(200).collect(),
        },
    })
}

/// Calls Vercel with `token`, answering with the JSON it sent, or null for an empty answer.
pub async fn request(
    token: &str,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<Value>,
) -> VercelResult<Value> {
    let (status, body) = request_unchecked(token, method, path, query, body).await?;
    checked(status, body)
}

/// Like `request`, but answers with Vercel's status and JSON even when it refused,
/// for the few calls whose refusal carries what to do next.
pub async fn request_unchecked(
    token: &str,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<Value>,
) -> VercelResult<(u16, Value)> {
    let mut builder = HTTP
        .client()?
        .request(method, format!("{API}{path}"))
        .bearer_auth(token)
        .query(query);
    if let Some(body) = body {
        builder = builder.json(&body);
    }
    let response = HTTP.send(builder).await?;
    let status = response.status().as_u16();
    let bytes = http::read_body(response, MAX_RESPONSE_BYTES).await?;
    let body = if bytes.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).map_err(|_| VercelError::Http {
            status,
            message: String::from_utf8_lossy(&bytes).chars().take(200).collect(),
        })?
    };
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vercels_errors_read_in_words() {
        let bad_token = json!({ "error": { "code": "forbidden", "message": "Not authorized", "invalidToken": true } });
        assert!(matches!(checked(403, bad_token), Err(VercelError::Auth(_))));
        let forbidden =
            json!({ "error": { "code": "forbidden", "message": "You don't have permission" } });
        assert!(
            matches!(checked(403, forbidden), Err(VercelError::Api(message)) if message.contains("permission"))
        );
        let missing = json!({ "error": { "code": "not_found", "message": "Project not found" } });
        assert!(
            matches!(checked(404, missing), Err(VercelError::NotFound(message)) if message == "Project not found")
        );
        assert!(matches!(
            checked(429, json!({})),
            Err(VercelError::RateLimited {
                resets_in_secs: None
            })
        ));
        assert!(matches!(
            checked(502, json!({})),
            Err(VercelError::Http { status: 502, .. })
        ));
        assert_eq!(
            checked(200, json!({ "id": 1 })).ok(),
            Some(json!({ "id": 1 }))
        );
    }

    #[test]
    fn a_team_is_named_by_id_or_by_slug() {
        assert_eq!(team_param("team_abc"), ("teamId", "team_abc".into()));
        assert_eq!(team_param("acme"), ("slug", "acme".into()));
    }
}
