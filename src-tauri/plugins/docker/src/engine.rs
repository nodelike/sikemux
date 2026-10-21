//! Finding the Docker Engine on this Mac and talking HTTP to it over its socket.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use tokio::net::UnixStream;

use crate::error::{DockerError, DockerResult};

/// Where Docker Desktop, Colima, OrbStack and Rancher Desktop put their sockets,
/// tried in this order when neither `DOCKER_HOST` nor a context names one.
const KNOWN_SOCKETS: &[&str] = &[
    "~/.docker/run/docker.sock",
    "/var/run/docker.sock",
    "~/.colima/default/docker.sock",
    "~/.orbstack/run/docker.sock",
    "~/.rd/docker.sock",
];

/// Where the engine's socket is, and what pointed there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub socket: PathBuf,
    /// `DOCKER_HOST`, a context's name, or the folder it was found in.
    pub source: String,
}

/// What `DOCKER_HOST`, `DOCKER_CONTEXT` and the home folder say, read once so
/// finding the socket can be tried without touching the real environment.
pub struct Surroundings {
    pub home: PathBuf,
    pub docker_host: Option<String>,
    pub docker_context: Option<String>,
}

impl Surroundings {
    pub fn read() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let set = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        Self {
            home,
            docker_host: set("DOCKER_HOST"),
            docker_context: set("DOCKER_CONTEXT"),
        }
    }

    fn expand(&self, path: &str) -> PathBuf {
        match path.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None => PathBuf::from(path),
        }
    }
}

/// The socket `DOCKER_HOST`, then the current context, then the usual places name.
pub fn find(around: &Surroundings) -> DockerResult<Endpoint> {
    if let Some(host) = &around.docker_host {
        return Ok(Endpoint {
            socket: socket_of(host)?,
            source: "DOCKER_HOST".to_owned(),
        });
    }
    if let Some(name) = current_context(around) {
        if name != "default" {
            let host = context_host(around, &name).ok_or_else(|| {
                DockerError::NotRunning(format!("the Docker context {name} has no endpoint"))
            })?;
            return Ok(Endpoint {
                socket: socket_of(&host)?,
                source: name,
            });
        }
    }
    first_existing(around, KNOWN_SOCKETS)
        .map(|socket| Endpoint {
            source: socket
                .parent()
                .map(|dir| dir.display().to_string())
                .unwrap_or_default(),
            socket,
        })
        .ok_or_else(|| {
            DockerError::NotRunning(
                "Docker isn't running. Start Docker Desktop, Colima or OrbStack, then try again."
                    .to_owned(),
            )
        })
}

fn first_existing(around: &Surroundings, candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|path| around.expand(path))
        .find(|path| path.exists())
}

fn socket_of(host: &str) -> DockerResult<PathBuf> {
    match host.strip_prefix("unix://") {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => Err(DockerError::Unsupported(format!(
            "Sikemux reaches Docker only through a socket on this Mac, not {host}"
        ))),
    }
}

fn current_context(around: &Surroundings) -> Option<String> {
    if let Some(name) = &around.docker_context {
        return Some(name.clone());
    }
    #[derive(Deserialize)]
    struct Config {
        #[serde(rename = "currentContext")]
        current_context: Option<String>,
    }
    let text = std::fs::read_to_string(around.home.join(".docker/config.json")).ok()?;
    serde_json::from_str::<Config>(&text)
        .ok()?
        .current_context
        .filter(|name| !name.is_empty())
}

/// Docker keeps each context in a folder named by a hash of its name, so the
/// folders are read until one carries the name.
fn context_host(around: &Surroundings, name: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Meta {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Endpoints")]
        endpoints: Endpoints,
    }
    #[derive(Deserialize)]
    struct Endpoints {
        docker: Option<HostOf>,
    }
    #[derive(Deserialize)]
    struct HostOf {
        #[serde(rename = "Host")]
        host: String,
    }
    let folders = std::fs::read_dir(around.home.join(".docker/contexts/meta")).ok()?;
    folders.flatten().find_map(|folder| {
        let text = std::fs::read_to_string(folder.path().join("meta.json")).ok()?;
        let meta: Meta = serde_json::from_str(&text).ok()?;
        (meta.name == name)
            .then_some(meta.endpoints.docker)
            .flatten()
            .map(|docker| docker.host)
    })
}

pub struct Opened {
    pub content_type: Option<String>,
    pub body: Incoming,
}

