// Vercel's REST API: one warm client, the token signed in here, the team a call
// is for, and Vercel's errors turned into ours.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::{Client, Method, Response};
use serde_json::Value;

use crate::config::{self, Account, SignIn};
use crate::error::{VercelError, VercelResult};

const API: &str = "https://api.vercel.com";
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const TOKEN_TTL: Duration = Duration::from_secs(60);

/// The value at `path` in `value`, or null when there is none.
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    value.pointer(path).unwrap_or(&Value::Null)
}

pub fn text(value: &Value, path: &str) -> String {
    at(value, path).as_str().unwrap_or_default().to_string()
}

pub fn http() -> VercelResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .user_agent("sikemux-vercel/0.1")
                .connect_timeout(Duration::from_secs(4))
                .timeout(Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| VercelError::Transport("could not start the HTTP client".into()))
}

struct HeldToken {
    entry: (String, Option<String>),
    token: String,
    at: Instant,
}

static TOKENS: Mutex<Vec<HeldToken>> = Mutex::new(Vec::new());

pub fn forget() {
    if let Ok(mut held) = TOKENS.lock() {
        held.clear();
    }
}

/// The token kept for the account, or for its install on `team`.
async fn token_of(account: &Account, team: Option<&str>) -> VercelResult<String> {
    let entry = (account.id.clone(), team.map(str::to_string));
    if let Ok(held) = TOKENS.lock() {
        if let Some(held) = held
            .iter()
            .find(|held| held.entry == entry && held.at.elapsed() < TOKEN_TTL)
        {
            return Ok(held.token.clone());
        }
    }
    let reading = account.clone();
    let scope = entry.1.clone();
    let token = config::blocking(Box::new(move || {
        config::keychain_read(&reading, scope.as_deref())
    }))
    .await?
    .ok_or(VercelError::Unconfigured)?;
    if let Ok(mut held) = TOKENS.lock() {
        held.retain(|held| held.entry != entry);
        held.push(HeldToken {
            entry,
            token: token.clone(),
            at: Instant::now(),
        });
    }
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
        let config = config::load(data_dir);
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

pub async fn read_body(response: Response) -> VercelResult<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(VercelError::Response("more than 16 MiB came back".into()));
        }
    }
    Ok(bytes)
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
        429 => VercelError::RateLimited,
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
    let mut builder = http()?
        .request(method, format!("{API}{path}"))
        .bearer_auth(token)
        .query(query);
    if let Some(body) = body {
        builder = builder.json(&body);
    }
    let response = builder.send().await?;
    let status = response.status().as_u16();
    let bytes = read_body(response).await?;
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
            Err(VercelError::RateLimited)
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
