// Signing in through the browser. Bitbucket sends the browser back to a fixed
// address on this machine with a one-time code, which is traded for tokens.
// Bitbucket has no flow for apps that cannot keep a secret, so the client
// secret is compiled in, as every desktop Bitbucket client does.

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::client;
use crate::error::{BitbucketError, BitbucketResult};

const CLIENT_ID: &str = match option_env!("BITBUCKET_OAUTH_KEY") {
    Some(key) => key,
    None => "lTLK4yE705mh4VJyRvmmB9RyIz7RlFsm",
};
const CLIENT_SECRET: Option<&str> = option_env!("BITBUCKET_OAUTH_SECRET");
/// Registered as the client's callback, so it cannot move without changing it there too.
const PORT: u16 = 47123;
const CALLBACK_PATH: &str = "/callback";
const AUTHORIZE_URL: &str = "https://bitbucket.org/site/oauth2/authorize";
const TOKEN_URL: &str = "https://bitbucket.org/site/oauth2/access_token";
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub fn available() -> bool {
    CLIENT_SECRET.is_some_and(|secret| !secret.is_empty())
}

fn secret() -> BitbucketResult<&'static str> {
    CLIENT_SECRET
        .filter(|secret| !secret.is_empty())
        .ok_or_else(|| {
            BitbucketError::Auth(
                "this build cannot sign in through the browser; use an API token instead".into(),
            )
        })
}

/// A value the browser has to hand back unchanged, so a code this app never
/// asked for is turned away.
pub fn new_state() -> BitbucketResult<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| BitbucketError::Transport(format!("no randomness: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn authorize_url(state: &str) -> String {
    format!("{AUTHORIZE_URL}?client_id={CLIENT_ID}&response_type=code&state={state}")
}

/// Listens on both loopback addresses, since a browser may try `localhost`
/// as either. Bound before the browser is opened, so the answer cannot arrive first.
pub struct Callback {
    v4: TcpListener,
    v6: Option<TcpListener>,
}

impl Callback {
    pub async fn bind() -> BitbucketResult<Self> {
        let v4 = TcpListener::bind(("127.0.0.1", PORT)).await.map_err(|_| {
            BitbucketError::Auth(format!(
                "port {PORT} is taken by another app, and Bitbucket can only send the browser back there"
            ))
        })?;
        let v6 = TcpListener::bind(("::1", PORT)).await.ok();
        Ok(Self { v4, v6 })
    }

    async fn accept(&self) -> std::io::Result<TcpStream> {
        match &self.v6 {
            Some(v6) => tokio::select! {
                accepted = self.v4.accept() => accepted.map(|(stream, _)| stream),
                accepted = v6.accept() => accepted.map(|(stream, _)| stream),
            },
            None => self.v4.accept().await.map(|(stream, _)| stream),
        }
    }

    /// Waits for the browser to come back with a code. Anything else that
    /// reaches the port, such as a request for a favicon, is answered and ignored.
    pub async fn code(&self, state: &str) -> BitbucketResult<String> {
        loop {
            let Ok(mut stream) = self.accept().await else {
                continue;
            };
            let Ok(Ok(target)) =
                tokio::time::timeout(REQUEST_TIMEOUT, request_target(&mut stream)).await
            else {
                continue;
            };
            match answer_of(&target, state) {
                None => respond(&mut stream, "404 Not Found", "Nothing here.").await,
                Some(Ok(code)) => {
                    respond(
                        &mut stream,
                        "200 OK",
                        "Signed in to Bitbucket. You can close this tab and go back to Sikemux.",
                    )
                    .await;
                    return Ok(code);
                }
                Some(Err(error)) => {
                    respond(&mut stream, "400 Bad Request", &error.to_string()).await;
                    return Err(error);
                }
            }
        }
    }
}

async fn request_target(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await?;
        if read == 0 || buffer.len() + read > MAX_REQUEST_BYTES {
            break;
        }
        buffer.extend(chunk.iter().take(read));
    }
    let head = String::from_utf8_lossy(&buffer);
    let line = head.lines().next().unwrap_or_default();
    let mut parts = line.split(' ');
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) => Ok(target.to_string()),
        _ => Ok(String::new()),
    }
}

