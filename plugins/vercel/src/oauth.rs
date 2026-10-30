// Signing in through the browser by installing the Sikemux integration. Vercel asks
// which team or account it goes on and which projects it may see, then sends the
// browser back with a code, traded for a token that does not expire.

use serde::Deserialize;
use sikemux_plugin_api::oauth::{Authorize, OAuthApp, Secret, TokenRequest};

use crate::client;

pub static APP: OAuthApp = OAuthApp {
    callback: "/vercel/callback",
    client_id: "oac_ShIPirtpIwfcsyfvCWka3qNU",
    secret: Secret::Compiled(option_env!("VERCEL_OAUTH_SECRET")),
    authorize_url: "https://vercel.com/integrations/sikemux/new",
    authorize: Authorize::Install,
    pkce: false,
    token_url: "https://api.vercel.com/v2/oauth/access_token",
    token_request: TokenRequest::Form,
};

#[derive(Deserialize)]
pub struct Installed {
    pub access_token: String,
    pub installation_id: String,
    /// The team it was installed on; none for the account's own projects.
    pub team_id: Option<String>,
}

/// Signing out removes the integration from the team too; it is gone here either way.
pub async fn uninstall(token: &str, team: Option<&str>, configuration: &str) {
    let query: Vec<(&str, String)> = team.map(client::team_param).into_iter().collect();
    if let Err(error) = client::request(
        token,
        reqwest::Method::DELETE,
        &format!("/v1/integrations/configuration/{configuration}"),
        &query,
        None,
    )
    .await
    {
        eprintln!("vercel: removing the integration: {error}");
    }
}
