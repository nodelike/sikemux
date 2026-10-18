use std::path::Path;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::StreamSink;

use crate::client::{self, Auth, Credentials};
use crate::config::{self, normalise_host, SignIn as SiteSignIn, Site};
use crate::error::{JiraError, JiraResult};
use crate::oauth;

/// How long a browser sign-in waits for the person to finish before giving up the port.
const BROWSER_WAIT: Duration = Duration::from_secs(300);

/// Who is asking, on the site named or the default one.
pub async fn credentials(data_dir: &Path, host: Option<&str>) -> JiraResult<(Site, Credentials)> {
    let site = config::load(data_dir).site(host)?.clone();
    let credentials = match &site.sign_in {
        SiteSignIn::Token { email } => {
            let kept = site.clone();
            let token = config::blocking(Box::new(move || config::keychain_read(&kept)))
                .await?
                .ok_or(JiraError::Unconfigured)?;
            Credentials {
                url: format!("https://{}", site.host),
                auth: Auth::Basic {
                    email: email.clone(),
                    token,
                },
            }
        }
        SiteSignIn::Atlassian { cloud_id } => Credentials {
            url: format!("{}/{cloud_id}", oauth::API_URL),
            auth: Auth::Atlassian {
                site: site.clone(),
                token: oauth::access_token(&site).await?,
            },
        },
    };
    Ok((site, credentials))
}

struct Me {
    account_id: String,
    display_name: Option<String>,
}

async fn who_is(credentials: &Credentials) -> JiraResult<Me> {
    let me = client::send(credentials, Method::GET, "/rest/api/3/myself", &[], None).await?;
    let account_id = me
        .get("accountId")
        .and_then(Value::as_str)
        .ok_or_else(|| JiraError::Response("Jira did not say who is signed in".into()))?
        .to_string();
    Ok(Me {
        account_id,
        display_name: me
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    pub site: String,
    pub email: String,
    pub token: String,
}

/// Checks the email and API token against the site, then keeps the token in the Keychain.
pub async fn sign_in(data_dir: &Path, request: SignIn) -> JiraResult<()> {
    let host = config::cloud_host(&request.site)?;
    let email = request.email.trim().to_string();
    let token = request.token.trim().to_string();
    if email.is_empty() || token.is_empty() {
        return Err(JiraError::BadArg(
            "an email and an API token are both needed".into(),
        ));
    }
    let credentials = Credentials {
        url: format!("https://{host}"),
        auth: Auth::Basic {
            email: email.clone(),
            token: token.clone(),
        },
    };
    let me = who_is(&credentials).await?;
    let site = Site {
        host,
        account_id: me.account_id,
        display_name: me.display_name,
        sign_in: SiteSignIn::Token { email },
    };
    let kept = site.clone();
    config::blocking(Box::new(move || config::keychain_write(&kept, &token))).await?;
    let mut saved = config::load(data_dir);
    saved.upsert(site);
    config::save(data_dir, &saved)
}

/// Opens Atlassian's sign-in in the browser, through the URL sent down the stream, and
/// signs in to every Jira site the account reaches. Stopping the stream stops the wait.
pub async fn sign_in_with_browser(data_dir: &Path, sink: &StreamSink) -> JiraResult<()> {
    if !oauth::available() {
        return Err(JiraError::Auth(
            "this build cannot sign in through the browser; use an API token instead".into(),
        ));
    }
    let callback = oauth::listen().await?;
    let state = oauth::new_state()?;
    sink.send(json!({ "url": oauth::authorize_url(&state) }))
        .map_err(|_| JiraError::Auth("the sign-in was closed".into()))?;
    let code = tokio::time::timeout(BROWSER_WAIT, oauth::code(&callback, &state))
        .await
        .map_err(|_| JiraError::Auth("nobody finished signing in".into()))??;
    drop(callback);
    let tokens = oauth::exchange(&code).await?;
    let cloud_sites = oauth::sites(&tokens.access_token).await?;
    let Some(first) = cloud_sites.first() else {
        return Err(JiraError::Auth(
            "that Atlassian account has no Jira site it let Sikemux into".into(),
        ));
    };
    let me = who_is(&Credentials {
        url: format!("{}/{}", oauth::API_URL, first.id),
        auth: Auth::Bearer(tokens.access_token.clone()),
    })
    .await?;
    let sites: Vec<Site> = cloud_sites
        .iter()
        .map(|cloud| Site {
            host: normalise_host(&cloud.url),
            account_id: me.account_id.clone(),
            display_name: me.display_name.clone(),
            sign_in: SiteSignIn::Atlassian {
                cloud_id: cloud.id.clone(),
            },
        })
        .collect();
    if let Some(site) = sites.first().cloned() {
        let refresh_token = tokens.refresh_token.clone();
        config::blocking(Box::new(move || {
            config::keychain_write(&site, &refresh_token)
        }))
        .await?;
    }
    oauth::remember(&me.account_id, &tokens).await;
    let mut saved = config::load(data_dir);
    for site in sites {
        saved.upsert(site);
    }
    config::save(data_dir, &saved)
}

#[derive(Deserialize)]
pub struct SignOut {
    pub site: String,
}

/// A browser sign-in is kept until the last of its sites is signed out of.
pub async fn sign_out(data_dir: &Path, request: SignOut) -> JiraResult<()> {
    let mut saved = config::load(data_dir);
    let Some(site) = saved.remove(&normalise_host(&request.site)) else {
        return Ok(());
    };
    let still_used = saved
        .sites
        .iter()
        .any(|other| other.shares_sign_in_with(&site));
    if !still_used {
        let account_id = site.account_id.clone();
        config::blocking(Box::new(move || config::keychain_delete(&site))).await?;
        oauth::forget(&account_id).await;
    }
    config::save(data_dir, &saved)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteStatus {
    pub host: String,
    pub display_name: Option<String>,
    pub default: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    pub sites: Vec<SiteStatus>,
    /// Whether the default site still takes its token; false with `authFailed` when it was revoked.
    pub ok: bool,
    pub auth_failed: bool,
    pub message: Option<String>,
    /// A build without the OAuth secret can only take an API token.
    pub browser_sign_in: bool,
}

pub async fn status(data_dir: &Path) -> Status {
    let saved = config::load(data_dir);
    let sites = saved
        .sites
        .iter()
        .map(|site| SiteStatus {
            host: site.host.clone(),
            display_name: site.display_name.clone(),
            default: saved.default.as_deref() == Some(site.host.as_str()),
        })
        .collect::<Vec<_>>();
    let probe = match credentials(data_dir, None).await {
        Ok((_, credentials)) => who_is(&credentials).await.map(|_| ()),
        Err(error) => Err(error),
    };
    let (ok, auth_failed, message) = match probe {
        Ok(()) => (true, false, None),
        Err(error) => {
            let auth_failed = matches!(
                error,
                JiraError::Unconfigured
                    | JiraError::Auth(_)
                    | JiraError::Http {
                        status: 401 | 403,
                        ..
                    }
            );
            (
                false,
                auth_failed,
                (!sites.is_empty()).then(|| error.to_string()),
            )
        }
    };
    Status {
        configured: !sites.is_empty(),
        sites,
        ok,
        auth_failed,
        message,
        browser_sign_in: oauth::available(),
    }
}