/// One engine, reached over its socket. Each request opens its own connection,
/// so a log being followed never holds up the rest of the pane.
#[derive(Debug, Clone)]
pub struct Engine {
    endpoint: Endpoint,
}

impl Engine {
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub fn find() -> DockerResult<Self> {
        find(&Surroundings::read()).map(Self::new)
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> DockerResult<T> {
        let body = self.send(Method::GET, path, None).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    /// A POST whose answer, if any, is not needed.
    pub async fn post(&self, path: &str, body: Option<Value>) -> DockerResult<()> {
        self.send(Method::POST, path, body).await.map(drop)
    }

    pub async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<Value>,
    ) -> DockerResult<T> {
        let answer = self.send(Method::POST, path, body).await?;
        Ok(serde_json::from_slice(&answer)?)
    }

    pub async fn delete(&self, path: &str) -> DockerResult<()> {
        self.send(Method::DELETE, path, None).await.map(drop)
    }

    pub async fn delete_json<T: DeserializeOwned>(&self, path: &str) -> DockerResult<T> {
        let answer = self.send(Method::DELETE, path, None).await?;
        Ok(serde_json::from_slice(&answer)?)
    }

    /// The answer's body as it arrives, and its content type, for logs.
    pub async fn open(&self, path: &str) -> DockerResult<Opened> {
        let response = self.request(Method::GET, path, None).await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.into_body().collect().await?.to_bytes();
            return Err(refusal(status, &body));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(Opened {
            content_type,
            body: response.into_body(),
        })
    }

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> DockerResult<Bytes> {
        let response = self.request(method, path, body).await?;
        let status = response.status();
        let body = response.into_body().collect().await?.to_bytes();
        if status.is_success() || status == StatusCode::NOT_MODIFIED {
            Ok(body)
        } else {
            Err(refusal(status, &body))
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> DockerResult<Response<Incoming>> {
        let stream = UnixStream::connect(&self.endpoint.socket)
            .await
            .map_err(|error| unreachable(&self.endpoint.socket, &error))?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let payload = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value)?),
            None => Bytes::new(),
        };
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "docker")
            .header("user-agent", "Sikemux");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request
            .body(Full::new(payload))
            .map_err(|error| DockerError::BadArg(error.to_string()))?;
        Ok(sender.send_request(request).await?)
    }
}

fn unreachable(socket: &Path, error: &std::io::Error) -> DockerError {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => DockerError::NotRunning(
            "Docker isn't running. Start Docker Desktop, Colima or OrbStack, then try again."
                .to_owned(),
        ),
        ErrorKind::PermissionDenied => DockerError::NotRunning(format!(
            "Sikemux may not open {}; check that it belongs to you",
            socket.display()
        )),
        _ => DockerError::Transport(error.to_string()),
    }
}

/// The engine's own words for why it refused, which it sends as `{"message": …}`.
fn refusal(status: StatusCode, body: &[u8]) -> DockerError {
    #[derive(Deserialize)]
    struct Message {
        message: String,
    }
    let message = serde_json::from_slice::<Message>(body)
        .map(|answer| answer.message)
        .unwrap_or_else(|_| String::from_utf8_lossy(body).trim().to_owned());
    match status {
        StatusCode::NOT_FOUND => DockerError::NotFound(message),
        StatusCode::CONFLICT => DockerError::Conflict(message),
        StatusCode::BAD_REQUEST => DockerError::BadArg(message),
        _ => DockerError::Http {
            status: status.as_u16(),
            message,
        },
    }
}

/// Text put into a request path, such as a container's name, kept to one segment.
pub fn segment(value: &str) -> DockerResult<&str> {
    let value = value.trim();
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':' | '@');
    if value.is_empty() || !value.chars().all(allowed) {
        return Err(DockerError::BadArg(format!(
            "{value:?} is not a Docker name or id"
        )));
    }
    Ok(value)
}

