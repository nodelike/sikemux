use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use reqwest::{Method, StatusCode};
use serde_json::Value;
use sikemux_plugin_api::http::{self, Http, Read, Redirects};
use sikemux_plugin_api::ratelimit::LimitHeaders;
use sikemux_plugin_api::store::{Accounts, Held};

use crate::config::{self, Account, CloudflareAccount, SignIn};
use crate::error::{Cloudflare, CloudflareError, CloudflareResult};
use crate::oauth;

const API: &str = "https://api.cloudflare.com/client/v4";

pub use sikemux_plugin_api::json::{at, number, some_text, text};

pub static HTTP: Http<Cloudflare> = Http::with_limits(
    Redirects::Follow,
    8,
    LimitHeaders {
        remaining: None,
        reset: None,
    },
);

static TOKENS: Held<String> = Held::secrets();

#[derive(Default)]
struct ZoneCache {
    by_name: BTreeMap<String, String>,
    by_id: BTreeMap<String, String>,
}

static ZONE_CACHE: Mutex<BTreeMap<String, ZoneCache>> = Mutex::new(BTreeMap::new());

pub fn forget(account_id: Option<&str>) {
    TOKENS.forget(account_id);
    if let Ok(mut cache) = ZONE_CACHE.lock() {
        match account_id {
            Some(id) => {
                cache.remove(id);
            }
            None => cache.clear(),
        }
    }
}

pub fn forget_zone(account_id: &str, zone: &str) {
    let key = normalize_domain(zone);
    if let Ok(mut cache) = ZONE_CACHE.lock() {
        if let Some(entry) = cache.get_mut(account_id) {
            if let Some(id) = entry.by_name.remove(&key) {
                entry.by_id.remove(&id);
            }
            if let Some(name) = entry.by_id.remove(&key) {
                entry.by_name.remove(&name);
            }
        }
    }
}

pub fn cache_zone(account_id: &str, id: &str, name: &str) {
    let id = id.trim().to_ascii_lowercase();
    let name = normalize_domain(name);
    if id.is_empty() || name.is_empty() {
        return;
    }
    if let Ok(mut cache) = ZONE_CACHE.lock() {
        let entry = cache.entry(account_id.to_owned()).or_default();
        entry.by_name.insert(name.clone(), id.clone());
        entry.by_id.insert(id, name);
    }
}

fn cached_zone(account_id: &str, input: &str) -> Option<(String, String)> {
    let key = normalize_domain(input);
    let cache = ZONE_CACHE.lock().ok()?;
    let entry = cache.get(account_id)?;
    if is_cf_id(&key) {
        let name = entry.by_id.get(&key)?;
        Some((key, name.clone()))
    } else {
        let id = entry.by_name.get(&key)?;
        Some((id.clone(), key))
    }
}

