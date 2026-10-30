use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::oauth::Tokens;
use sikemux_plugin_api::store::{self, Accounts};
use sikemux_plugin_api::{SignInStatus, StreamSink};
use sikemux_plugin_jira_views::{SiteStatus, Sites};

use crate::client::{self, Auth, Credentials};
use crate::config::{self, normalise_host, JiraConfig, SignIn as SiteSignIn, Site};
use crate::error::{JiraError, JiraResult};
use crate::oauth;

/// Who is asking, on the site named or the default one.
pub async fn credentials(data_dir: &Path, host: Option<&str>) -> JiraResult<(Site, Credentials)> {
    let site = config::load(data_dir)?.site(host)?.clone();
    let credentials = match &site.sign_in {
        SiteSignIn::Token { email } => {
            let kept = site.keychain_account();
            let token = store::blocking(move || Ok(config::KEYCHAIN.read(&kept)?))
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
                token: oauth::ACCESS
                    .access(&site.account_id, oauth::entry(&site))
                    .await?
                    .token,
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
    store::update(data_dir, move |config: &mut JiraConfig| {
        config::KEYCHAIN.write(&site.keychain_account(), &token)?;
        config.upsert(site);
        Ok(())
    })
    .await
}

/// Opens Atlassian's sign-in in the browser, through the URL sent down the stream, and
/// signs in to every Jira site the account reaches. Stopping the stream stops the wait.
pub async fn sign_in_with_browser(data_dir: &Path, sink: &StreamSink) -> JiraResult<()> {
    let tokens: Tokens = oauth::APP.sign_in(&client::HTTP, sink).await?;
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
    let refresh_token = tokens.refresh_token.clone();
    store::update(data_dir, move |config: &mut JiraConfig| {
        if let Some(site) = sites.first() {
            config::KEYCHAIN.write(&site.keychain_account(), &refresh_token)?;
        }
        for site in sites {
            config.upsert(site);
        }
        Ok(())
    })
    .await?;
    oauth::ACCESS.remember(&me.account_id, &tokens).await;
    Ok(())
}

#[derive(Deserialize)]
pub struct SignOut {
    pub site: String,
}

/// A browser sign-in is kept until the last of its sites is signed out of.
pub async fn sign_out(data_dir: &Path, request: SignOut) -> JiraResult<()> {
    let host = normalise_host(&request.site);
    let unused = store::update(data_dir, move |config: &mut JiraConfig| {
        let Some(site) = config.remove(&host) else {
            return Ok(None);
        };
        let still_used = config
            .sites
            .iter()
            .any(|other| other.shares_sign_in_with(&site));
        if still_used {
            return Ok(None);
        }
        config::KEYCHAIN.delete(&site.keychain_account())?;
        Ok(Some(site.account_id))
    })
    .await?;
    if let Some(account_id) = unused {
        oauth::ACCESS.forget(Some(&account_id)).await;
    }
    Ok(())
}

/// `ok` says whether the default site still takes its token; false with `authFailed` when it was revoked.
pub type Status = SignInStatus<Sites>;

pub async fn status(data_dir: &Path) -> Status {
    let saved = match config::load(data_dir) {
        Ok(saved) => saved,
        Err(error) => return status_of(Vec::new(), Some(error)),
    };
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
    status_of(sites, probe.err())
}

/// A site whose token is gone from the Keychain, or that Jira refuses, needs signing in again.
fn status_of(sites: Vec<SiteStatus>, error: Option<JiraError>) -> Status {
    let auth_failed = matches!(
        error,
        Some(
            JiraError::Unconfigured
                | JiraError::Auth(_)
                | JiraError::Http {
                    status: 401 | 403,
                    ..
                }
        )
    );
    let configured = !sites.is_empty();
    Status {
        configured,
        ok: error.is_none(),
        auth_failed,
        message: error
            .filter(|error| configured || !matches!(error, JiraError::Unconfigured))
            .map(|error| error.to_string()),
        details: Sites {
            sites,
            browser_sign_in: oauth::APP.available(),
        },
    }
}
