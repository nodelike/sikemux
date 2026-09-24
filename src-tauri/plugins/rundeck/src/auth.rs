// Password login is two steps: POST /j_security_check for a session cookie,
// then POST /api/{v}/tokens/{user} to mint an API token. Each login gets its
// own cookie jar so sessions never leak between attempts. A pasted token is
// checked against /system/info and /user/info instead.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use reqwest::{Client, Method, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{RundeckError, RundeckResult};

use crate::client::{request_as, seg, Session, API_VERSION};
use crate::config::{self, RundeckConfig};

const MAX_AUTH_RESPONSE_BYTES: usize = 1024 * 1024;

async fn response_text_limited(resp: Response) -> RundeckResult<String> {
    if resp
        .content_length()
        .is_some_and(|size| size > MAX_AUTH_RESPONSE_BYTES as u64)
    {
        return Err(RundeckError::Auth(
            "authentication response exceeds 1 MiB limit".into(),
        ));
    }
    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| RundeckError::Auth(e.to_string()))?;
        if bytes.len() + chunk.len() > MAX_AUTH_RESPONSE_BYTES {
            return Err(RundeckError::Auth(
                "authentication response exceeds 1 MiB limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes)
        .map_err(|_| RundeckError::Auth("authentication response is not UTF-8".into()))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LoginRequest {
    pub url: String,
    pub user: String,
    pub password: String,
    #[serde(default)]
    pub allow_insecure_private_http: bool,
}

#[derive(Serialize, Clone)]
pub struct LoginResult {
    pub url: String,
    pub user: String,
    pub token_set: bool,
    pub rundeck_version: Option<String>,
}

fn short_hostname() -> String {
    use std::process::Command;
    Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn fresh_session_client(transport: &config::ValidatedTransport) -> RundeckResult<Client> {
    Ok(transport
        .pin_dns(Client::builder())
        .cookie_provider(Arc::new(reqwest::cookie::Jar::default()))
        .timeout(Duration::from_secs(20))
        .user_agent("sikemux-rundeck-auth/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

fn login_was_rejected(location: &str) -> bool {
    location.contains("/user/error") || location.contains("/user/login")
}

struct MintedToken {
    token: String,
    id: String,
}

/// Drive the j_security_check → /tokens/{user} flow.
async fn perform_login(req: &LoginRequest) -> RundeckResult<MintedToken> {
    let url = req.url.trim_end_matches('/');
    let transport = config::validate_transport(url, req.allow_insecure_private_http).await?;
    let client = fresh_session_client(&transport)?;

    let form = [
        ("j_username", req.user.as_str()),
        ("j_password", req.password.as_str()),
    ];
    let resp = client
        .post(format!("{url}/j_security_check"))
        .form(&form)
        .send()
        .await
        .map_err(|e| RundeckError::Auth(format!("login request: {e}")))?;
    let status = resp.status();
    if status.is_redirection() {
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if login_was_rejected(location) {
            return Err(RundeckError::Auth("invalid username or password".into()));
        }
    } else if !status.is_success() {
        return Err(RundeckError::Auth(format!(
            "login returned http {}",
            status.as_u16()
        )));
    }

    let roles: String = (async {
        let resp = client
            .get(format!("{url}/api/{API_VERSION}/user/roles"))
            .header("Accept", "application/json")
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let text = response_text_limited(resp).await.ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        let roles = v
            .get("roles")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        (!roles.is_empty()).then_some(roles)
    })
    .await
    .unwrap_or_else(|| "*".into());

    let payload = serde_json::json!({
        "user": req.user,
        "roles": roles,
        "name": format!("sikemux-{}", short_hostname()),
    });

    let token_resp = client
        .post(format!(
            "{url}/api/{API_VERSION}/tokens/{}",
            seg(&req.user)?
        ))
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| RundeckError::Auth(format!("token request: {e}")))?;

    let status = token_resp.status();
    if status.is_redirection() {
        return Err(RundeckError::Auth("invalid username or password".into()));
    }
    let body = response_text_limited(token_resp).await?;
    if !status.is_success() {
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(String::from))
            .unwrap_or(body);
        return Err(RundeckError::Auth(format!(
            "could not mint token (http {}): {}",
            status.as_u16(),
            msg
        )));
    }
    let parsed: Value = serde_json::from_str(&body).map_err(|_| {
        RundeckError::Auth("the token response was not JSON — is this a Rundeck URL?".into())
    })?;
    let text = |key: &str| parsed.get(key).and_then(Value::as_str).map(String::from);
    let token = text("token")
        .ok_or_else(|| RundeckError::Auth("token field missing in response".into()))?;
    Ok(MintedToken {
        token,
        id: text("id").unwrap_or_default(),
    })
}

// ---- Tauri commands ------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct RundeckStatus {
    pub configured: bool,
    pub url: String,
    pub user: String,
    pub token_present: bool,
    pub rundeck_version: Option<String>,
    pub ok: bool,
    pub auth_failed: bool,
    pub message: Option<String>,
    pub allow_insecure_private_http: bool,
}

fn is_auth_failure(e: &RundeckError) -> bool {
    matches!(
        e,
        RundeckError::Auth(_) | RundeckError::Unconfigured | RundeckError::Http { status: 401, .. }
    )
}

pub async fn status() -> RundeckStatus {
    // Always reload from disk on status checks — the CLI may have written
    // a new token since boot.
    let cfg = config::refresh_from_disk()
        .await
        .unwrap_or_else(|_| RundeckConfig::default());

    if !cfg.is_configured() {
        return RundeckStatus {
            configured: false,
            url: cfg.url,
            user: cfg.user,
            token_present: !cfg.token.is_empty(),
            rundeck_version: None,
            ok: false,
            auth_failed: false,
            message: None,
            allow_insecure_private_http: cfg.allow_insecure_private_http,
        };
    }

    if cfg.url.starts_with("http://") && !cfg.allow_insecure_private_http {
        return RundeckStatus {
            configured: true,
            url: cfg.url,
            user: cfg.user,
            token_present: true,
            rundeck_version: None,
            ok: false,
            auth_failed: true,
            message: Some("Private-subnet HTTP now requires explicit acknowledgement before credentials are sent".into()),
            allow_insecure_private_http: false,
        };
    }

    let info: RundeckResult<Value> = crate::client::get_json("/system/info", &[]).await;
    match info {
        Err(RundeckError::Forbidden(message)) => RundeckStatus {
            configured: true,
            url: cfg.url,
            user: cfg.user,
            token_present: true,
            rundeck_version: None,
            ok: true,
            auth_failed: false,
            message: Some(message),
            allow_insecure_private_http: cfg.allow_insecure_private_http,
        },
        Ok(v) => RundeckStatus {
            configured: true,
            url: cfg.url,
            user: cfg.user,
            token_present: true,
            rundeck_version: rundeck_version(&v),
            ok: true,
            auth_failed: false,
            message: None,
            allow_insecure_private_http: cfg.allow_insecure_private_http,
        },
        Err(e) => RundeckStatus {
            configured: true,
            url: cfg.url,
            user: cfg.user,
            token_present: true,
            rundeck_version: None,
            ok: false,
            auth_failed: is_auth_failure(&e),
            message: Some(e.to_string()),
            allow_insecure_private_http: cfg.allow_insecure_private_http,
        },
    }
}

fn rundeck_version(info: &Value) -> Option<String> {
    info.pointer("/system/rundeck/version")
        .and_then(Value::as_str)
        .map(String::from)
}

const REVOKE_TIMEOUT: Duration = Duration::from_secs(5);

async fn revoke(session: &Session, token_id: &str) {
    let Ok(id) = seg(token_id) else {
        return;
    };
    let endpoint = format!("/token/{id}");
    let _ = tokio::time::timeout(
        REVOKE_TIMEOUT,
        request_as::<Value>(session, Method::DELETE, &endpoint, None, &[]),
    )
    .await;
}

pub async fn login(req: LoginRequest) -> RundeckResult<LoginResult> {
    let url = req.url.trim_end_matches('/').to_string();
    let minted = perform_login(&LoginRequest {
        url: url.clone(),
        ..req.clone()
    })
    .await?;
    let session = Session {
        url: url.clone(),
        token: minted.token.clone(),
        allow_insecure_private_http: req.allow_insecure_private_http,
    };

    let previous = config::load().await.unwrap_or_default();
    if !previous.token_id.is_empty() && previous.url == url && previous.token_id != minted.id {
        revoke(&session, &previous.token_id).await;
    }
    config::save(RundeckConfig {
        url: url.clone(),
        user: req.user.clone(),
        token: minted.token,
        token_id: minted.id,
        allow_insecure_private_http: req.allow_insecure_private_http,
    })
    .await?;

    let version = request_as::<Value>(&session, Method::GET, "/system/info", None, &[])
        .await
        .ok()
        .and_then(|info| rundeck_version(&info));
    Ok(LoginResult {
        url,
        user: req.user,
        token_set: true,
        rundeck_version: version,
    })
}

#[derive(Deserialize)]
pub struct TokenLoginRequest {
    pub url: String,
    pub token: String,
    #[serde(default)]
    pub allow_insecure_private_http: bool,
}

pub async fn login_with_token(req: TokenLoginRequest) -> RundeckResult<LoginResult> {
    let url = req.url.trim().trim_end_matches('/').to_string();
    let token = req.token.trim().to_string();
    if token.is_empty() {
        return Err(RundeckError::BadArg("token is empty"));
    }
    let session = Session {
        url: url.clone(),
        token: token.clone(),
        allow_insecure_private_http: req.allow_insecure_private_http,
    };
    let (info, user_info) = tokio::join!(
        request_as::<Value>(&session, Method::GET, "/system/info", None, &[]),
        request_as::<Value>(&session, Method::GET, "/user/info", None, &[]),
    );
    let user = user_info?
        .get("login")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| {
            RundeckError::Auth("Rundeck did not say who this token belongs to".into())
        })?;
    let version = match info {
        Ok(info) => rundeck_version(&info),
        Err(RundeckError::Forbidden(_)) => None,
        Err(error) => return Err(error),
    };
    config::save(RundeckConfig {
        url: url.clone(),
        user: user.clone(),
        token,
        token_id: String::new(),
        allow_insecure_private_http: req.allow_insecure_private_http,
    })
    .await?;
    Ok(LoginResult {
        url,
        user,
        token_set: true,
        rundeck_version: version,
    })
}

pub async fn logout() -> RundeckResult<()> {
    // The URL and user stay so signing back in to the same Rundeck is quick.
    let mut cfg = config::load().await?;
    if !cfg.token_id.is_empty() && !cfg.token.is_empty() && !cfg.url.is_empty() {
        let session = Session {
            url: cfg.url.clone(),
            token: cfg.token.clone(),
            allow_insecure_private_http: cfg.allow_insecure_private_http,
        };
        revoke(&session, &cfg.token_id).await;
    }
    cfg.token.clear();
    cfg.token_id.clear();
    config::save(cfg).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redirect_back_to_the_login_page_means_bad_credentials() {
        assert!(login_was_rejected("https://rd.example.com/user/error"));
        assert!(login_was_rejected("/user/login?login_error=1"));
        assert!(!login_was_rejected("https://rd.example.com/menu/home"));
    }
}
