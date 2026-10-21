// Signing in through the browser. A service sends the browser back to a fixed
// address on this machine with a one-time code. Every plugin listens on the same
// port, each at a path of its own, so only one port has to stay free. A browser
// on another machine comes back to its own localhost instead, and the address it
// landed on is handed in with `relay`.

use std::fmt;
use std::io::ErrorKind;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Registered with each service as part of its callback, so it cannot move without changing them too.
pub const PORT: u16 = 47123;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
pub struct LoopbackError(pub String);

impl fmt::Display for LoopbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LoopbackError {}

pub type LoopbackResult<T> = Result<T, LoopbackError>;

/// Where the service sends the browser back to, for the callback at `path`.
pub fn redirect_uri(path: &str) -> String {
    format!("http://localhost:{PORT}{path}")
}

/// A value the browser has to hand back unchanged, so a code this app never
/// asked for is turned away.
pub fn new_state() -> LoopbackResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| LoopbackError(format!("no randomness: {error}")))?;
    Ok(hex::encode(bytes))
}

/// The PKCE secret the browser never sees: the service hands out tokens only to whoever holds it,
/// so a code caught on its way back is useless.
pub fn new_verifier() -> LoopbackResult<String> {
    Ok(format!("{}{}", new_state()?, new_state()?))
}

/// What the browser carries to the service in place of the verifier.
pub fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What the browser is shown once it came back.
pub enum Finish {
    /// A page saying the sign-in to this service is done.
    Page(&'static str),
    /// Sent on to `done`, or to `refused` when the sign-in was turned down.
    Redirect { done: String, refused: String },
}

/// Where the browser comes back to with a code.
pub struct Callback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    redirect_uri: String,
    path: &'static str,
    finish: Finish,
    relayed: Mutex<Option<UnboundedReceiver<String>>>,
}

/// The sign-ins waiting for a browser, by their callback path.
static WAITING: Mutex<Vec<(&'static str, UnboundedSender<String>)>> = Mutex::new(Vec::new());

fn waiting() -> std::sync::MutexGuard<'static, Vec<(&'static str, UnboundedSender<String>)>> {
    WAITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Hands the waiting sign-in the address a browser came back to elsewhere,
/// as a whole URL or its path and query.
pub fn relay(landed: &str) -> LoopbackResult<()> {
    let landed = landed.trim();
    let target = match url::Url::parse(landed) {
        Ok(url) => match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        },
        Err(_) => landed.to_owned(),
    };
    let path = target.split('?').next().unwrap_or_default();
    let waiting = waiting();
    let sender = waiting
        .iter()
        .find(|(waits_at, _)| *waits_at == path)
        .map(|(_, sender)| sender)
        .ok_or_else(|| LoopbackError("no sign-in is waiting for that address".into()))?;
    sender
        .send(target)
        .map_err(|_| LoopbackError("that sign-in stopped waiting".into()))
}

impl Callback {
    /// Listens on the shared port at both loopback addresses, since a browser
    /// may try `localhost` as either. Both are held before the browser is
    /// opened, so no other program can be listening on one of them for the code.
    pub async fn bind(path: &'static str, service: &'static str) -> LoopbackResult<Self> {
        let taken = || {
            LoopbackError(format!(
                "port {PORT} is taken, by another sign-in that is still waiting or by another app"
            ))
        };
        let v4 = TcpListener::bind(("127.0.0.1", PORT))
            .await
            .map_err(|_| taken())?;
        let v6 = match TcpListener::bind(("::1", PORT)).await {
            Ok(v6) => Some(v6),
            Err(error) if error.kind() == ErrorKind::AddrNotAvailable => None,
            Err(_) => return Err(taken()),
        };
        Ok(Self::waiting(
            v4,
            v6,
            redirect_uri(path),
            path,
            Finish::Page(service),
        ))
    }

