//! Signing in through the browser. The service sends the browser back to this
//! machine with a one-time code, traded for tokens; an access token that runs
//! out is renewed with the refresh token kept in the Keychain.

use std::collections::BTreeMap;
use std::future::Future;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_loopback::{challenge, redirect_uri, Callback, LoopbackError};

use crate::http::{read_body, Http};
use crate::service::{Service, ServiceError};
use crate::store::blocking;
use crate::StreamSink;

/// Long enough to sign up or find a password, short enough that a forgotten
/// sign-in does not hold the shared port all day.
const BROWSER_WAIT: Duration = Duration::from_secs(10 * 60);
/// Renew a little before the service would refuse the token, not after.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);
const MAX_TOKEN_ANSWER: usize = 64 * 1024;

pub enum Secret {
    /// The service takes PKCE alone, so no secret is compiled in.
    None,
    /// The service has no flow for apps that cannot keep a secret, so the
    /// secret is compiled in when the build was given one.
    Compiled(Option<&'static str>),
}

pub enum Authorize {
    /// The usual request: the client, the callback, a code back, and these.
    Code(&'static [(&'static str, &'static str)]),
    /// A page that knows the app already, such as an integration's install
    /// page, and needs only the state.
    Install,
}

pub enum TokenRequest {
    Form,
    /// A form, with the client and its secret as basic auth.
    BasicAuthForm,
    Json,
}

/// An app registered with a service, and how that service wants to be asked.
pub struct OAuthApp {
    /// Registered as the app's callback, so it cannot move without changing it there too.
    pub callback: &'static str,
    pub client_id: &'static str,
    pub secret: Secret,
    pub authorize_url: &'static str,
    pub authorize: Authorize,
    pub pkce: bool,
    pub token_url: &'static str,
    pub token_request: TokenRequest,
}

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default = "an_hour")]
    pub expires_in: u64,
    /// What the grant allows, for a service that says.
    #[serde(default)]
    pub scopes: String,
}

fn an_hour() -> u64 {
    3600
}

fn refused<S>(error: LoopbackError) -> ServiceError<S> {
    ServiceError::Auth(error.0)
}

/// What a service said when it would not hand out tokens.
fn refusal(bytes: &[u8]) -> Option<String> {
    let said: Value = serde_json::from_slice(bytes).ok()?;
    ["/error_description", "/error/message", "/error"]
        .iter()
        .find_map(|path| said.pointer(path).and_then(Value::as_str))
        .map(str::to_owned)
}

impl OAuthApp {
    /// A build without the secret a service needs can only take a pasted token.
    pub fn available(&self) -> bool {
        match self.secret {
            Secret::None => true,
            Secret::Compiled(secret) => secret.is_some_and(|secret| !secret.is_empty()),
        }
    }

    fn secret<S>(&self) -> Result<Option<&'static str>, ServiceError<S>> {
        match self.secret {
            Secret::None => Ok(None),
            Secret::Compiled(secret) => secret
                .filter(|secret| !secret.is_empty())
                .map(Some)
                .ok_or_else(|| {
                    ServiceError::Auth(
                        "this build cannot sign in through the browser; paste a token instead"
                            .into(),
                    )
                }),
        }
    }

    pub fn redirect_uri(&self) -> String {
        redirect_uri(self.callback)
    }

    pub fn authorize_address(&self, state: &str, verifier: Option<&str>) -> String {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        if let Authorize::Code(params) = self.authorize {
            query
                .append_pair("client_id", self.client_id)
                .append_pair("redirect_uri", &self.redirect_uri())
                .append_pair("response_type", "code")
                .extend_pairs(params.iter());
        }
        query.append_pair("state", state);
        if let Some(verifier) = verifier {
            query
                .append_pair("code_challenge", &challenge(verifier))
                .append_pair("code_challenge_method", "S256");
        }
        format!("{}?{}", self.authorize_url, query.finish())
    }

    /// Opens the service's sign-in in the browser, through the address sent
    /// down the stream, and trades the code it comes back with. Stopping the
    /// stream stops the wait.
    pub async fn sign_in<S: Service, T: DeserializeOwned>(
        &self,
        http: &Http<S>,
        sink: &StreamSink,
    ) -> Result<T, ServiceError<S>> {
        self.secret()?;
        let callback = Callback::bind(self.callback, S::NAME)
            .await
            .map_err(refused)?;
        let state = sikemux_loopback::new_state().map_err(refused)?;
        let verifier = match self.pkce {
            true => Some(sikemux_loopback::new_verifier().map_err(refused)?),
            false => None,
        };
        sink.send(json!({ "url": self.authorize_address(&state, verifier.as_deref()) }))
            .map_err(|_| ServiceError::Auth("the sign-in was closed".into()))?;
        let code = tokio::time::timeout(BROWSER_WAIT, callback.code(&state))
            .await
            .map_err(|_| ServiceError::Auth("nobody finished signing in".into()))?
            .map_err(refused)?;
        drop(callback);
        let redirect = self.redirect_uri();
        let mut fields = vec![("code", code.as_str()), ("redirect_uri", &redirect)];
        if let Authorize::Code(_) = self.authorize {
            fields.push(("grant_type", "authorization_code"));
        }
        if let Some(verifier) = &verifier {
            fields.push(("code_verifier", verifier));
        }
        self.token_request(http, &fields).await
    }

    /// Services hand back a new refresh token every time, and the old one stops working.
    pub async fn refresh<S: Service>(
        &self,
        http: &Http<S>,
        refresh_token: &str,
    ) -> Result<Tokens, ServiceError<S>> {
        self.token_request(
            http,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ],
        )
        .await
    }

    async fn token_request<S: Service, T: DeserializeOwned>(
        &self,
        http: &Http<S>,
        fields: &[(&str, &str)],
    ) -> Result<T, ServiceError<S>> {
        let secret = self.secret()?;
        let mut fields = fields.to_vec();
        let client = http.client()?.post(self.token_url);
        let request = match self.token_request {
            TokenRequest::BasicAuthForm => client.basic_auth(self.client_id, secret),
            TokenRequest::Form | TokenRequest::Json => {
                fields.push(("client_id", self.client_id));
                if let Some(secret) = secret {
                    fields.push(("client_secret", secret));
                }
                client
            }
        };
        let request = match self.token_request {
            TokenRequest::Json => {
                let body: Map<String, Value> = fields
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), Value::from(*value)))
                    .collect();
                request
                    .header("Content-Type", "application/json")
                    .body(Value::Object(body).to_string())
            }
            TokenRequest::Form | TokenRequest::BasicAuthForm => request
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(fields)
                        .finish(),
                ),
        };
        let response = http.send(request).await?;
        let status = response.status();
        let bytes = read_body(response, MAX_TOKEN_ANSWER).await?;
        if status.is_success() {
            return Ok(serde_json::from_slice(&bytes)?);
        }
        Err(ServiceError::Auth(
            refusal(&bytes).unwrap_or_else(|| format!("http {status}")),
        ))
    }
}

