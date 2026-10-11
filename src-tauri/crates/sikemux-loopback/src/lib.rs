// Signing in through the browser. A service sends the browser back to a fixed
// address on this machine with a one-time code. Every plugin listens on the same
// port, each at a path of its own, so only one port has to stay free.

use std::fmt;
use std::io::Read;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| LoopbackError(format!("no randomness: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Listens on both loopback addresses, since a browser may try `localhost`
/// as either. Bound before the browser is opened, so the answer cannot arrive first.
pub struct Callback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    path: &'static str,
    service: &'static str,
}

impl Callback {
    pub async fn bind(path: &'static str, service: &'static str) -> LoopbackResult<Self> {
        let v4 = TcpListener::bind(("127.0.0.1", PORT)).await.map_err(|_| {
            LoopbackError(format!(
                "port {PORT} is taken, by another sign-in that is still waiting or by another app"
            ))
        })?;
        let v6 = TcpListener::bind(("::1", PORT)).await.ok();
        Ok(Self {
            v4,
            v6,
            path,
            service,
        })
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
        loop {
            let Ok(mut stream) = self.accept().await else {
                continue;
            };
            let Ok(Ok(target)) =
                tokio::time::timeout(REQUEST_TIMEOUT, request_target(&mut stream)).await
            else {
                continue;
            };
            match answer_of(&target, self.path, state) {
                None => respond(&mut stream, "404 Not Found", "Nothing here.").await,
                Some(Ok(code)) => {
                    let message = format!(
                        "Signed in to {}. You can close this tab and go back to Sikemux.",
                        self.service
                    );
                    respond(&mut stream, "200 OK", &message).await;
                    return Ok(code);
                }
                Some(Err(error)) => {
                    respond(&mut stream, "400 Bad Request", &error.0).await;
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

/// What a request to the port says: `None` when it is not this sign-in's callback at all.
fn answer_of(target: &str, path: &str, state: &str) -> Option<LoopbackResult<String>> {
    let url = url::Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != path {
        return None;
    }
    let value = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if let Some(error) = value("error") {
        return Some(Err(LoopbackError(
            value("error_description").unwrap_or(error),
        )));
    }
    if value("state").as_deref() != Some(state) {
        return Some(Err(LoopbackError(
            "the browser came back from a sign-in this app did not start".into(),
        )));
    }
    Some(value("code").ok_or_else(|| LoopbackError("the browser came back without a code".into())))
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
        assert_eq!(answer, Some(Ok("abc".into())));
    }

    #[test]
    fn a_code_for_someone_elses_sign_in_is_refused() {
        let answer = answer_of(
            "/jira/callback?code=abc&state=other",
            "/jira/callback",
            "s1",
        );
        assert!(matches!(answer, Some(Err(_))));
    }

    #[test]
    fn saying_no_in_the_browser_reads_as_a_refusal() {
        let answer = answer_of(
            "/jira/callback?error=access_denied&error_description=The+user+denied",
            "/jira/callback",
            "s1",
        );
        assert_eq!(answer, Some(Err(LoopbackError("The user denied".into()))));
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
