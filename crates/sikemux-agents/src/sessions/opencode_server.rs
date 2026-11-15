//! `opencode serve`, spoken to over HTTP on the port it reports, with a password of its own.
//! A [`Server`] starts one for a short ask and is gone once its input closes; a [`Client`]
//! speaks to one whoever started it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

pub const READY_WAIT: Duration = Duration::from_secs(20);
const ANSWER_WAIT: Duration = Duration::from_secs(30);

/// The arguments that start a server for one owner, which leaves once its input closes.
pub const SERVE_ARGS: [&str; 4] = ["serve", "--stdio", "--port", "0"];

/// The variable OpenCode's server reads its password from.
pub const PASSWORD_ENV: &str = "OPENCODE_PASSWORD";

pub fn new_password() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Where the server listens, from the one line it prints once it does.
pub fn ready_url(line: &Value) -> Result<String, String> {
    Ok(line["url"]
        .as_str()
        .ok_or("OpenCode's server named no address")?
        .trim_end_matches('/')
        .to_owned())
}

/// Speaks to one server for one directory. Clones share it.
#[derive(Clone)]
pub struct Client(Arc<Reached>);

struct Reached {
    url: String,
    password: String,
    directory: PathBuf,
}

/// What OpenCode answered other than yes.
#[derive(Debug)]
pub struct Refused {
    /// The HTTP status, when the server answered at all.
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<Refused> for String {
    fn from(refused: Refused) -> Self {
        refused.message
    }
}

fn answers() -> Result<&'static reqwest::Client, String> {
    static LOOPBACK: OnceLock<reqwest::Client> = OnceLock::new();
    crate::http::shared(&LOOPBACK, || crate::http::builder().no_proxy())
        .map_err(|error| format!("Could not reach OpenCode's server: {error}"))
}

/// Without a read timeout, since the event stream can be quiet for as long as nothing happens.
fn streams() -> Result<&'static reqwest::Client, String> {
    static STREAMS: OnceLock<reqwest::Client> = OnceLock::new();
    crate::http::shared(&STREAMS, || {
        reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
    })
    .map_err(|error| format!("Could not reach OpenCode's server: {error}"))
}

impl Client {
    pub fn new(url: String, password: String, directory: &Path) -> Self {
        Self(Arc::new(Reached {
            url,
            password,
            directory: directory.to_owned(),
        }))
    }

    pub fn url(&self) -> &str {
        &self.0.url
    }

    pub fn password(&self) -> &str {
        &self.0.password
    }

    /// Asks for `path` in the server's directory and reads the `data` of its answer.
    pub async fn get(&self, path: &str) -> Result<Value, Refused> {
        self.send(reqwest::Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, Refused> {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value, Refused> {
        self.send(reqwest::Method::PATCH, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value, Refused> {
        self.send(reqwest::Method::PUT, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value, Refused> {
        self.send(reqwest::Method::DELETE, path, None).await
    }

    fn request(
        &self,
        http: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        http.request(method, format!("{}{path}", self.0.url))
            .basic_auth("opencode", Some(&self.0.password))
            .header(
                "x-opencode-directory",
                self.0.directory.to_string_lossy().as_ref(),
            )
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Refused> {
        let unreached = |message: String| Refused {
            status: None,
            message,
        };
        let mut request = self
            .request(answers().map_err(unreached)?, method, path)
            .timeout(ANSWER_WAIT);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let answer = request
            .send()
            .await
            .map_err(|error| unreached(format!("OpenCode did not answer {path}: {error}")))?;
        let status = answer.status();
        let text = answer.text().await.map_err(|error| {
            unreached(format!("Could not read OpenCode's answer to {path}: {error}"))
        })?;
        let body: Value = serde_json::from_str(&text).unwrap_or_default();
        if !status.is_success() {
            let said = body["message"].as_str().filter(|said| !said.is_empty());
            return Err(Refused {
                status: Some(status.as_u16()),
                message: said.map_or_else(
                    || format!("OpenCode answered {path} with {status}"),
                    str::to_owned,
                ),
            });
        }
        let mut body = body;
        Ok(body.get_mut("data").map(Value::take).unwrap_or_default())
    }

    /// Follows the server's events from now on, each sent to `events` as it comes, until the
    /// stream ends or `events` is dropped. Answers once the server has said it is connected,
    /// so nothing done after this is missed.
    pub async fn follow(&self, events: mpsc::UnboundedSender<Value>) -> Result<(), String> {
        let mut answer = self
            .request(streams()?, reqwest::Method::GET, "/api/event")
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(|error| format!("OpenCode's events could not be followed: {error}"))?;
        if !answer.status().is_success() {
            return Err(format!("OpenCode answered its events with {}", answer.status()));
        }
        let mut reader = EventReader::default();
        let (connected, is_connected) = tokio::sync::oneshot::channel();
        let mut connected = Some(connected);
        tokio::spawn(async move {
            while let Ok(Some(chunk)) = answer.chunk().await {
                for event in reader.read(&chunk) {
                    if event["type"] == "server.connected" {
                        if let Some(connected) = connected.take() {
                            let _ = connected.send(());
                        }
                        continue;
                    }
                    if events.send(event).is_err() {
                        return;
                    }
                }
            }
        });
        tokio::time::timeout(READY_WAIT, is_connected)
            .await
            .map_err(|_| "OpenCode's events did not start in time".to_owned())?
            .map_err(|_| "OpenCode's events ended before they started".to_owned())
    }
}

/// The events in a server-sent event stream, read as its bytes come.
#[derive(Default)]
struct EventReader {
    pending: Vec<u8>,
    data: String,
}

impl EventReader {
    fn read(&mut self, chunk: &[u8]) -> Vec<Value> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if let Ok(event) = serde_json::from_str(&self.data) {
                    events.push(event);
                }
                self.data.clear();
            } else if let Some(data) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.trim_start());
            }
        }
        events
    }
}

/// A server started for one short ask.
pub struct Server {
    client: Client,
    /// OpenCode serves for as long as this stays open.
    lease: Option<ChildStdin>,
    child: Child,
}

impl std::ops::Deref for Server {
    type Target = Client;

    fn deref(&self) -> &Client {
        &self.client
    }
}

impl Server {
    /// Starts `command` as a server for `directory` and waits until it says where it listens.
    pub async fn start(mut command: Command, directory: &Path) -> Result<Self, String> {
        let password = new_password();
        command
            .args(SERVE_ARGS)
            .current_dir(directory)
            .env(PASSWORD_ENV, &password)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| format!("Could not start OpenCode's server: {error}"))?;
        let lease = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or("OpenCode's server has no output")?;
        let mut line = String::new();
        tokio::time::timeout(READY_WAIT, BufReader::new(stdout).read_line(&mut line))
            .await
            .map_err(|_| "OpenCode's server did not start in time".to_owned())?
            .map_err(|error| format!("Could not read OpenCode's server: {error}"))?;
        let ready: Value = serde_json::from_str(line.trim()).map_err(|_| {
            format!(
                "OpenCode's server said {:?} instead of where it listens",
                line.trim()
            )
        })?;
        Ok(Self {
            client: Client::new(ready_url(&ready)?, password, directory),
            lease,
            child,
        })
    }

    /// Ends the lease and waits for the server to leave.
    pub async fn close(mut self) {
        drop(self.lease.take());
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }
}
