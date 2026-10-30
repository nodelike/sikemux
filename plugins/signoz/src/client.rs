use std::path::Path;

use reqwest::{Method, Response};
use serde_json::Value;
use sikemux_plugin_api::http::{self, BodyError, Deadline, Http, Redirects};

use crate::auth::{self, Auth, Credentials};
use crate::error::{Signoz, SignozError, SignozResult};
use crate::query;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

static HTTP: Http<Signoz> = Http::new(Redirects::Refuse, Deadline::WholeAnswer, 16);

async fn read_limited(response: Response) -> SignozResult<Vec<u8>> {
    http::read_body(response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            BodyError::TooBig(_) => {
                SignozError::Response("more than 16 MiB came back; narrow the query".into())
            }
            error => error.into(),
        })
}

fn error_message(body: &Value) -> Option<String> {
    let error = body.get("error")?;
    error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(str::to_string)
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
    let response = HTTP.send(request).await?;
    let status = response.status();
    let bytes = read_limited(response).await?;
    if !status.is_success() {
        let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let message = error_message(&parsed)
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).chars().take(400).collect());
        if matches!(status.as_u16(), 401 | 403) && matches!(credentials.auth, Auth::ApiKey(_)) {
            auth::forget_api_key();
            return Err(SignozError::Auth(format!(
                "SigNoz turned down the API key: {message}"
            )));
        }
        return Err(SignozError::Http {
            status: status.as_u16(),
            message,
        });
    }
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
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
        (Err(SignozError::Http { status: 401, .. }), Auth::Bearer(refused)) => {
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
