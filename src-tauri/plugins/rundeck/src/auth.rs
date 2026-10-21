// Password login is two steps: POST /j_security_check for a session cookie,
// then POST /api/{v}/tokens/{user} to mint an API token. Each login gets its
// own cookie jar so sessions never leak between attempts. A pasted token is
// checked against /system/info and /user/info instead.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, Method, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::http::{self, Deadline, Redirects};
use sikemux_plugin_api::SignInStatus;

use crate::error::{Rundeck, RundeckError, RundeckResult};

use crate::client::{get_json, request_as, seg, Session, API_VERSION};
use crate::config::{self, Saved, Settings};

const MAX_AUTH_RESPONSE_BYTES: usize = 1024 * 1024;
const LOGIN_DEADLINE: Duration = Duration::from_secs(20);

async fn response_text_limited(resp: Response) -> RundeckResult<String> {
    let bytes = http::read_body(resp, MAX_AUTH_RESPONSE_BYTES)
        .await
        .map_err(|error| RundeckError::Auth(error.to_string()))?;
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
    sikemux_process::user_environment::command("hostname")
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
        .pin_dns(http::builder::<Rundeck>(
            Redirects::Refuse,
            Deadline::WholeAnswer,
        ))
        .cookie_provider(Arc::new(reqwest::cookie::Jar::default()))
        .timeout(LOGIN_DEADLINE)
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub url: String,
    pub user: String,
    pub token_present: bool,
    pub rundeck_version: Option<String>,
    pub allow_insecure_private_http: bool,
}

pub type RundeckStatus = SignInStatus<Connection>;

fn is_auth_failure(e: &RundeckError) -> bool {
    matches!(
        e,
        RundeckError::Auth(_) | RundeckError::Unconfigured | RundeckError::Http { status: 401, .. }
    )
}

/// A token that may not read the system info still signs in.
pub async fn status(data_dir: &Path) -> RundeckStatus {
    let (saved, problem) = match config::load(data_dir).await {
        Ok(saved) => (saved, None),
        Err(error) => (Saved::default(), Some(error.to_string())),
    };
    let settings = saved.settings.clone();
    let mut connection = Connection {
        url: settings.url.clone(),
        user: settings.user.clone(),
        token_present: saved.token.is_some(),
        rundeck_version: None,
        allow_insecure_private_http: settings.allow_insecure_private_http,
    };
    let (ok, auth_failed, message) = match Session::of(&saved) {
        Err(_) => (false, false, problem),
        Ok(session) => match get_json::<Value>(&session, "/system/info", &[]).await {
            Ok(info) => {
                connection.rundeck_version = rundeck_version(&info);
                (true, false, None)
            }
            Err(RundeckError::Forbidden(message)) => (true, false, Some(message)),
            Err(error) => (false, is_auth_failure(&error), Some(error.to_string())),
        },
    };
    SignInStatus {
        configured: saved.is_signed_in(),
        ok,
        auth_failed,
        message,
        details: connection,
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

pub async fn login(data_dir: &Path, req: LoginRequest) -> RundeckResult<LoginResult> {
    let url = req.url.trim_end_matches('/').to_string();
    let minted = perform_login(&LoginRequest {
        url: url.clone(),
        ..req.clone()
    })
    .await?;
    let session = Session {
        url: url.clone(),
        token: minted.token.clone(),
        user: req.user.clone(),
        allow_insecure_private_http: req.allow_insecure_private_http,
    };

    let previous = config::load(data_dir).await.unwrap_or_default().settings;
    if !previous.token_id.is_empty() && previous.url == url && previous.token_id != minted.id {
        revoke(&session, &previous.token_id).await;
    }
    config::save(
        data_dir,
        Saved {
            settings: Settings {
                url: url.clone(),
                user: req.user.clone(),
                token_id: minted.id,
                allow_insecure_private_http: req.allow_insecure_private_http,
                ..previous
            },
            token: Some(minted.token),
        },
    )
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

pub async fn login_with_token(
    data_dir: &Path,
    req: TokenLoginRequest,
) -> RundeckResult<LoginResult> {
    let url = req.url.trim().trim_end_matches('/').to_string();
    let token = req.token.trim().to_string();
    if token.is_empty() {
        return Err(RundeckError::BadArg("token is empty".into()));
    }
    let session = Session {
        url: url.clone(),
        token: token.clone(),
        user: String::new(),
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
    let previous = config::load(data_dir).await.unwrap_or_default().settings;
    config::save(
        data_dir,
        Saved {
            settings: Settings {
                url: url.clone(),
                user: user.clone(),
                token_id: String::new(),
                allow_insecure_private_http: req.allow_insecure_private_http,
                ..previous
            },
            token: Some(token),
        },
    )
    .await?;
    Ok(LoginResult {
        url,
        user,
        token_set: true,
        rundeck_version: version,
    })
}

pub async fn logout(data_dir: &Path) -> RundeckResult<()> {
    let mut saved = config::load(data_dir).await?;
    if let (Ok(session), false) = (Session::of(&saved), saved.settings.token_id.is_empty()) {
        revoke(&session, &saved.settings.token_id).await;
    }
    saved.token = None;
    saved.settings.token_id.clear();
    config::save(data_dir, saved).await
}

/// The lists people edit in the plugin's settings.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lists {
    pub branch_options: Vec<String>,
    pub prod_envs: Vec<String>,
}

/// The lists to change; one left out stays as it is, and an empty one goes back to the defaults.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChanges {
    pub branch_options: Option<Vec<String>>,
    pub prod_envs: Option<Vec<String>>,
}

pub async fn lists(data_dir: &Path) -> RundeckResult<Lists> {
    let settings = config::load(data_dir).await?.settings;
    Ok(Lists {
        branch_options: settings.branch_options,
        prod_envs: settings.prod_envs,
    })
}

pub async fn set_lists(data_dir: &Path, lists: ListChanges) -> RundeckResult<()> {
    fn tidy(names: Vec<String>, fallback: fn() -> Vec<String>) -> Vec<String> {
        let names: Vec<String> = names
            .into_iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        if names.is_empty() {
            fallback()
        } else {
            names
        }
    }
    let mut settings = config::load(data_dir).await?.settings;
    if let Some(names) = lists.branch_options {
        settings.branch_options = tidy(names, config::default_branch_options);
    }
    if let Some(names) = lists.prod_envs {
        settings.prod_envs = tidy(names, config::default_prod_envs);
    }
    config::save_settings(data_dir, settings).await
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
