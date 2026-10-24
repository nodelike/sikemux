// CloudWatch logs: a bounded search for agents, and the live tail the pane
// streams with `aws logs tail <group> --follow`, whose child is killed when
// the host drops the stream.

use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::aws::{LogLevel, LogLine};
use sikemux_plugin_api::{reply, PluginResult, StreamSink};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::common::{aws_bin, aws_command, aws_json, cli_value, Target};
use crate::error::{AwsError, AwsResult};

const DEFAULT_EVENTS: usize = 100;
const MAX_EVENTS: usize = 500;
// filter-log-events returns the oldest matches first, so this many are read
// to keep the newest.
const SCANNED_EVENTS: &str = "2000";
const MAX_MESSAGE_CHARS: usize = 2000;
const RESULT_BUDGET: usize = 24_000;
const SUGGESTED_GROUPS: &str = "30";
const ERROR_WORDS: [&str; 6] = ["ERROR", "ERR", "FATAL", "CRITICAL", "PANIC", "SEVERE"];
const WARN_WORDS: [&str; 2] = ["WARN", "WARNING"];

#[derive(Serialize)]
pub struct LogSearch {
    group: String,
    events: Vec<LogEvent>,
    note: Option<String>,
}

#[derive(Serialize)]
pub struct LogEvent {
    at: String,
    stream: Option<String>,
    message: String,
}

impl LogEvent {
    fn size(&self) -> usize {
        self.at.len() + self.stream.as_ref().map_or(0, String::len) + self.message.len()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    timestamp: Option<i64>,
    log_stream_name: Option<String>,
    #[serde(default)]
    message: String,
}

/// `30s`, `15m`, `2h` or `3d` back from now.
pub(crate) fn since_millis(since: &str, now: SystemTime) -> AwsResult<i64> {
    let bad = || AwsError::BadArg("since is a number and a unit, like 30m, 2h or 1d");
    let unit = since.chars().last().ok_or_else(bad)?;
    let amount: u64 = since
        .strip_suffix(unit)
        .ok_or_else(bad)?
        .parse()
        .map_err(|_| bad())?;
    let seconds = match unit {
        's' => amount,
        'm' => amount * 60,
        'h' => amount * 3600,
        'd' => amount * 86_400,
        _ => return Err(bad()),
    };
    let start = now
        .checked_sub(Duration::from_secs(seconds))
        .and_then(|start| start.duration_since(UNIX_EPOCH).ok())
        .ok_or_else(bad)?;
    Ok(start.as_millis() as i64)
}

fn utc_text(millis: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
        .ok()
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| millis.to_string())
}

/// Drops terminal colour codes, which loggers often write into CloudWatch.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for code in chars.by_ref() {
                    if code.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn trimmed(raw: RawEvent) -> LogEvent {
    let mut message = plain(raw.message.trim_end());
    if let Some((cut, _)) = message.char_indices().nth(MAX_MESSAGE_CHARS) {
        message.truncate(cut);
        message.push('…');
    }
    LogEvent {
        at: raw.timestamp.map(utc_text).unwrap_or_default(),
        stream: raw.log_stream_name,
        message,
    }
}