/// A query string value, with the few characters Docker's filters use escaped.
pub fn query_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                escaped.push(char::from(byte));
            }
            _ => escaped.push_str(&format!("%{byte:02X}")),
        }
    }
    escaped
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    pub fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sikemux-docker-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn around(home: &Path) -> Surroundings {
        Surroundings {
            home: home.to_owned(),
            docker_host: None,
            docker_context: None,
        }
    }

    /// An engine stand-in that answers each request with the next canned
    /// response, and hands back the requests it saw.
    pub struct FakeEngine {
        pub engine: Engine,
        pub seen: tokio::sync::mpsc::UnboundedReceiver<String>,
    }

    pub fn fake_engine(answers: Vec<(u16, &'static str, String)>) -> FakeEngine {
        let dir = scratch("engine");
        let socket = dir.join("docker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (tell, seen) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for (status, content_type, body) in answers {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    request.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&request);
                    if read == 0 || head_and_body_read(&text) {
                        break;
                    }
                }
                let _ = tell.send(String::from_utf8_lossy(&request).into_owned());
                let reply = format!(
                    "HTTP/1.1 {status} OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        FakeEngine {
            engine: Engine::new(Endpoint {
                socket,
                source: "test".to_owned(),
            }),
            seen,
        }
    }

    fn head_and_body_read(text: &str) -> bool {
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            return false;
        };
        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        body.len() >= length
    }

    #[test]
    fn docker_host_wins_and_must_be_a_socket() {
        let home = scratch("host");
        let mut around = around(&home);
        around.docker_host = Some("unix:///tmp/elsewhere.sock".to_owned());
        assert_eq!(
            find(&around).unwrap(),
            Endpoint {
                socket: PathBuf::from("/tmp/elsewhere.sock"),
                source: "DOCKER_HOST".to_owned()
            }
        );
        around.docker_host = Some("tcp://10.0.0.2:2376".to_owned());
        assert!(matches!(find(&around), Err(DockerError::Unsupported(_))));
    }

    #[test]
    fn the_current_context_names_the_socket() {
        let home = scratch("context");
        std::fs::create_dir_all(home.join(".docker/contexts/meta/abc123")).unwrap();
        std::fs::write(
            home.join(".docker/config.json"),
            r#"{"auths":{},"currentContext":"colima"}"#,
        )
        .unwrap();
        std::fs::write(
            home.join(".docker/contexts/meta/abc123/meta.json"),
            r#"{"Name":"colima","Metadata":{},"Endpoints":{"docker":{"Host":"unix:///Users/me/.colima/default/docker.sock","SkipTLSVerify":false}}}"#,
        )
        .unwrap();
        let endpoint = find(&around(&home)).unwrap();
        assert_eq!(
            endpoint.socket,
            PathBuf::from("/Users/me/.colima/default/docker.sock")
        );
        assert_eq!(endpoint.source, "colima");
    }

    #[test]
    fn without_a_context_the_first_known_socket_that_exists_is_used() {
        let home = scratch("known");
        std::fs::create_dir_all(home.join(".orbstack/run")).unwrap();
        std::fs::write(home.join(".orbstack/run/docker.sock"), "").unwrap();
        let candidates = ["~/.docker/run/docker.sock", "~/.orbstack/run/docker.sock"];
        assert_eq!(
            first_existing(&around(&home), &candidates),
            Some(home.join(".orbstack/run/docker.sock"))
        );
        assert_eq!(first_existing(&around(&home), &candidates[..1]), None);
    }

    #[test]
    fn names_put_into_a_path_stay_one_segment() {
        assert_eq!(segment(" web-1 ").unwrap(), "web-1");
        assert_eq!(segment("sha256:abc").unwrap(), "sha256:abc");
        assert!(segment("../../info").is_err());
        assert!(segment("a b").is_err());
        assert!(segment("").is_err());
        assert_eq!(
            query_value(r#"{"label":["a=b"]}"#),
            "%7B%22label%22%3A%5B%22a%3Db%22%5D%7D"
        );
    }

    #[tokio::test]
    async fn a_request_goes_over_the_socket_and_its_refusal_reads_in_the_engines_words() {
        let mut fake = fake_engine(vec![
            (
                200,
                "application/json",
                r#"{"Version":"29.2.1"}"#.to_owned(),
            ),
            (
                409,
                "application/json",
                r#"{"message":"container is running"}"#.to_owned(),
            ),
        ]);
        let version: Value = fake.engine.get("/version").await.unwrap();
        assert_eq!(version["Version"], "29.2.1");
        assert!(fake
            .seen
            .recv()
            .await
            .unwrap()
            .starts_with("GET /version HTTP/1.1"));
        let refused = fake.engine.delete("/containers/web").await.unwrap_err();
        assert!(
            matches!(refused, DockerError::Conflict(message) if message == "container is running")
        );
    }

    #[tokio::test]
    async fn a_missing_socket_reads_as_docker_not_running() {
        let engine = Engine::new(Endpoint {
            socket: scratch("missing").join("docker.sock"),
            source: "test".to_owned(),
        });
        let error = engine.get::<Value>("/version").await.unwrap_err();
        assert!(matches!(error, DockerError::NotRunning(_)));
    }
}
