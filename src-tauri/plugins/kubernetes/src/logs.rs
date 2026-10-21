//! A container's logs: read once, from the container before the last restart when
//! it crashed, or followed as kubectl writes them.

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::{reply, PluginResult, StreamSink};
use tokio::sync::mpsc;

use crate::error::{KubeError, KubeResult};
use crate::kubectl::{self, Scope};

const DEFAULT_TAIL: u32 = 200;
const MAX_TAIL: u32 = 5000;
/// A line longer than this is cut, so one runaway line can't fill the pane.
const MAX_LINE_CHARS: usize = 2000;
/// Lines sent to the pane together while following, so a busy pod doesn't flood it.
const BATCH: usize = 200;

#[derive(Debug, Serialize, PartialEq, Clone)]
pub struct Line {
    /// When the container wrote it, RFC 3339.
    pub at: Option<String>,
    pub text: String,
}

/// Colours and cursor moves written for a terminal, which mean nothing here.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    out
}

/// One line as `kubectl logs --timestamps` prints it: its time, a space, then the text.
pub fn line_of(raw: &str) -> Line {
    let raw = raw.strip_suffix('\r').unwrap_or(raw);
    let (at, text) = match raw.split_once(' ') {
        Some((stamp, rest)) if stamp.len() >= 20 && stamp.as_bytes().get(10) == Some(&b'T') => {
            (Some(stamp.to_string()), rest)
        }
        _ => (None, raw),
    };
    let mut text = strip_ansi(text);
    if text.chars().count() > MAX_LINE_CHARS {
        text = text.chars().take(MAX_LINE_CHARS).collect::<String>() + " …";
    }
    Line { at, text }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogParams {
    #[serde(flatten)]
    pub scope: Scope,
    pub pod: String,
    /// The container; a pod with one needs none.
    #[serde(default)]
    pub container: Option<String>,
    /// The last this many lines. Default 200, at most 5000.
    #[serde(default)]
    pub tail: Option<u32>,
    /// The container that ran before the last restart, which says why a crash loop crashes.
    #[serde(default)]
    pub previous: bool,
    /// Only lines from the last this many seconds.
    #[serde(default)]
    pub since_secs: Option<u64>,
    /// Only lines containing these words, ignoring case.
    #[serde(default)]
    pub grep: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Logs {
    pub lines: Vec<Line>,
    /// Lines left out for not containing the words asked for.
    pub filtered: usize,
}

fn log_args(params: &LogParams, follow: bool) -> KubeResult<Vec<String>> {
    let mut args = vec![
        "logs".to_string(),
        kubectl::object_name("pod", &params.pod)?,
        "--timestamps".to_string(),
        format!(
            "--tail={}",
            params.tail.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL)
        ),
    ];
    if let Some(container) = params
        .container
        .as_deref()
        .filter(|container| !container.is_empty())
    {
        args.push(format!(
            "--container={}",
            kubectl::object_name("container", container)?
        ));
    }
    if params.previous {
        args.push("--previous".into());
    }
    if let Some(secs) = params.since_secs.filter(|secs| *secs > 0) {
        args.push(format!("--since={secs}s"));
    }
    if follow {
        args.push("--follow".into());
    }
    Ok(args)
}

pub async fn read(params: LogParams) -> KubeResult<Logs> {
    let args = log_args(&params, false)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let text = kubectl::run(&params.scope, &refs).await?;
    let mut lines: Vec<Line> = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(line_of)
        .collect();
    let before = lines.len();
    if let Some(words) = params
        .grep
        .as_deref()
        .map(str::to_lowercase)
        .filter(|words| !words.trim().is_empty())
    {
        lines.retain(|line| line.text.to_lowercase().contains(&words));
    }
    Ok(Logs {
        filtered: before - lines.len(),
        lines,
    })
}

/// Stops kubectl however following ends, including the pane closing mid-stream, which drops this.
struct KillOnDrop(Arc<Mutex<std::process::Child>>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Ok(mut child) = self.0.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Batch<'a> {
    lines: &'a [Line],
    /// The log ended, as it does when the container stops.
    ended: bool,
}

/// Sends the last `tail` lines, then each new batch as the container writes it,
/// until the pane stops listening or the container stops. kubectl is stopped either way.
pub async fn follow(params: LogParams, sink: &StreamSink) -> PluginResult<()> {
    let mut args = kubectl::scope_args(&params.scope)?;
    args.extend(log_args(&params, true)?);
    let mut child = sikemux_process::user_environment::command(kubectl::kubectl_bin())
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => KubeError::CliMissing,
            _ => KubeError::Kubectl(error.to_string()),
        })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));
    let guard = KillOnDrop(Arc::clone(&child));
    let (tell, mut heard) = mpsc::channel::<Line>(BATCH * 4);
    let reader = tokio::task::spawn_blocking(move || {
        let Some(stdout) = stdout else { return };
        for raw in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tell.blocking_send(line_of(&raw)).is_err() {
                return;
            }
        }
    });
    let mut listening = true;
    while let Some(first) = heard.recv().await {
        let mut lines = vec![first];
        while lines.len() < BATCH {
            match heard.try_recv() {
                Ok(more) => lines.push(more),
                Err(_) => break,
            }
        }
        if sink
            .send(reply(Batch {
                lines: &lines,
                ended: false,
            })?)
            .is_err()
        {
            listening = false;
            break;
        }
    }
    drop(guard);
    drop(heard);
    let _ = reader.await;
    if !listening {
        return Ok(());
    }
    let complaint = stderr
        .map(|stderr| {
            BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !complaint.trim().is_empty() {
        return Err(kubectl::classify(&complaint).into());
    }
    let _ = sink.send(reply(Batch {
        lines: &[],
        ended: true,
    })?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: serde_json::Value) -> LogParams {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_line_reads_its_time_and_drops_terminal_colours() {
        let line = line_of("2026-10-19T11:44:17.123456789Z \u{1b}[31mFATAL: password authentication failed\u{1b}[0m\r");
        assert_eq!(line.at.as_deref(), Some("2026-10-19T11:44:17.123456789Z"));
        assert_eq!(line.text, "FATAL: password authentication failed");
        assert_eq!(line_of("no stamp here").at, None);
        assert!(line_of(&"x".repeat(MAX_LINE_CHARS + 3))
            .text
            .ends_with(" …"));
    }

    #[test]
    fn the_previous_container_and_a_window_are_asked_of_kubectl() {
        let args = log_args(
            &params(serde_json::json!({ "pod": "worker-1", "container": "worker", "previous": true, "sinceSecs": 600, "tail": 99999 })),
            false,
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "logs",
                "worker-1",
                "--timestamps",
                "--tail=5000",
                "--container=worker",
                "--previous",
                "--since=600s"
            ]
        );
        let followed = log_args(&params(serde_json::json!({ "pod": "web-1" })), true).unwrap();
        assert_eq!(
            followed,
            ["logs", "web-1", "--timestamps", "--tail=200", "--follow"]
        );
        assert!(log_args(
            &params(serde_json::json!({ "pod": "--all-containers" })),
            false
        )
        .is_err());
    }
}
