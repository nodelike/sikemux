// Signing in with a service account's key, and the access tokens that key mints:
// a JWT signed with the key is traded at Google's token endpoint for an hour's token.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde::{Deserialize, Serialize};
use sikemux_plugin_api::http::Read;
use sikemux_plugin_api::store::{self, Held};
use sikemux_plugin_api::{SignInStatus, SignedInAs};

use crate::client::HTTP;
use crate::config::{self, Account, PlayConfig, ACCOUNT_KEY, KEYCHAIN};
use crate::error::{PlayError, PlayResult};

const SCOPE: &str = "https://www.googleapis.com/auth/androidpublisher";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const JWT_BEARER: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer";
const MAX_KEY_FILE: u64 = 64 * 1024;

/// Google's tokens last an hour; one is used for 50 minutes, so none runs out mid-upload.
static TOKENS: Held<String> = Held::new(Duration::from_secs(50 * 60));
static KEYS: Held<String> = Held::secrets();

#[derive(Deserialize)]
struct ServiceAccount {
    #[serde(rename = "type")]
    kind: String,
    client_email: String,
    private_key: String,
    #[serde(default)]
    project_id: String,
}

impl ServiceAccount {
    fn parse(json: &str) -> PlayResult<Self> {
        let key: Self = serde_json::from_str(json).map_err(|_| not_a_key())?;
        if key.kind != "service_account" || key.client_email.is_empty() {
            return Err(not_a_key());
        }
        Ok(key)
    }

    fn assertion(&self) -> PlayResult<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({
            "iss": self.client_email,
            "scope": SCOPE,
            "aud": TOKEN_URL,
            "iat": now,
            "exp": now + 3600,
        }))?);
        let signed = format!("{header}.{claims}");
        let der: String = self
            .private_key
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let der = STANDARD.decode(der.trim()).map_err(|_| not_a_key())?;
        let pair = RsaKeyPair::from_pkcs8(&der).map_err(|_| not_a_key())?;
        let mut signature = vec![0; pair.public().modulus_len()];
        pair.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            signed.as_bytes(),
            &mut signature,
        )
        .map_err(|_| PlayError::Auth("the key could not sign".into()))?;
        Ok(format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }

    async fn mint(&self) -> PlayResult<String> {
        #[derive(Deserialize)]
        struct Minted {
            access_token: String,
        }
        let request = HTTP
            .client()?
            .post(TOKEN_URL)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type={JWT_BEARER}&assertion={}",
                self.assertion()?
            ));
        let minted: Minted = HTTP
            .ask(request, Read::IDENTITY)
            .await?
            .succeeded()?
            .parsed()?;
        Ok(minted.access_token)
    }
}

fn not_a_key() -> PlayError {
    PlayError::BadArg(
        "that is not a service account key; make a JSON key for the service account in Google Cloud".into(),
    )
}

/// An access token for the account signed in.
pub async fn token(data_dir: &Path) -> PlayResult<String> {
    let account = config::load(data_dir)?
        .account
        .ok_or(PlayError::Unconfigured)?;
    if let Some(token) = TOKENS.get(&account.email) {
        return Ok(token);
    }
    let json = KEYS
        .secret(KEYCHAIN, ACCOUNT_KEY, ACCOUNT_KEY.into())
        .await?;
    let token = ServiceAccount::parse(&json)?.mint().await?;
    TOKENS.put(&account.email, token.clone());
    Ok(token)
}

#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Signed {
    pub account: Option<Account>,
}

impl SignedInAs for Signed {
    fn signed_in_as(&self) -> Option<String> {
        self.account.as_ref().map(|account| account.project.clone())
    }
}

pub type Status = SignInStatus<Signed>;

pub async fn status(data_dir: &Path) -> Status {
    let account = match config::load(data_dir) {
        Ok(config) => config.account,
        Err(error) => return Status::checked(false, Signed::default(), &Err::<(), _>(error)),
    };
    let check = token(data_dir).await;
    Status::checked(account.is_some(), Signed { account }, &check)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySignIn {
    pub key_file: String,
}

pub fn expand(typed: &str) -> PathBuf {
    let typed = typed.trim();
    match (typed.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(typed),
    }
}

/// Copies the key into the Keychain once it has minted a token, so a key Google turns away is never kept.
pub async fn sign_in(data_dir: &Path, input: KeySignIn) -> PlayResult<()> {
    let path = expand(&input.key_file);
    let read = tokio::fs::metadata(&path)
        .await
        .ok()
        .filter(|about| about.is_file() && about.len() <= MAX_KEY_FILE);
    if read.is_none() {
        return Err(PlayError::BadArg(format!(
            "no service account key at {}",
            path.display()
        )));
    }
    let json = tokio::fs::read_to_string(&path).await?;
    let key = ServiceAccount::parse(&json)?;
    let json = serde_json::to_string(&serde_json::from_str::<serde_json::Value>(&json)?)?;
    let token = key.mint().await?;
    let account = Account {
        email: key.client_email.clone(),
        project: match key.project_id.is_empty() {
            true => key
                .client_email
                .split('@')
                .next()
                .unwrap_or_default()
                .into(),
            false => key.project_id.clone(),
        },
    };
    TOKENS.put(&account.email, token);
    KEYS.forget(None);
    store::update(data_dir, move |config: &mut PlayConfig| {
        KEYCHAIN.write(ACCOUNT_KEY, &json)?;
        config.account = Some(account);
        Ok(())
    })
    .await
}

pub async fn sign_out(data_dir: &Path) -> PlayResult<()> {
    TOKENS.forget(None);
    KEYS.forget(None);
    store::update(data_dir, |config: &mut PlayConfig| {
        KEYCHAIN.delete(ACCOUNT_KEY)?;
        config.account = None;
        Ok(())
    })
    .await
}
