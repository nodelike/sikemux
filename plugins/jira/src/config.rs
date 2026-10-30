// The Jira Cloud sites signed in here. A site's API token, or the Atlassian
// sign-in it came from, lives in the Keychain; the file beside it only lists the
// sites, who is signed in to each, and which one to use when none is named.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::store::{self, Accounts, Secrets};
use sikemux_plugin_api::{accounts, keychain_service};

use crate::error::{JiraError, JiraResult};

/// Each site's API token or refresh token, under `Site::keychain_account`.
pub const KEYCHAIN: Secrets = Secrets::new(keychain_service!("sikemux-jira-token"));

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

accounts!(JiraConfig, Site, sites, default);

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

/// An issue a web address names on a site signed in here: `/browse/KEY-1`, or a board or
/// list with `selectedIssue=KEY-1`.
#[derive(serde::Serialize)]
pub struct IssueLink {
    pub site: String,
    pub key: String,
}

pub fn issue_link(data_dir: &Path, address: &str) -> Option<IssueLink> {
    let url = url::Url::parse(address).ok()?;
    let site = url.host_str()?.to_ascii_lowercase();
    if !load(data_dir)
        .ok()?
        .sites
        .iter()
        .any(|known| known.host == site)
    {
        return None;
    }
    let browsed = url
        .path_segments()?
        .skip_while(|part| *part != "browse")
        .nth(1)
        .map(str::to_owned);
    let selected = url
        .query_pairs()
        .find(|(name, _)| name == "selectedIssue")
        .map(|(_, key)| key.into_owned());
    let key = browsed.or(selected).filter(|key| is_issue_key(key))?;
    Some(IssueLink { site, key })
}

/// Whether `key` is an issue's key, such as `ACME-12`.
fn is_issue_key(key: &str) -> bool {
    let Some((space, number)) = key.split_once('-') else {
        return false;
    };
    space.starts_with(|letter: char| letter.is_ascii_uppercase())
        && space
            .chars()
            .all(|letter| letter.is_ascii_uppercase() || letter.is_ascii_digit() || letter == '_')
        && !number.is_empty()
        && number.chars().all(|digit| digit.is_ascii_digit())
}

pub fn load(data_dir: &Path) -> JiraResult<JiraConfig> {
    Ok(store::load(data_dir)?)
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