/// The newest events in a log group, or one stream of it, since a while back,
/// optionally matching a CloudWatch filter pattern.
pub(crate) async fn search(
    target: &Target,
    group: &str,
    stream: Option<&str>,
    pattern: Option<&str>,
    since: Option<&str>,
    limit: Option<usize>,
) -> AwsResult<LogSearch> {
    let limit = limit.unwrap_or(DEFAULT_EVENTS).clamp(1, MAX_EVENTS);
    let start = since_millis(since.unwrap_or("1h"), SystemTime::now())?.to_string();
    let filter = pattern.map(|pattern| format!("--filter-pattern={pattern}"));
    let mut args = vec![
        "logs",
        "filter-log-events",
        "--log-group-name",
        cli_value(group)?,
        "--start-time",
        &start,
        "--max-items",
        SCANNED_EVENTS,
    ];
    if let Some(stream) = stream {
        args.extend(["--log-stream-names", cli_value(stream)?]);
    }
    if let Some(filter) = &filter {
        args.push(filter);
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Resp {
        #[serde(default)]
        events: Vec<RawEvent>,
        next_token: Option<String>,
    }
    let resp: Resp = match aws_json(target, &args).await {
        Err(AwsError::Aws(message)) if message.contains("ResourceNotFoundException") => {
            return Err(unknown_group(target, group).await)
        }
        other => other?,
    };
    let matched = resp.events.len();
    let mut events: Vec<LogEvent> = resp
        .events
        .into_iter()
        .skip(matched.saturating_sub(limit))
        .map(trimmed)
        .collect();
    let mut size: usize = events.iter().map(LogEvent::size).sum();
    let mut over_budget = 0;
    while events.len() > 1 && size > RESULT_BUDGET {
        size -= events.remove(0).size();
        over_budget += 1;
    }
    let shown = events.len();
    let note = match (matched - shown, resp.next_token.is_some()) {
        (0, false) => None,
        (0, true) => Some("More events matched after these; narrow since or the pattern.".to_string()),
        (_, more) => Some(format!(
            "These are the newest {shown} of {}{matched} matches{}; narrow since or the pattern for the rest.",
            if more { "over " } else { "" },
            if over_budget > 0 { ", cut to keep this short" } else { "" }
        )),
    };
    Ok(LogSearch {
        group: group.to_string(),
        events,
        note,
    })
}

async fn unknown_group(target: &Target, group: &str) -> AwsError {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Resp {
        #[serde(default)]
        log_groups: Vec<Group>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Group {
        log_group_name: String,
    }
    let pattern = group.trim_matches('/').rsplit('/').next().unwrap_or(group);
    let pattern_arg = format!("--log-group-name-pattern={pattern}");
    let similar: Vec<String> = aws_json::<Resp>(
        target,
        &[
            "logs",
            "describe-log-groups",
            &pattern_arg,
            "--max-items",
            SUGGESTED_GROUPS,
        ],
    )
    .await
    .map(|resp| {
        resp.log_groups
            .into_iter()
            .map(|g| g.log_group_name)
            .collect()
    })
    .unwrap_or_default();
    AwsError::NotFound(if similar.is_empty() {
        format!("No log group {group}.")
    } else {
        format!(
            "No log group {group}. Groups with {pattern} in their name: {}.",
            similar.join(", ")
        )
    })
}

pub(crate) async fn tail(
    target: Target,
    log_group: String,
    log_stream: Option<String>,
    since: Option<String>,
    sink: StreamSink,
) -> PluginResult<()> {
    cli_value(&log_group)?;
    if let Some(stream) = &log_stream {
        cli_value(stream)?;
    }
    if let Some(since) = &since {
        cli_value(since)?;
    }
    let bin = aws_bin();
    let mut cmd = Command::from(aws_command(&target));
    // The AWS CLI's embedded Python block-buffers stdout when it is a
    // pipe, so a quiet log stream would never reach us without this.
    cmd.env("PYTHONUNBUFFERED", "1")
        .arg("logs")
        .arg("tail")
        .arg(&log_group)
        .arg("--follow")
        .arg("--format")
        .arg("short");
    if let Some(stream) = &log_stream {
        cmd.arg("--log-stream-names").arg(stream);
    }
    cmd.arg("--since").arg(since.as_deref().unwrap_or("5m"));
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AwsError::CliMissing(bin.clone())
        } else {
            AwsError::Io(e)
        }
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or(AwsError::Aws("no stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(AwsError::Aws("no stderr".into()))?;

    let mut out_lines = BufReader::new(stdout).lines();
    let mut err_lines = BufReader::new(stderr).lines();
    let mut stderr_open = true;
    loop {
        tokio::select! {
            line = out_lines.next_line() => match line {
                Ok(Some(line)) => send(&sink, tailed(&line))?,
                _ => break,
            },
            line = err_lines.next_line(), if stderr_open => match line {
                Ok(Some(line)) => send(&sink, from_cli(line))?,
                _ => stderr_open = false,
            },
        }
    }
    while stderr_open {
        match err_lines.next_line().await {
            Ok(Some(line)) => send(&sink, from_cli(line))?,
            _ => stderr_open = false,
        }
    }
    let _ = child.wait().await;
    Ok(())
}

fn send(sink: &StreamSink, line: Option<LogLine>) -> PluginResult<()> {
    match line {
        Some(line) => sink.send(reply(line)?),
        None => Ok(()),
    }
}

/// A line `aws logs tail --format short` printed: its time, then the event's text.
fn tailed(line: &str) -> Option<LogLine> {
    if line.trim().is_empty() {
        return None;
    }
    let dated = line.split_once(' ').and_then(|(stamp, text)| {
        let (day, clock) = stamp.split_once('T')?;
        let clock = clock.get(..8)?;
        let shaped = |text: &str, form: &str| {
            text.len() == form.len()
                && text.chars().zip(form.chars()).all(|(c, f)| {
                    if f == '9' {
                        c.is_ascii_digit()
                    } else {
                        c == f
                    }
                })
        };
        (shaped(day, "9999-99-99") && shaped(clock, "99:99:99")).then_some((day, clock, text))
    });
    let (day, clock, text) = match dated {
        Some((day, clock, text)) => (Some(day.to_owned()), Some(clock.to_owned()), text),
        None => (None, None, line),
    };
    let text = plain(text);
    Some(LogLine {
        level: level_of(&text),
        text,
        day,
        clock,
    })
}

/// Something the CLI itself said, such as its sign-in having run out.
fn from_cli(line: String) -> Option<LogLine> {
    (!line.trim().is_empty()).then(|| LogLine {
        day: None,
        clock: None,
        text: plain(&line),
        level: Some(LogLevel::Error),
    })
}

fn level_of(text: &str) -> Option<LogLevel> {
    let words = || {
        text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map(str::to_ascii_uppercase)
    };
    if words().any(|word| ERROR_WORDS.contains(&word.as_str())) {
        Some(LogLevel::Error)
    } else if words().any(|word| WARN_WORDS.contains(&word.as_str())) {
        Some(LogLevel::Warn)
    } else {
        None
    }
}
