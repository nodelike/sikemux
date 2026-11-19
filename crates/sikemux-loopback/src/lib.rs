// Signing in through the browser. A service sends the browser back to a fixed
// address on this machine with a one-time code. Every plugin listens on the same
// port, each at a path of its own, so only one port has to stay free. A browser
// on another machine comes back to its own localhost instead, and the address it
// landed on is handed in with `relay`.

use std::fmt;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;

/// Registered with each service as part of its callback, so it cannot move without changing them too.
pub const PORT: u16 = 47123;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a new sign-in waits for an earlier one it replaced to let go of the port.
const TAKEOVER_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq, Eq)]
pub struct LoopbackError(pub String);

impl fmt::Display for LoopbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LoopbackError {}

pub type LoopbackResult<T> = Result<T, LoopbackError>;

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

pub struct Callback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    redirect_uri: String,
    path: &'static str,
    finish: Finish,
    /// A page this machine serves the browser first, at its own path, such as one that posts a
    /// form to the service.
    start: Option<(&'static str, String)>,
    relayed: Mutex<Option<UnboundedReceiver<String>>>,
    id: u64,
    replaced: watch::Receiver<bool>,
}

struct Waiting {
    id: u64,
    path: &'static str,
    on_shared_port: bool,
    relay: UnboundedSender<String>,
    replace: watch::Sender<bool>,
}

/// The sign-ins waiting for a browser.
static WAITING: Mutex<Vec<Waiting>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn waiting() -> std::sync::MutexGuard<'static, Vec<Waiting>> {
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
        .rev()
        .find(|entry| entry.path == path)
        .map(|entry| &entry.relay)
        .ok_or_else(|| LoopbackError("no sign-in is waiting for that address".into()))?;
    sender
        .send(target)
        .map_err(|_| LoopbackError("that sign-in stopped waiting".into()))
}

impl Callback {
    /// Listens on the shared port at both loopback addresses, since a browser
    /// may try `localhost` as either. Both are held before the browser is
    /// opened, so no other program can be listening on one of them for the code.
    /// A sign-in still waiting on the port, such as one the person gave up on in
    /// the browser, is stopped so this one can start.
    pub async fn bind(path: &'static str, service: &'static str) -> LoopbackResult<Self> {
        Self::bind_at(PORT, path, service).await
    }

    async fn bind_at(port: u16, path: &'static str, service: &'static str) -> LoopbackResult<Self> {
        let replacing = replace_shared_port_sign_ins();
        let deadline = Instant::now() + TAKEOVER_WAIT;
        let (v4, v6) = loop {
            match bind_shared_port(port).await {
                Ok(listeners) => break listeners,
                Err(_) if replacing && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(25)).await
                }
                Err(error) => return Err(error),
            }
        };
        Ok(Self::waiting(
            v4,
            v6,
            redirect_uri(path),
            path,
            Finish::Page(service),
            true,
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
        Ok(Self::waiting(v4, None, redirect, path, finish, false))
    }

    fn waiting(
        v4: TcpListener,
        v6: Option<TcpListener>,
        redirect_uri: String,
        path: &'static str,
        finish: Finish,
        on_shared_port: bool,
    ) -> Self {
        let (relay, relayed) = unbounded_channel();
        let (replace, replaced) = watch::channel(false);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        waiting().push(Waiting {
            id,
            path,
            on_shared_port,
            relay,
            replace,
        });
        Self {
            v4,
            v6,
            redirect_uri,
            path,
            finish,
            start: None,
            relayed: Mutex::new(Some(relayed)),
            id,
            replaced,
        }
    }

    /// Serves `page` as it is at `path`, for the browser to open before it goes to the service.
    pub fn serving(mut self, path: &'static str, page: String) -> Self {
        self.start = Some((path, page));
        self
    }

    pub fn address_of(&self, path: &str) -> String {
        let base = self
            .redirect_uri
            .strip_suffix(self.path)
            .unwrap_or_default();
        format!("{base}{path}")
    }

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
        let mut replaced = self.replaced.clone();
        loop {
            if *replaced.borrow_and_update() {
                return Err(LoopbackError("a newer sign-in took over".into()));
            }
            let accepted = tokio::select! {
                _ = replaced.changed() => continue,
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
            if let Some((_, page)) = self.start.as_ref().filter(|(at, _)| is_path(&target, at)) {
                respond_page(&mut stream, page).await;
                continue;
            }
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
        waiting().retain(|entry| entry.id != self.id);
    }
}

fn port_taken(port: u16) -> LoopbackError {
    LoopbackError(format!(
        "port {port} is taken, by another sign-in that is still waiting or by another app"
    ))
}

async fn bind_shared_port(port: u16) -> LoopbackResult<(TcpListener, Option<TcpListener>)> {
    let v4 = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|_| port_taken(port))?;
    let v6 = match TcpListener::bind(("::1", port)).await {
        Ok(v6) => Some(v6),
        Err(error) if error.kind() == ErrorKind::AddrNotAvailable => None,
        Err(_) => return Err(port_taken(port)),
    };
    Ok((v4, v6))
}

/// Tells every sign-in on the shared port to stop, and says whether there was one.
fn replace_shared_port_sign_ins() -> bool {
    let waiting = waiting();
    let mut replaced = false;
    for entry in waiting.iter().filter(|entry| entry.on_shared_port) {
        entry.replace.send_replace(true);
        replaced = true;
    }
    replaced
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

fn is_path(target: &str, path: &str) -> bool {
    target.split('?').next() == Some(path)
}

async fn respond_page(stream: &mut TcpStream, page: &str) {
    let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n{page}",
        page.len()
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

    #[tokio::test]
    async fn a_new_sign_in_takes_over_from_one_left_waiting() {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let first = Callback::bind_at(port, "/github/app/callback", "GitHub")
            .await
            .unwrap();
        let abandoned = tokio::spawn(async move {
            let answer = first.code("first").await;
            drop(first);
            answer
        });
        tokio::task::yield_now().await;
        let second = Callback::bind_at(port, "/jira/callback", "Jira")
            .await
            .expect("the second sign-in gets the port");
        assert_eq!(
            abandoned.await.unwrap(),
            Err(LoopbackError("a newer sign-in took over".into()))
        );
        let waiting = tokio::spawn(async move { second.code("second").await });
        tokio::task::yield_now().await;
        relay("/jira/callback?code=abc&state=second").unwrap();
        assert_eq!(waiting.await.unwrap(), Ok("abc".into()));
    }

    #[test]
    fn the_redirect_names_the_shared_port_and_the_plugins_path() {
        assert_eq!(
            redirect_uri("/jira/callback"),
            "http://localhost:47123/jira/callback"
        );
    }
}