/// What a request to the callback says: `None` when it is not the callback at all.
fn answer_of(target: &str, state: &str) -> Option<BitbucketResult<String>> {
    let url = url::Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != CALLBACK_PATH {
        return None;
    }
    let value = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if let Some(error) = value("error") {
        let reason = value("error_description").unwrap_or(error);
        return Some(Err(BitbucketError::Auth(reason)));
    }
    if value("state").as_deref() != Some(state) {
        return Some(Err(BitbucketError::Auth(
            "the browser came back from a sign-in this app did not start".into(),
        )));
    }
    Some(value("code").ok_or_else(|| BitbucketError::Auth("Bitbucket sent no code".into())))
}

async fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Sikemux</title><body style=\"font:15px -apple-system,sans-serif;padding:48px\">{}</body>",
        message.replace('&', "&amp;").replace('<', "&lt;")
    );
    let reply = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(reply.as_bytes()).await.ok();
    stream.shutdown().await.ok();
}

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default = "default_lifetime")]
    pub expires_in: u64,
    #[serde(default)]
    pub scopes: String,
}

fn default_lifetime() -> u64 {
    3600
}

#[derive(Deserialize)]
struct Refused {
    error: Option<String>,
    error_description: Option<String>,
}

async fn token_request(form: &[(&str, &str)]) -> BitbucketResult<Tokens> {
    let response = client::limited(
        client::http()?
            .post(TOKEN_URL)
            .basic_auth(CLIENT_ID, Some(secret()?))
            .form(form)
            .send(),
    )
    .await?;
    let status = response.status();
    let (bytes, _) = client::read_body(response, 64 * 1024, false).await?;
    if status.is_success() {
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let refused: Option<Refused> = serde_json::from_slice(&bytes).ok();
    let reason = refused
        .and_then(|refused| refused.error_description.or(refused.error))
        .unwrap_or_else(|| format!("http {status}"));
    Err(BitbucketError::Auth(reason))
}

pub async fn exchange(code: &str) -> BitbucketResult<Tokens> {
    token_request(&[("grant_type", "authorization_code"), ("code", code)]).await
}

pub async fn refresh(refresh_token: &str) -> BitbucketResult<Tokens> {
    token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ])
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_callback_carries_the_code_when_the_state_matches() {
        let answer = answer_of("/callback?code=abc&state=s1", "s1");
        assert_eq!(answer.and_then(Result::ok).as_deref(), Some("abc"));
    }

    #[test]
    fn a_code_for_someone_elses_sign_in_is_refused() {
        let answer = answer_of("/callback?code=abc&state=other", "s1");
        assert!(matches!(answer, Some(Err(BitbucketError::Auth(_)))));
    }

    #[test]
    fn saying_no_in_the_browser_reads_as_a_refusal() {
        let answer = answer_of(
            "/callback?error=access_denied&error_description=The+user+denied",
            "s1",
        );
        match answer {
            Some(Err(BitbucketError::Auth(reason))) => assert_eq!(reason, "The user denied"),
            _ => panic!("expected a refusal"),
        }
    }

    #[test]
    fn other_requests_to_the_port_are_not_the_callback() {
        assert!(answer_of("/favicon.ico", "s1").is_none());
        assert!(answer_of("", "s1").is_none());
    }

    #[test]
    fn every_sign_in_asks_with_a_fresh_state() -> BitbucketResult<()> {
        let first = new_state()?;
        assert_eq!(first.len(), 32);
        assert_ne!(first, new_state()?);
        assert!(authorize_url(&first).ends_with(&format!("state={first}")));
        Ok(())
    }
}