pub fn is_cf_id(value: &str) -> bool {
    let value = value.trim();
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn normalize_domain(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

pub struct Session {
    pub account: Account,
    token: Option<String>,
}

impl Session {
    pub async fn of(data_dir: &Path) -> CloudflareResult<Session> {
        let account = config::load(data_dir)?
            .account(None)
            .cloned()
            .ok_or(CloudflareError::Unconfigured)?;
        let token = match account.sign_in {
            SignIn::Token => Some(
                TOKENS
                    .secret(config::KEYCHAIN, &account.id, account.id.clone())
                    .await?,
            ),
            SignIn::Browser => None,
        };
        Ok(Session { account, token })
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> CloudflareResult<Value> {
        self.send(Method::GET, path, query, None).await
    }

    pub async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> CloudflareResult<Value> {
        let account_id = self.account.id.clone();
        match &self.token {
            Some(token) => request_json(&account_id, token, method, path, query, body).await,
            None => {
                let query = query.to_vec();
                let body = body.cloned();
                oauth::ACCESS
                    .with_access(
                        &self.account.id,
                        || self.account.id.clone(),
                        |access| {
                            let account_id = account_id.clone();
                            let method = method.clone();
                            let query = query.clone();
                            let body = body.clone();
                            async move {
                                request_json(
                                    &account_id,
                                    &access.token,
                                    method,
                                    path,
                                    &query,
                                    body.as_ref(),
                                )
                                .await
                            }
                        },
                    )
                    .await
            }
        }
    }

    pub async fn get_text(&self, path: &str, query: &[(&str, String)]) -> CloudflareResult<String> {
        let account_id = self.account.id.clone();
        match &self.token {
            Some(token) => request_text(&account_id, token, path, query).await,
            None => {
                let query = query.to_vec();
                oauth::ACCESS
                    .with_access(
                        &self.account.id,
                        || self.account.id.clone(),
                        |access| {
                            let account_id = account_id.clone();
                            let query = query.clone();
                            async move {
                                request_text(&account_id, &access.token, path, &query).await
                            }
                        },
                    )
                    .await
            }
        }
    }

    pub async fn list_accounts(&self) -> CloudflareResult<Vec<CloudflareAccount>> {
        let body = self.get("/accounts", &[("per_page", "50".into())]).await?;
        Ok(accounts_of(&body))
    }

    pub async fn resolve_account_id(&self, requested: Option<&str>) -> CloudflareResult<String> {
        let requested = requested.map(str::trim).filter(|value| !value.is_empty());
        if let Some(requested) = requested {
            if is_cf_id(requested) {
                return Ok(requested.to_ascii_lowercase());
            }
            if let Some(found) = find_account(&self.account.cf_accounts, requested) {
                return Ok(found.id.clone());
            }
            let live = self.list_accounts().await?;
            if let Some(found) = find_account(&live, requested) {
                return Ok(found.id.clone());
            }
            return Err(CloudflareError::NotFound(format!(
                "cloudflare: account '{requested}' not found. Available accounts: {}",
                describe_accounts(&live)
            )));
        }

        if let Some(default_id) = self
            .account
            .default_account
            .as_deref()
            .filter(|id| !id.is_empty())
        {
            return Ok(default_id.to_owned());
        }

        let accounts = if !self.account.cf_accounts.is_empty() {
            self.account.cf_accounts.clone()
        } else {
            self.list_accounts().await?
        };
        pick_single_account(&accounts)
    }

    pub async fn resolve_zone(
        &self,
        zone: &str,
        account: Option<&str>,
    ) -> CloudflareResult<(String, String)> {
        let zone = zone.trim();
        if zone.is_empty() {
            return Err(CloudflareError::BadArg("zone is required".into()));
        }
        if let Some(cached) = cached_zone(&self.account.id, zone) {
            return Ok(cached);
        }
        if is_cf_id(zone) {
            let id = zone.to_ascii_lowercase();
            let answer = self
                .get(&format!("/zones/{id}"), &[])
                .await
                .map_err(|error| {
                    if matches!(error, CloudflareError::NotFound(_)) {
                        forget_zone(&self.account.id, &id);
                    }
                    error
                })?;
            let name = normalize_domain(&text(&answer, "/result/name"));
            if name.is_empty() {
                return Err(CloudflareError::NotFound(format!(
                    "cloudflare: zone '{id}' not found"
                )));
            }
            cache_zone(&self.account.id, &id, &name);
            return Ok((id, name));
        }

        let domain = normalize_domain(zone);
        let mut query = vec![("name", domain.clone()), ("per_page", "50".into())];
        if let Some(account) = account.map(str::trim).filter(|value| !value.is_empty()) {
            let account_id = self.resolve_account_id(Some(account)).await?;
            query.push(("account.id", account_id));
        }
        let answer = self.get("/zones", &query).await?;
        let matches: Vec<(String, String)> = at(&answer, "/result")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let id = text(item, "/id");
                let name = normalize_domain(&text(item, "/name"));
                (!id.is_empty() && name == domain).then_some((id, name))
            })
            .collect();
        match matches.as_slice() {
            [(id, name)] => {
                cache_zone(&self.account.id, id, name);
                Ok((id.clone(), name.clone()))
            }
            [] => Err(CloudflareError::NotFound(format!(
                "cloudflare: zone '{domain}' not found"
            ))),
            _ => Err(CloudflareError::BadArg(format!(
                "cloudflare: multiple zones match '{domain}'; pass the 32-char zone ID or specify account"
            ))),
        }
    }
}

pub fn accounts_of(body: &Value) -> Vec<CloudflareAccount> {
    at(body, "/result")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = text(item, "/id");
            let name = text(item, "/name");
            (!id.is_empty()).then_some(CloudflareAccount { id, name })
        })
        .collect()
}