/// Where a sign-in's refresh token is kept.
pub struct KeychainEntry {
    pub service: &'static str,
    pub account: String,
}

#[derive(Clone)]
pub struct Access {
    pub token: String,
    pub scopes: Vec<String>,
    expires: Instant,
}

impl Access {
    fn of(tokens: &Tokens) -> Self {
        Self {
            token: tokens.access_token.clone(),
            scopes: tokens
                .scopes
                .split([' ', ','])
                .filter(|scope| !scope.is_empty())
                .map(str::to_owned)
                .collect(),
            expires: Instant::now() + Duration::from_secs(tokens.expires_in),
        }
    }
}

/// The access tokens of every browser sign-in to one service, by account.
pub struct Refreshing<S: 'static> {
    app: &'static OAuthApp,
    http: &'static Http<S>,
    held: tokio::sync::Mutex<BTreeMap<String, Access>>,
}

impl<S: Service> Refreshing<S> {
    pub const fn new(app: &'static OAuthApp, http: &'static Http<S>) -> Self {
        Self {
            app,
            http,
            held: tokio::sync::Mutex::const_new(BTreeMap::new()),
        }
    }

    pub async fn remember(&self, account: &str, tokens: &Tokens) {
        self.held
            .lock()
            .await
            .insert(account.to_owned(), Access::of(tokens));
    }

    pub async fn forget(&self, account: Option<&str>) {
        let mut held = self.held.lock().await;
        match account {
            Some(account) => {
                held.remove(account);
            }
            None => held.clear(),
        }
    }

    /// One refresh at a time: the lock is held across it, because spending a
    /// refresh token twice would sign the account out.
    pub async fn access(
        &self,
        account: &str,
        entry: KeychainEntry,
    ) -> Result<Access, ServiceError<S>> {
        let mut held = self.held.lock().await;
        if let Some(access) = held
            .get(account)
            .filter(|access| access.expires > Instant::now() + EXPIRY_MARGIN)
        {
            return Ok(access.clone());
        }
        let KeychainEntry {
            service,
            account: entry_account,
        } = entry;
        let reading = entry_account.clone();
        let refresh_token = blocking(move || Ok(sikemux_keychain::read(service, &reading)?))
            .await?
            .ok_or(ServiceError::Unconfigured)?;
        let tokens = self.app.refresh(self.http, &refresh_token).await?;
        if tokens.refresh_token != refresh_token {
            let replacement = tokens.refresh_token.clone();
            blocking(move || {
                Ok(sikemux_keychain::write(
                    service,
                    &entry_account,
                    &replacement,
                )?)
            })
            .await?;
        }
        let access = Access::of(&tokens);
        held.insert(account.to_owned(), access.clone());
        Ok(access)
    }

    /// The service refused `rejected`, so it is dropped and a fresh one
    /// fetched. A request that renewed it meanwhile is answered with that one
    /// rather than refreshing again.
    pub async fn renew(
        &self,
        account: &str,
        rejected: &str,
        entry: KeychainEntry,
    ) -> Result<Access, ServiceError<S>> {
        {
            let mut held = self.held.lock().await;
            if held
                .get(account)
                .is_some_and(|access| access.token == rejected)
            {
                held.remove(account);
            }
        }
        self.access(account, entry).await
    }

    /// Sends with the access token, and when the service refuses it, renews
    /// it and sends once more.
    pub async fn with_access<T, F, Fut>(
        &self,
        account: &str,
        entry: impl Fn() -> KeychainEntry,
        send: F,
    ) -> Result<T, ServiceError<S>>
    where
        F: Fn(Access) -> Fut,
        Fut: Future<Output = Result<T, ServiceError<S>>>,
    {
        let access = self.access(account, entry()).await?;
        let rejected = access.token.clone();
        match send(access).await {
            Err(ServiceError::Auth(_)) => {
                send(self.renew(account, &rejected, entry()).await?).await
            }
            answered => answered,
        }
    }
}
