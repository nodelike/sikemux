// Signing in through the browser with an Atlassian account. Atlassian has no
// flow for apps that cannot keep a secret, so the client secret is compiled in,
// as Bitbucket's is; PKCE is what keeps a code caught on its way back from being
// traded by anyone else.

use serde::Deserialize;
use sikemux_plugin_api::oauth::{
    Authorize, KeychainEntry, OAuthApp, Refreshing, Secret, TokenRequest,
};

use crate::client;
use crate::config::{self, Site};
use crate::error::{Jira, JiraError, JiraResult};

const SITES_URL: &str = "https://api.atlassian.com/oauth/token/accessible-resources";
/// A site signed in through the browser is reached here, by its id, rather than at its own host.
pub const API_URL: &str = "https://api.atlassian.com/ex/jira";
/// Boards are Jira Software's API, which a browser sign-in reaches only through its own scopes.
const SCOPES: &str = "read:jira-work write:jira-work read:jira-user \
    read:board-scope:jira-software read:board-scope.admin:jira-software \
    read:sprint:jira-software read:issue:jira-software read:project:jira \
    read:issue-details:jira read:jql:jira read:filter:jira offline_access";

pub static APP: OAuthApp = OAuthApp {
    callback: "/jira/callback",
    client_id: match option_env!("JIRA_OAUTH_KEY") {
        Some(key) => key,
        None => "aJY3c50sMiGWv0fJpveLHOJ5Os2Z7rkt",
    },
    secret: Secret::Compiled(option_env!("JIRA_OAUTH_SECRET")),
    authorize_url: "https://auth.atlassian.com/authorize",
    authorize: Authorize::Code(&[
        ("audience", "api.atlassian.com"),
        ("scope", SCOPES),
        ("prompt", "consent"),
    ]),
    pkce: true,
    token_url: "https://auth.atlassian.com/oauth/token",
    token_request: TokenRequest::Json,
};

/// Every site one Atlassian account reaches shares its sign-in, so access is kept by account.
pub static ACCESS: Refreshing<Jira> = Refreshing::new(&APP, &client::HTTP);

pub fn entry(site: &Site) -> KeychainEntry {
    KeychainEntry {
        secrets: config::KEYCHAIN,
        account: site.keychain_account(),
    }
}

/// A Jira site the signed-in account may use.
#[derive(Deserialize)]
pub struct CloudSite {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// The Jira sites one browser sign-in reaches. Other Atlassian products, such as
/// Confluence, come back too, without Jira's scopes.
pub async fn sites(access_token: &str) -> JiraResult<Vec<CloudSite>> {
    let request = client::HTTP
        .client()?
        .get(SITES_URL)
        .bearer_auth(access_token)
        .header("Accept", "application/json");
    let response = client::HTTP.send(request).await?;
    let status = response.status();
    let bytes = client::read_limited(response).await?;
    if !status.is_success() {
        return Err(JiraError::Auth(format!(
            "Atlassian would not list the sites (http {status})"
        )));
    }
    Ok(jira_sites(serde_json::from_slice(&bytes)?))
}

fn jira_sites(all: Vec<CloudSite>) -> Vec<CloudSite> {
    all.into_iter()
        .filter(|site| site.scopes.iter().any(|scope| scope == "read:jira-work"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_browser_asks_for_jira_and_a_refresh_token_and_comes_back_to_jiras_path() {
        let url = APP.authorize_address("s1", Some("verifier"));
        assert!(url.starts_with("https://auth.atlassian.com/authorize?"));
        assert!(url.contains("&audience=api.atlassian.com&"));
        assert!(url.contains("scope=read%3Ajira-work+write%3Ajira-work+read%3Ajira-user+"));
        assert!(url.contains("+read%3Aboard-scope%3Ajira-software+"));
        assert!(url.contains("+offline_access&"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A47123%2Fjira%2Fcallback"));
        assert!(url.contains("&state=s1&"));
        assert!(url.ends_with("&code_challenge_method=S256"));
    }

    #[test]
    fn only_sites_that_grant_jira_are_kept() -> JiraResult<()> {
        let all: Vec<CloudSite> = serde_json::from_value(json!([
            { "id": "1", "url": "https://acme.atlassian.net", "name": "acme", "scopes": ["read:jira-work", "write:jira-work"] },
            { "id": "2", "url": "https://acme.atlassian.net", "name": "acme", "scopes": ["read:confluence-content.all"] }
        ]))?;
        let kept: Vec<_> = jira_sites(all).into_iter().map(|site| site.id).collect();
        assert_eq!(kept, ["1"]);
        Ok(())
    }
}
