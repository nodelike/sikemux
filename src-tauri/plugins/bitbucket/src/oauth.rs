// Signing in through the browser. Bitbucket has no flow for apps that cannot
// keep a secret, so the client secret is compiled in, as every desktop
// Bitbucket client does.

use sikemux_plugin_api::oauth::{
    Authorize, KeychainEntry, OAuthApp, Refreshing, Secret, TokenRequest,
};

use crate::client::HTTP;
use crate::config::{self, Account};
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

pub static ACCESS: Refreshing<Bitbucket> = Refreshing::new(&APP, &HTTP);

pub fn entry(account: &Account) -> KeychainEntry {
    KeychainEntry {
        service: config::TOKEN_SERVICE,
        account: account.keychain_account(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_is_sent_back_to_bitbucket_s_own_path() {
        let url = APP.authorize_address("s1", None);
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A47123%2Fbitbucket%2Fcallback"));
        assert!(url.ends_with("state=s1"));
    }
}
