use std::sync::OnceLock;
use std::time::Duration;

use futures::StreamExt;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;

use crate::config::Site;
use crate::error::{JiraError, JiraResult};
use crate::oauth;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Where a request goes and who it is from: the site itself with an email and its API
/// token, or Atlassian's API for the site with a browser sign-in's access token.
pub struct Credentials {
    pub url: String,
    pub auth: Auth,
}

pub enum Auth {
    Basic {
        email: String,
        token: String,
    },
    Bearer(String),
    /// A kept browser sign-in, whose access token is renewed once if Jira stops taking it.
    Atlassian {
        site: Site,
        token: String,
    },
}

pub fn http() -> JiraResult<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .pool_idle_timeout(Duration::from_secs(25))
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("sikemux-jira/0.1")
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| JiraError::Transport("could not start the HTTP client".into()))
}

pub async fn read_limited(response: Response) -> JiraResult<Vec<u8>> {
    let too_big = || JiraError::Response("more than 16 MiB came back; narrow the query".into());
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(too_big());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(too_big());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Jira's error body: `errorMessages` for the request and `errors` per field.
pub fn error_message(body: &Value) -> Option<String> {
    let mut parts: Vec<String> = body
        .get("errorMessages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Some(fields) = body.get("errors").and_then(Value::as_object) {
        parts.extend(fields.iter().filter_map(|(field, message)| {
            message
                .as_str()
                .map(|message| format!("{field}: {message}"))
        }));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// One request to `path` on the site, e.g. `/rest/api/3/myself`. An empty answer, as a
/// transition or an assignment gives, comes back as `null`.
pub async fn send(
    credentials: &Credentials,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> JiraResult<Value> {
    let (mut status, mut bytes) = send_once(credentials, None, &method, path, query, body).await?;
    if status == StatusCode::UNAUTHORIZED {
        if let Auth::Atlassian { site, token } = &credentials.auth {
            let renewed = oauth::renew(site, token).await?;
            (status, bytes) =
                send_once(credentials, Some(&renewed), &method, path, query, body).await?;
        }
    }
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    if status.is_success() {
        return Ok(parsed);
    }
    let message = error_message(&parsed)
        .unwrap_or_else(|| String::from_utf8_lossy(&bytes).chars().take(400).collect());
    if matches!(status.as_u16(), 401) {
        return Err(JiraError::Auth(if message.is_empty() {
            "Jira did not accept the sign-in".into()
        } else {
            message
        }));
    }
    Err(JiraError::Http {
        status: status.as_u16(),
        message,
    })
}

/// A picture the site gives only to someone signed in, such as a project's avatar: its
/// content type and its bytes.
pub async fn fetch(
    credentials: &Credentials,
    path: &str,
    query: &[(String, String)],
) -> JiraResult<(Option<String>, Vec<u8>)> {
    let get = |renewed: Option<String>| {
        signed(
            http().map(|client| client.get(format!("{}{path}", credentials.url))),
            credentials,
            renewed.as_deref(),
        )
        .map(|request| request.header("Accept", "image/*").query(query))
    };
    let mut response = get(None)?.send().await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        if let Auth::Atlassian { site, token } = &credentials.auth {
            response = get(Some(oauth::renew(site, token).await?))?.send().await?;
        }
    }
    let status = response.status();
    if !status.is_success() {
        return Err(JiraError::Http {
            status: status.as_u16(),
            message: "the picture did not load".into(),
        });
    }
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    Ok((kind, read_limited(response).await?))
}

fn signed(
    request: JiraResult<RequestBuilder>,
    credentials: &Credentials,
    renewed: Option<&str>,
) -> JiraResult<RequestBuilder> {
    let request = request?;
    Ok(match (&credentials.auth, renewed) {
        (_, Some(token)) => request.bearer_auth(token),
        (Auth::Bearer(token) | Auth::Atlassian { token, .. }, None) => request.bearer_auth(token),
        (Auth::Basic { email, token }, None) => request.basic_auth(email, Some(token)),
    })
}

/// `renewed` stands in for the access token the credentials were made with.
async fn send_once(
    credentials: &Credentials,
    renewed: Option<&str>,
    method: &Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> JiraResult<(StatusCode, Vec<u8>)> {
    let request =
        http().map(|client| client.request(method.clone(), format!("{}{path}", credentials.url)));
    let mut request = signed(request, credentials, renewed)?
        .header("Accept", "application/json")
        .query(query);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await?;
    let status = response.status();
    Ok((status, read_limited(response).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jiras_errors_read_as_one_message() {
        let body = json!({ "errorMessages": ["Issue does not exist"], "errors": { "summary": "required" } });
        assert_eq!(
            error_message(&body).as_deref(),
            Some("Issue does not exist; summary: required")
        );
        assert_eq!(
            error_message(&json!({ "errorMessages": [], "errors": {} })),
            None
        );
    }
}