pub fn find_account<'a>(
    accounts: &'a [CloudflareAccount],
    wanted: &str,
) -> Option<&'a CloudflareAccount> {
    let wanted = wanted.trim();
    accounts.iter().find(|account| {
        account.id.eq_ignore_ascii_case(wanted) || account.name.eq_ignore_ascii_case(wanted)
    })
}

pub fn pick_single_account(accounts: &[CloudflareAccount]) -> CloudflareResult<String> {
    match accounts {
        [only] => Ok(only.id.clone()),
        [] => Err(CloudflareError::NotFound(
            "cloudflare: no accounts are visible to this sign-in".into(),
        )),
        many => Err(CloudflareError::BadArg(format!(
            "cloudflare: multiple accounts exist; pass `account` with one of: {}",
            describe_accounts(many)
        ))),
    }
}

fn describe_accounts(accounts: &[CloudflareAccount]) -> String {
    if accounts.is_empty() {
        return "none".into();
    }
    accounts
        .iter()
        .map(|account| format!("{} ({})", account.name, account.id))
        .collect::<Vec<_>>()
        .join(", ")
}

fn first_error(body: &Value) -> Option<(Option<u64>, String)> {
    let errors = at(body, "/errors").as_array()?;
    let first = errors.first()?;
    let code = first.get("code").and_then(Value::as_u64);
    let message = first
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if message.is_empty() && code.is_none() {
        return None;
    }
    let summary = errors
        .iter()
        .filter_map(|item| {
            let msg = item.get("message").and_then(Value::as_str)?;
            match item.get("code").and_then(Value::as_u64) {
                Some(code) => Some(format!("{msg} (code {code})")),
                None => Some(msg.to_owned()),
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some((code, if summary.is_empty() { message } else { summary }))
}

pub fn said(body: &[u8]) -> Option<String> {
    if let Ok(parsed) = serde_json::from_slice::<Value>(body) {
        if let Some((_, message)) = first_error(&parsed) {
            return Some(message);
        }
        if let Some(message) = parsed.get("error_description").and_then(Value::as_str) {
            return Some(message.to_owned());
        }
    }
    http::quoted(body)
}

pub fn refused(status: StatusCode, body: &[u8], spent: Option<u64>) -> CloudflareError {
    if let Some(resets_in_secs) = spent {
        return CloudflareError::RateLimited {
            resets_in_secs: Some(resets_in_secs),
        };
    }
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let (code, message) = first_error(&parsed).unwrap_or_else(|| {
        (
            None,
            said(body).unwrap_or_else(|| {
                status
                    .canonical_reason()
                    .unwrap_or("Cloudflare refused the request")
                    .to_owned()
            }),
        )
    });
    classify_error(status, code, message)
}

fn classify_error(status: StatusCode, code: Option<u64>, message: String) -> CloudflareError {
    if status == StatusCode::UNAUTHORIZED || matches!(code, Some(10000 | 9109 | 6003)) {
        if status == StatusCode::FORBIDDEN && code == Some(10000) {
            return CloudflareError::Forbidden(format!(
                "{message} (missing API token permission or OAuth scope)"
            ));
        }
        return CloudflareError::Auth(message);
    }
    match status.as_u16() {
        403 => CloudflareError::Forbidden(format!(
            "{message} (missing API token permission or OAuth scope)"
        )),
        404 => CloudflareError::NotFound(message),
        429 => CloudflareError::RateLimited {
            resets_in_secs: None,
        },
        400 | 409 | 422 => CloudflareError::Api(message),
        _ if matches!(code, Some(7003 | 81044)) => CloudflareError::NotFound(message),
        _ => http::classify(status, Some(message), None),
    }
}

pub fn checked_envelope(
    status: StatusCode,
    bytes: &[u8],
    spent: Option<u64>,
) -> CloudflareResult<Value> {
    if !status.is_success() {
        return Err(refused(status, bytes, spent));
    }
    let body: Value = http::parse(bytes)?;
    if body.get("success").and_then(Value::as_bool) == Some(false) {
        let (code, message) = first_error(&body)
            .unwrap_or_else(|| (None, "Cloudflare reported success=false".to_owned()));
        return Err(classify_error(status, code, message));
    }
    Ok(body)
}

pub async fn request_json(
    account_id: &str,
    token: &str,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> CloudflareResult<Value> {
    let mut builder = HTTP
        .client()?
        .request(method, format!("{API}{path}"))
        .bearer_auth(token)
        .query(query);
    if let Some(body) = body {
        builder = builder.json(body);
    }
    let answer = HTTP.exchange(account_id, builder, Read::ALL).await?;
    checked_envelope(answer.status, &answer.bytes, None).map_err(|error| {
        if matches!(answer.status, StatusCode::TOO_MANY_REQUESTS) {
            answer.refused()
        } else {
            error
        }
    })
}

pub async fn request_text(
    account_id: &str,
    token: &str,
    path: &str,
    query: &[(&str, String)],
) -> CloudflareResult<String> {
    let builder = HTTP
        .client()?
        .get(format!("{API}{path}"))
        .bearer_auth(token)
        .query(query);
    let answer = HTTP.exchange(account_id, builder, Read::ALL).await?;
    if !answer.status.is_success() {
        return Err(answer.refused());
    }
    Ok(answer.text())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zone_cache_resolves_both_directions_and_clears() {
        let acct = "test-cache-acct";
        let zone_id = "023e105f4ecef8ad9ca31a8372d0c353";
        forget(Some(acct));
        cache_zone(acct, zone_id, "Sikemux.COM.");
        assert_eq!(
            cached_zone(acct, "sikemux.com"),
            Some((zone_id.into(), "sikemux.com".into()))
        );
        assert_eq!(
            cached_zone(acct, zone_id),
            Some((zone_id.into(), "sikemux.com".into()))
        );
        forget_zone(acct, zone_id);
        assert_eq!(cached_zone(acct, "sikemux.com"), None);
        assert_eq!(cached_zone(acct, zone_id), None);
    }

    #[test]
    fn cloudflare_errors_map_auth_forbidden_and_rate_limit() {
        let auth = json!({
            "success": false,
            "errors": [{ "code": 9109, "message": "Invalid access token" }]
        });
        assert!(matches!(
            refused(StatusCode::UNAUTHORIZED, auth.to_string().as_bytes(), None),
            CloudflareError::Auth(_)
        ));
        let scope = json!({
            "success": false,
            "errors": [{ "code": 10000, "message": "Authentication error" }]
        });
        assert!(matches!(
            refused(StatusCode::FORBIDDEN, scope.to_string().as_bytes(), None),
            CloudflareError::Forbidden(message) if message.contains("missing API token permission")
        ));
        assert!(matches!(
            refused(StatusCode::TOO_MANY_REQUESTS, b"{}", Some(42)),
            CloudflareError::RateLimited {
                resets_in_secs: Some(42)
            }
        ));
    }

    #[test]
    fn multi_account_resolution_requires_choice_when_ambiguous() {
        let one = vec![CloudflareAccount {
            id: "0123456789abcdef0123456789abcdef".into(),
            name: "Personal".into(),
        }];
        assert_eq!(
            pick_single_account(&one).ok().as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        let two = vec![
            one[0].clone(),
            CloudflareAccount {
                id: "fedcba9876543210fedcba9876543210".into(),
                name: "Work".into(),
            },
        ];
        let error = pick_single_account(&two).err();
        assert!(matches!(
            error,
            Some(CloudflareError::BadArg(message)) if message.contains("Personal") && message.contains("Work")
        ));
    }
}
