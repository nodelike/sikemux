use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::oauth::Tokens;
use sikemux_plugin_api::store::{self, Accounts, Held};
use sikemux_plugin_api::{SignInStatus, SignedInAs, StreamSink};

use crate::client::{self, Auth, Credentials};
use crate::config::{self, normalise_host, JiraConfig, SignIn as SiteSignIn, Site};
use crate::error::{JiraError, JiraResult};
use crate::oauth;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SiteStatus {
    pub host: String,
    pub display_name: Option<String>,
    pub default: bool,
}

/// The sites signed in to, as the plugin's `status` says.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Sites {
    pub sites: Vec<SiteStatus>,
    /// A build without the OAuth secret can only take an API token.
    pub browser_sign_in: bool,
}

impl Sites {
    fn of(sites: Vec<SiteStatus>) -> Self {
        Self {
            sites,
            browser_sign_in: oauth::APP.available(),
        }
    }
}

impl SignedInAs for Sites {
    fn signed_in_as(&self) -> Option<String> {
        let named: Vec<String> = self
            .sites
            .iter()
            .map(|site| match &site.display_name {
                Some(name) => format!("{name} on {}", site.host),
                None => site.host.clone(),
            })
            .collect();
        Some(named.join(", "))
    }
}

/// The token read lately for each site, under `Site::keychain_account`.
static TOKENS: Held<String> = Held::secrets();

/// Who is asking, on the site named or the default one.
pub async fn credentials(data_dir: &Path, host: Option<&str>) -> JiraResult<(Site, Credentials)> {
    let site = config::load(data_dir)?.site(host)?.clone();
    let credentials = match &site.sign_in {
        SiteSignIn::Token { email } => {
            let kept = site.keychain_account();
            let token = TOKENS.secret(config::KEYCHAIN, &kept, kept.clone()).await?;
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
                    .access(&site.account_id, site.keychain_account())
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
    let entry = site.keychain_account();
    let forgotten = entry.clone();
    store::update(data_dir, move |config: &mut JiraConfig| {
        config::KEYCHAIN.write(&entry, &token)?;
        config.upsert(site);
        Ok(())
    })
    .await?;
    TOKENS.forget(Some(&forgotten));
    Ok(())
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
    /// The site to sign out of; the default one when none is named.
    #[serde(default)]
    pub site: Option<String>,
}

/// A browser sign-in is kept until the last of its sites is signed out of.
pub async fn sign_out(data_dir: &Path, request: SignOut) -> JiraResult<()> {
    let unused = store::update(data_dir, move |config: &mut JiraConfig| {
        let Ok(host) = config
            .site(request.site.as_deref())
            .map(|site| site.host.clone())
        else {
            return Ok(None);
        };
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
    TOKENS.forget(None);
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
        Err(error) => return Status::checked(false, Sites::of(Vec::new()), &Err::<(), _>(error)),
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
    Status::checked(!sites.is_empty(), Sites::of(sites), &probe)
}
