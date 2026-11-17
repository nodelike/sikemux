use std::path::Path;

use reqwest::Method;
use serde_json::Value;
use sikemux_plugin_api::http::{Http, Read, Redirects};
use sikemux_plugin_api::Service;

use crate::auth::{self, Auth, Credentials};
use crate::error::{Signoz, SignozError, SignozResult};
use crate::query;

static HTTP: Http<Signoz> = Http::new(Redirects::Refuse, 16);

/// SigNoz's message, with the reasons it lists under it, which name the field at fault.
pub fn error_message(body: &Value) -> Option<String> {
    let error = body.get("error")?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())?;
    let reasons: Vec<&str> = error
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|reason| reason.get("message").and_then(Value::as_str))
        .filter(|reason| !reason.is_empty() && !message.contains(reason))
        .collect();
    Some(if reasons.is_empty() {
        message.to_string()
    } else {
        format!("{message}: {}", reasons.join("; "))
    })
}

/// One request with exactly the credentials given.
pub async fn send(
    credentials: &Credentials,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> SignozResult<Value> {
    let mut request = HTTP
        .client()?
        .request(method, format!("{}{path}", credentials.url))
        .header("Accept", "application/json");
    request = match &credentials.auth {
        Auth::None => request,
        Auth::ApiKey(key) => request.header("SIGNOZ-API-KEY", key),
        Auth::Bearer(token) => request.bearer_auth(token),
    };
    if let Some(body) = body {
        request = request.json(body);
    }
    let answer = HTTP.ask(request, Read::ALL).await?;
    if !answer.status.is_success() {
        if matches!(answer.status.as_u16(), 401 | 403)
            && matches!(credentials.auth, Auth::ApiKey(_))
        {
            auth::forget_api_key();
            return Err(SignozError::Auth(format!(
                "SigNoz turned down the API key: {}",
                Signoz::said(&answer.bytes).unwrap_or_default()
            )));
        }
        return Err(answer.refused());
    }
    let parsed: Value = answer.parsed()?;
    if parsed
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|value| value != "success")
    {
        return Err(SignozError::Response(
            error_message(&parsed).unwrap_or_else(|| "the query failed".into()),
        ));
    }
    Ok(parsed)
}

/// A request as whoever is signed in. A session token SigNoz turns down is
/// renewed and the request tried once more.
pub async fn request(
    data_dir: &Path,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> SignozResult<Value> {
    let credentials = auth::credentials(data_dir).await?;
    match (
        send(&credentials, method.clone(), path, body).await,
        &credentials.auth,
    ) {
        (Err(SignozError::Auth(_)), Auth::Bearer(refused)) => {
            let renewed = auth::renew(data_dir, refused).await?;
            send(&renewed, method, path, body).await
        }
        (answer, _) => answer,
    }
}

fn first_result(body: Value) -> SignozResult<Value> {
    body.pointer("/data/data/results/0")
        .cloned()
        .ok_or_else(|| SignozError::Response("no results in the answer".into()))
}

/// The rows and columns of the one query in `query`, which is always named "A".
pub async fn query_range(data_dir: &Path, query: &Value) -> SignozResult<Value> {
    first_result(request(data_dir, Method::POST, "/api/v5/query_range", Some(query)).await?)
}

/// Proves the credentials can read, with the smallest query there is.
pub async fn check(credentials: &Credentials) -> SignozResult<()> {
    let probe = query::builder(
        "raw",
        query::window(Some(1)),
        serde_json::json!({ "signal": "logs", "limit": 1 }),
    );
    send(
        credentials,
        Method::POST,
        "/api/v5/query_range",
        Some(&probe),
    )
    .await
    .map(|_| ())
}
