use sikemux_plugin_api::http;
use sikemux_plugin_api::oauth::{Authorize, OAuthApp, Refreshing, Secret, TokenRequest};

use crate::client::HTTP;
use crate::config;
use crate::error::{Cloudflare, CloudflareResult};

const REVOKE_URL: &str = "https://dash.cloudflare.com/oauth2/revoke";

pub const SCOPES: &str = "user.read account.read zone.read zone.write dns.read dns.write \
    cache_purge.write workers_scripts.read workers_routes.read pages.read offline_access";

pub static APP: OAuthApp = OAuthApp {
    icon: include_str!("../../../app/assets/icons/cloudflare.svg"),
    callback: "/cloudflare/callback",
    client_id: match option_env!("CLOUDFLARE_OAUTH_CLIENT_ID") {
        Some(id) => id,
        None => "",
    },
    secret: Secret::None,
    authorize_url: "https://dash.cloudflare.com/oauth2/auth",
    authorize: Authorize::Code(&[("scope", SCOPES)]),
    pkce: true,
    token_url: "https://dash.cloudflare.com/oauth2/token",
    token_request: TokenRequest::Form,
};

pub static ACCESS: Refreshing<Cloudflare> = Refreshing::new(&APP, &HTTP, config::KEYCHAIN);

async fn post_revoke(refresh_token: &str) -> CloudflareResult<()> {
    if APP.client_id.is_empty() {
        return Ok(());
    }
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("client_id", APP.client_id),
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

pub async fn revoke(refresh_token: &str) {
    if let Err(error) = post_revoke(refresh_token).await {
        eprintln!("cloudflare: revoking the sign-in: {error}");
    }
}

pub static BOOK: sikemux_plugin_api::vault::Listed<config::Account> =
    sikemux_plugin_api::vault::Listed::new(
        "config.json",
        "accounts",
        "id",
        Some("default"),
        config::KEYCHAIN,
    )
    .renewed_by(&ACCESS);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_uses_pkce_and_cloudflare_callback() -> Result<(), String> {
        let address = APP.authorize_address("state-1", Some("verifier-1"));
        let url = url::Url::parse(&address).map_err(|error| error.to_string())?;
        assert_eq!(url.host_str(), Some("dash.cloudflare.com"));
        assert_eq!(url.path(), "/oauth2/auth");
        let param = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default()
        };
        assert_eq!(param("response_type"), "code");
        assert_eq!(param("code_challenge_method"), "S256");
        assert!(param("redirect_uri").ends_with("/cloudflare/callback"));
        assert!(param("scope")
            .split(' ')
            .any(|scope| scope == "offline_access"));
        assert!(!param("scope").contains(':'));
        Ok(())
    }

    #[test]
    fn tokens_parse_singular_cloudflare_scope_field() -> Result<(), String> {
        let tokens: sikemux_plugin_api::oauth::Tokens = serde_json::from_str(
            r#"{"access_token":"at","refresh_token":"rt","expires_in":3600,"scope":"zone.read dns.write offline_access"}"#,
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(tokens.scopes, "zone.read dns.write offline_access");
        Ok(())
    }
}
