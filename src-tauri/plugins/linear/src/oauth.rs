// Signing in through the browser with a Linear account. The app proves it asked
// for the code with PKCE, so no client secret is compiled in.

use sikemux_plugin_api::http;
use sikemux_plugin_api::oauth::{
    Authorize, KeychainEntry, OAuthApp, Refreshing, Secret, TokenRequest,
};

use crate::client::HTTP;
use crate::config::{self, Workspace};
use crate::error::{Linear, LinearResult};

const REVOKE_URL: &str = "https://api.linear.app/oauth/revoke";

pub static APP: OAuthApp = OAuthApp {
    callback: "/linear/callback",
    client_id: "a597de542170c3976bde9c670f879ee7",
    secret: Secret::None,
    authorize_url: "https://linear.app/oauth/authorize",
    authorize: Authorize::Code(&[("scope", "read,write")]),
    pkce: true,
    token_url: "https://api.linear.app/oauth/token",
    token_request: TokenRequest::Form,
};

pub static ACCESS: Refreshing<Linear> = Refreshing::new(&APP, &HTTP);

pub fn entry(workspace: &Workspace) -> KeychainEntry {
    KeychainEntry {
        service: config::KEY_SERVICE,
        account: workspace.id.clone(),
    }
}

pub fn bearer(access_token: &str) -> String {
    format!("Bearer {access_token}")
}

async fn post_revoke(refresh_token: &str) -> LinearResult<()> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("token", refresh_token),
            ("token_type_hint", "refresh_token"),
        ])
        .finish();
    let response = HTTP
        .send(
            HTTP.client()?
                .post(REVOKE_URL)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body),
        )
        .await?;
    http::read_body(response, 64 * 1024).await?;
    Ok(())
}

/// Signing out tells Linear to forget the sign-in too; it is gone here either way.
pub async fn revoke(refresh_token: &str) {
    if let Err(error) = post_revoke(refresh_token).await {
        eprintln!("linear: revoking the sign-in: {error}");
    }
}
