//! A private `opencode serve` for one owner: started with a password of its own, spoken to
//! over HTTP on the port it reports, and gone once its input closes.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

const READY_WAIT: Duration = Duration::from_secs(20);
const ANSWER_WAIT: Duration = Duration::from_secs(30);

pub struct Server {
    url: String,
    password: String,
    directory: PathBuf,
    http: &'static reqwest::Client,
    /// OpenCode serves for as long as this stays open.
    lease: Option<ChildStdin>,
    child: Child,
}

impl Server {
    /// Starts `command` as a server for `directory` and waits until it says where it listens.
    pub async fn start(mut command: Command, directory: &Path) -> Result<Self, String> {
        let password = uuid::Uuid::new_v4().simple().to_string();
        command
            .args(["serve", "--stdio", "--port", "0"])
            .current_dir(directory)
            .env("OPENCODE_PASSWORD", &password)
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
        let url = ready["url"]
            .as_str()
            .ok_or("OpenCode's server named no address")?
            .trim_end_matches('/')
            .to_owned();
        static LOOPBACK: OnceLock<reqwest::Client> = OnceLock::new();
        let http = crate::http::shared(&LOOPBACK, || crate::http::builder().no_proxy())
            .map_err(|error| format!("Could not reach OpenCode's server: {error}"))?;
        Ok(Self {
            url,
            password,
            directory: directory.to_owned(),
            http,
            lease,
            child,
        })
    }

    /// Asks for `path` in the server's directory and reads the `data` of its answer.
    pub async fn get(&self, path: &str) -> Result<Value, String> {
        self.send(reqwest::Method::GET, path, None).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value, String> {
        self.send(reqwest::Method::PATCH, path, Some(body)).await
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.url))
            .basic_auth("opencode", Some(&self.password))
            .header(
                "x-opencode-directory",
                self.directory.to_string_lossy().as_ref(),
            )
            .timeout(ANSWER_WAIT);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let answer = request
            .send()
            .await
            .map_err(|error| format!("OpenCode did not answer {path}: {error}"))?;
        let status = answer.status();
        if !status.is_success() {
            return Err(format!("OpenCode answered {path} with {status}"));
        }
        let text = answer
            .text()
            .await
            .map_err(|error| format!("Could not read OpenCode's answer to {path}: {error}"))?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        let mut body: Value = serde_json::from_str(&text)
            .map_err(|error| format!("OpenCode's answer to {path} was not JSON: {error}"))?;
        Ok(body.get_mut("data").map(Value::take).unwrap_or_default())
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
