use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use sikemux_plugin_api::http::{self, BodyError, Deadline, Http, Redirects};

use crate::config::Site;
use crate::error::{Jira, JiraError, JiraResult};
use crate::oauth;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub static HTTP: Http<Jira> = Http::new(Redirects::Refuse, Deadline::WholeAnswer, 16);

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

pub async fn read_limited(response: Response) -> JiraResult<Vec<u8>> {
    http::read_body(response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            BodyError::TooBig(_) => {
                JiraError::Response("more than 16 MiB came back; narrow the query".into())
            }
            error => error.into(),
        })
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
            let renewed = oauth::ACCESS
                .renew(&site.account_id, token, oauth::entry(site))
                .await?
                .token;
            (status, bytes) =
                send_once(credentials, Some(&renewed), &method, path, query, body).await?;
        }
    }
    if status.is_success() {
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
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

/// A picture the site gives only to someone signed in, such as a project's avatar.
pub async fn fetch(
    credentials: &Credentials,
    path: &str,
    query: &[(String, String)],
) -> JiraResult<Response> {
    let get = |renewed: Option<String>| {
        signed(
            HTTP.client()
                .map(|client| client.get(format!("{}{path}", credentials.url))),
            credentials,
            renewed.as_deref(),
        )
        .map(|request| request.header("Accept", "image/*").query(query))
    };
    let mut response = HTTP.send(get(None)?).await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        if let Auth::Atlassian { site, token } = &credentials.auth {
            response = HTTP
                .send(get(Some(
                    oauth::ACCESS
                        .renew(&site.account_id, token, oauth::entry(site))
                        .await?
                        .token,
                ))?)
                .await?;
        }
    }
    let status = response.status();
    if !status.is_success() {
        return Err(JiraError::Http {
            status: status.as_u16(),
            message: "the picture did not load".into(),
        });
    }
    Ok(response)
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
    let request = HTTP
        .client()
        .map(|client| client.request(method.clone(), format!("{}{path}", credentials.url)));
    let mut request = signed(request, credentials, renewed)?
        .header("Accept", "application/json")
        .query(query);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = HTTP.send(request).await?;
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
