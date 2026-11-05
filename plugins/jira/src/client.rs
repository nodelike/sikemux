use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::Value;
use sikemux_plugin_api::http::{Answer, Http, Read, Redirects};

use crate::config::Site;
use crate::error::{Jira, JiraResult};
use crate::oauth;

pub static HTTP: Http<Jira> = Http::new(Redirects::Refuse, 16);

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
    let mut answer = send_once(credentials, None, &method, path, query, body).await?;
    if answer.status == StatusCode::UNAUTHORIZED {
        if let Auth::Atlassian { site, token } = &credentials.auth {
            let renewed = oauth::ACCESS
                .renew(&site.account_id, token, site.keychain_account())
                .await?
                .token;
            answer = send_once(credentials, Some(&renewed), &method, path, query, body).await?;
        }
    }
    answer.succeeded()?.parsed()
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
) -> JiraResult<Answer> {
    let request = HTTP
        .client()
        .map(|client| client.request(method.clone(), format!("{}{path}", credentials.url)));
    let mut request = signed(request, credentials, renewed)?
        .header("Accept", "application/json")
        .query(query);
    if let Some(body) = body {
        request = request.json(body);
    }
    HTTP.ask(request, Read::ALL).await
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