    /// Listens on a port the system picks, for a service that takes any
    /// port on `127.0.0.1` in its redirect.
    pub async fn bind_any(path: &'static str, finish: Finish) -> LoopbackResult<Self> {
        let v4 = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| LoopbackError(format!("could not listen for the browser: {error}")))?;
        let port = v4
            .local_addr()
            .map_err(|error| LoopbackError(error.to_string()))?
            .port();
        let redirect = format!("http://127.0.0.1:{port}{path}");
        Ok(Self::waiting(v4, None, redirect, path, finish))
    }

    fn waiting(
        v4: TcpListener,
        v6: Option<TcpListener>,
        redirect_uri: String,
        path: &'static str,
        finish: Finish,
    ) -> Self {
        let (sender, relayed) = unbounded_channel();
        waiting().push((path, sender));
        Self {
            v4,
            v6,
            redirect_uri,
            path,
            finish,
            relayed: Mutex::new(Some(relayed)),
        }
    }

    /// Where the service sends the browser back to.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
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
    pub async fn code(&self, state: &str) -> LoopbackResult<String> {
        let mut relayed = self
            .relayed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(|| LoopbackError("this sign-in is already waiting".into()))?;
        loop {
            let accepted = tokio::select! {
                accepted = self.accept() => accepted,
                Some(target) = relayed.recv() => match answer_of(&target, self.path, state) {
                    Some(Answer::Done(answer)) => return answer,
                    Some(Answer::Stray) => return Err(not_this_sign_in()),
                    None => continue,
                },
            };
            let Ok(mut stream) = accepted else {
                continue;
            };
            let Ok(Ok(target)) =
                tokio::time::timeout(REQUEST_TIMEOUT, request_target(&mut stream)).await
            else {
                continue;
            };
            match answer_of(&target, self.path, state) {
                None => respond(&mut stream, "404 Not Found", "Nothing here.").await,
                Some(Answer::Stray) => {
                    respond(&mut stream, "400 Bad Request", &not_this_sign_in().0).await
                }
                Some(Answer::Done(answer)) => {
                    self.finish(&mut stream, &answer).await;
                    return answer;
                }
            }
        }
    }
}

impl Callback {
    async fn finish(&self, stream: &mut TcpStream, answer: &LoopbackResult<String>) {
        match (&self.finish, answer) {
            (Finish::Page(service), Ok(_)) => {
                let message = format!(
                    "Signed in to {service}. You can close this tab and go back to Sikemux."
                );
                respond(stream, "200 OK", &message).await;
            }
            (Finish::Page(_), Err(error)) => respond(stream, "400 Bad Request", &error.0).await,
            (Finish::Redirect { done, .. }, Ok(_)) => redirect(stream, done).await,
            (Finish::Redirect { refused, .. }, Err(_)) => redirect(stream, refused).await,
        }
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        waiting().retain(|(path, _)| *path != self.path);
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

#[derive(Debug, PartialEq)]
enum Answer {
    Done(LoopbackResult<String>),
    /// Any page can send the browser here, so a callback without this sign-in's state changes nothing.
    Stray,
}

fn not_this_sign_in() -> LoopbackError {
    LoopbackError("the browser came back from a sign-in this app did not start".into())
}

/// What a request to the port says: `None` when it is not this sign-in's callback at all.
fn answer_of(target: &str, path: &str, state: &str) -> Option<Answer> {
    let url = url::Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != path {
        return None;
    }
    let value = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if value("state").as_deref() != Some(state) {
        return Some(Answer::Stray);
    }
    if let Some(error) = value("error") {
        return Some(Answer::Done(Err(LoopbackError(
            value("error_description").unwrap_or(error),
        ))));
    }
    Some(Answer::Done(value("code").ok_or_else(|| {
        LoopbackError("the browser came back without a code".into())
    })))
}

async fn redirect(stream: &mut TcpStream, location: &str) {
    let reply = format!(
        "HTTP/1.1 303 See Other\r\nlocation: {location}\r\ncontent-length: 0\r\ncache-control: no-store\r\nconnection: close\r\n\r\n"
    );
    stream.write_all(reply.as_bytes()).await.ok();
    stream.shutdown().await.ok();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_callback_carries_the_code_when_the_state_matches() {
        let answer = answer_of("/jira/callback?code=abc&state=s1", "/jira/callback", "s1");
        assert_eq!(answer, Some(Answer::Done(Ok("abc".into()))));
    }

    #[test]
    fn a_callback_for_someone_elses_sign_in_changes_nothing() {
        for target in [
            "/jira/callback?code=abc&state=other",
            "/jira/callback?error=access_denied",
            "/jira/callback?error=access_denied&state=other",
        ] {
            assert_eq!(
                answer_of(target, "/jira/callback", "s1"),
                Some(Answer::Stray)
            );
        }
    }

    #[test]
    fn saying_no_in_the_browser_reads_as_a_refusal() {
        let answer = answer_of(
            "/jira/callback?error=access_denied&error_description=The+user+denied&state=s1",
            "/jira/callback",
            "s1",
        );
        assert_eq!(
            answer,
            Some(Answer::Done(Err(LoopbackError("The user denied".into()))))
        );
    }

    #[test]
    fn another_plugins_callback_and_stray_requests_are_not_this_one() {
        assert!(answer_of(
            "/bitbucket/callback?code=abc&state=s1",
            "/jira/callback",
            "s1"
        )
        .is_none());
        assert!(answer_of("/favicon.ico", "/jira/callback", "s1").is_none());
        assert!(answer_of("", "/jira/callback", "s1").is_none());
    }

    #[test]
    fn the_redirect_names_the_shared_port_and_the_plugins_path() {
        assert_eq!(
            redirect_uri("/jira/callback"),
            "http://localhost:47123/jira/callback"
        );
    }
}
