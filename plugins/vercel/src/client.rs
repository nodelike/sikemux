// Vercel's REST API: one warm client, the token signed in here, the team a call
// is for, and Vercel's errors turned into ours.

use std::path::Path;

use reqwest::{Method, StatusCode};
use serde_json::Value;
use sikemux_plugin_api::http::{self, Http, Read, Redirects};
use sikemux_plugin_api::store::Held;

use crate::config::{self, Account, SignIn};
use crate::error::{Vercel, VercelError, VercelResult};

const API: &str = "https://api.vercel.com";

pub static HTTP: Http<Vercel> = Http::new(Redirects::Follow, 8);
static TOKENS: Held<String> = Held::secrets();

pub use sikemux_plugin_api::json::{at, number, some_text, text};

pub fn forget() {
    TOKENS.forget(None);
}

/// The token kept for the account, or for its install on `team`.
async fn token_of(account: &Account, team: Option<&str>) -> VercelResult<String> {
    let entry = config::entry(account, team);
    TOKENS.secret(config::KEYCHAIN, &entry, entry.clone()).await
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
                        "Sikemux is not installed on {}; add it from Vercel in Settings, under Connectors",
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

/// Vercel says a call failed with a status and an `error` holding a code and message.
pub fn refused(status: StatusCode, body: &[u8], spent: Option<u64>) -> VercelError {
    let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let error = at(&body, "/error");
    let message = at(error, "/message")
        .as_str()
        .unwrap_or("Vercel refused the request")
        .to_string();
    let invalid_token = at(error, "/invalidToken").as_bool() == Some(true)
        || at(error, "/code").as_str() == Some("invalid_token");
    match status.as_u16() {
        403 if invalid_token => VercelError::Auth(message),
        400 | 409 | 422 => VercelError::Api(message),
        _ => http::classify(status, Some(message), spent),
    }
}

/// Calls Vercel with `token`, answering with the JSON it sent, or null for an empty answer.
pub async fn request(
    token: &str,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<Value>,
) -> VercelResult<Value> {
    let mut builder = HTTP
        .client()?
        .request(method, format!("{API}{path}"))
        .bearer_auth(token)
        .query(query);
    if let Some(body) = body {
        builder = builder.json(&body);
    }
    HTTP.ask(builder, Read::ALL).await?.succeeded()?.parsed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vercels_errors_read_in_words() {
        let refusal = |status: u16, body: Value| {
            refused(
                StatusCode::from_u16(status).expect("a status"),
                body.to_string().as_bytes(),
                None,
            )
        };
        let bad_token = json!({ "error": { "code": "forbidden", "message": "Not authorized", "invalidToken": true } });
        assert!(matches!(refusal(403, bad_token), VercelError::Auth(_)));
        let forbidden =
            json!({ "error": { "code": "forbidden", "message": "You don't have permission" } });
        assert!(
            matches!(refusal(403, forbidden), VercelError::Forbidden(message) if message.contains("permission"))
        );
        let missing = json!({ "error": { "code": "not_found", "message": "Project not found" } });
        assert!(
            matches!(refusal(404, missing), VercelError::NotFound(message) if message == "Project not found")
        );
        assert!(matches!(
            refusal(429, json!({})),
            VercelError::RateLimited {
                resets_in_secs: None
            }
        ));
        assert!(matches!(
            refusal(502, json!({})),
            VercelError::Http { status: 502, .. }
        ));
    }

    #[test]
    fn a_team_is_named_by_id_or_by_slug() {
        assert_eq!(team_param("team_abc"), ("teamId", "team_abc".into()));
        assert_eq!(team_param("acme"), ("slug", "acme".into()));
    }
}
