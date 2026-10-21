// The Jira Cloud sites signed in here. A site's API token, or the Atlassian
// sign-in it came from, lives in the Keychain; the file beside it only lists the
// sites, who is signed in to each, and which one to use when none is named.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::keychain_service;
use sikemux_plugin_api::store::{self, Accounts};

use crate::error::{JiraError, JiraResult};

pub const TOKEN_SERVICE: &str = keychain_service!("sikemux-jira-token");

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    /// The site's host, e.g. `acme.atlassian.net`, which also names it to tools.
    pub host: String,
    pub account_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub sign_in: SignIn,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum SignIn {
    /// An email and the API token made for it, sent to the site itself.
    #[serde(rename_all = "camelCase")]
    Token { email: String },
    /// Signed in through the browser. Every site one Atlassian account reaches
    /// shares its sign-in, and requests go through Atlassian's API by the site's id.
    #[serde(rename_all = "camelCase")]
    Atlassian { cloud_id: String },
}

impl Site {
    /// Several sites from one browser sign-in share one Keychain entry.
    pub fn keychain_account(&self) -> String {
        match &self.sign_in {
            SignIn::Token { email } => format!("{}:{email}", self.host),
            SignIn::Atlassian { .. } => format!("atlassian:{}", self.account_id),
        }
    }

    pub fn shares_sign_in_with(&self, other: &Site) -> bool {
        self.keychain_account() == other.keychain_account()
    }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JiraConfig {
    #[serde(default)]
    pub sites: Vec<Site>,
    /// The site used when a call names none.
    #[serde(default)]
    pub default: Option<String>,
}

impl store::Account for Site {
    fn id(&self) -> &str {
        &self.host
    }

    fn is_named(&self, name: &str) -> bool {
        self.host == normalise_host(name)
    }
}

impl Accounts for JiraConfig {
    type Account = Site;

    fn accounts(&self) -> &[Site] {
        &self.sites
    }
    fn accounts_mut(&mut self) -> &mut Vec<Site> {
        &mut self.sites
    }
    fn default_id(&self) -> Option<&str> {
        self.default.as_deref()
    }
    fn default_id_mut(&mut self) -> &mut Option<String> {
        &mut self.default
    }
}

impl JiraConfig {
    /// The site named by host, or with none named, the default one.
    pub fn site(&self, host: Option<&str>) -> JiraResult<&Site> {
        if let Some(site) = self.account(host) {
            return Ok(site);
        }
        let wanted = host.map(normalise_host).or_else(|| self.default.clone());
        match wanted {
            Some(wanted) if !self.sites.is_empty() => Err(JiraError::NotFound(format!(
                "jira: {wanted} is not signed in"
            ))),
            _ => Err(JiraError::Unconfigured),
        }
    }
}

/// `https://Acme.atlassian.net/jira/…` and `acme.atlassian.net` name the same site.
pub fn normalise_host(given: &str) -> String {
    let trimmed = given.trim();
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    url::Url::parse(&with_scheme)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default()
}

/// The token is only ever sent to a Jira Cloud site, whatever was pasted as its address.
pub fn cloud_host(given: &str) -> JiraResult<String> {
    let host = normalise_host(given);
    if host.ends_with(".atlassian.net") || host.ends_with(".jira.com") {
        Ok(host)
    } else {
        Err(JiraError::BadArg(
            "the site looks like acme.atlassian.net".into(),
        ))
    }
}

pub fn load(data_dir: &Path) -> JiraResult<JiraConfig> {
    Ok(store::load(data_dir)?)
}

pub fn keychain_read(site: &Site) -> JiraResult<Option<String>> {
    Ok(sikemux_keychain::read(
        TOKEN_SERVICE,
        &site.keychain_account(),
    )?)
}

pub fn keychain_write(site: &Site, token: &str) -> JiraResult<()> {
    Ok(sikemux_keychain::write(
        TOKEN_SERVICE,
        &site.keychain_account(),
        token,
    )?)
}

pub fn keychain_delete(site: &Site) -> JiraResult<()> {
    Ok(sikemux_keychain::delete(
        TOKEN_SERVICE,
        &site.keychain_account(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(host: &str) -> Site {
        Site {
            host: host.into(),
            account_id: "1".into(),
            display_name: None,
            sign_in: SignIn::Token {
                email: "me@example.com".into(),
            },
        }
    }

    #[test]
    fn a_site_is_named_by_its_host_however_it_is_written() {
        assert_eq!(
            normalise_host("https://Acme.atlassian.net/jira/software"),
            "acme.atlassian.net"
        );
        assert_eq!(normalise_host(" acme.atlassian.net "), "acme.atlassian.net");
    }

    #[test]
    fn only_jira_cloud_sites_are_accepted() {
        assert_eq!(
            cloud_host("https://acme.atlassian.net:8443/jira").ok(),
            Some("acme.atlassian.net".into())
        );
        for given in [
            "acme.atlassian.net@evil.com",
            "evil.com/acme.atlassian.net",
            "evil.com#.atlassian.net",
            "evilatlassian.net",
            "localhost:9000",
            "10.0.0.5",
            "",
        ] {
            assert!(cloud_host(given).is_err(), "{given}");
        }
    }

    #[test]
    fn sites_from_one_browser_sign_in_share_it_and_token_sites_keep_their_own() {
        let atlassian = |host: &str, account: &str| Site {
            host: host.into(),
            account_id: account.into(),
            display_name: None,
            sign_in: SignIn::Atlassian {
                cloud_id: format!("{host}-id"),
            },
        };
        let acme = atlassian("acme.atlassian.net", "me");
        assert!(acme.shares_sign_in_with(&atlassian("acme-dev.atlassian.net", "me")));
        assert!(!acme.shares_sign_in_with(&atlassian("other.atlassian.net", "someone-else")));
        assert!(!site("a.atlassian.net").shares_sign_in_with(&site("b.atlassian.net")));
    }

    #[test]
    fn a_site_is_found_however_its_host_is_written_and_one_not_signed_in_is_said_so() {
        let mut config = JiraConfig::default();
        assert!(matches!(config.site(None), Err(JiraError::Unconfigured)));
        config.upsert(site("a.atlassian.net"));
        config.upsert(site("b.atlassian.net"));
        assert_eq!(
            config.site(None).map(|site| site.host.clone()).ok(),
            Some("a.atlassian.net".into())
        );
        assert_eq!(
            config
                .site(Some("https://B.atlassian.net"))
                .map(|site| site.host.clone())
                .ok(),
            Some("b.atlassian.net".into())
        );
        assert!(matches!(
            config.site(Some("c.atlassian.net")),
            Err(JiraError::NotFound(_))
        ));
    }
}
