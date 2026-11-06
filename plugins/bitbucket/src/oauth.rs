// Signing in through the browser. Bitbucket has no flow for apps that cannot
// keep a secret, so the client secret is compiled in, as every desktop
// Bitbucket client does.

use sikemux_plugin_api::oauth::{Authorize, OAuthApp, Refreshing, Secret, TokenRequest};

use crate::client::HTTP;
use crate::config;
use crate::error::Bitbucket;

pub static APP: OAuthApp = OAuthApp {
    callback: "/bitbucket/callback",
    client_id: match option_env!("BITBUCKET_OAUTH_KEY") {
        Some(key) => key,
        None => "lTLK4yE705mh4VJyRvmmB9RyIz7RlFsm",
    },
    secret: Secret::Compiled(option_env!("BITBUCKET_OAUTH_SECRET")),
    authorize_url: "https://bitbucket.org/site/oauth2/authorize",
    authorize: Authorize::Code(&[]),
    pkce: false,
    token_url: "https://bitbucket.org/site/oauth2/access_token",
    token_request: TokenRequest::BasicAuthForm,
};

pub static ACCESS: Refreshing<Bitbucket> = Refreshing::new(&APP, &HTTP, config::KEYCHAIN);
