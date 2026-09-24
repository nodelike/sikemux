// Live log tail via /execution/{id}/output. Rundeck returns an offset cursor
// (`offset` + `lastModified`) so each poll returns only the new bytes, with
// per-entry step context (`stepctx`) the UI uses to filter by step.

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};
use sikemux_plugin_api::{reply, PluginResult, StreamSink};
use tokio::time::sleep;

use crate::client::get_json;
use crate::error::RundeckResult;

#[derive(Serialize, Clone, Deserialize)]
pub struct LogEntry {
    pub time: Option<String>,
    pub level: Option<String>,
    pub log: Option<String>,
    pub user: Option<String>,
    #[serde(rename = "stepctx")]
    pub step_ctx: Option<String>,
    pub node: Option<String>,
}

#[derive(Serialize, Clone, Deserialize)]
pub struct LogChunk {
    pub completed: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_cursor")]
    pub offset: Option<String>,
    #[serde(
        rename = "lastModified",
        default,
        deserialize_with = "deserialize_cursor"
    )]
    pub last_modified: Option<String>,
    #[serde(rename = "execCompleted")]
    pub exec_completed: Option<bool>,
    #[serde(rename = "execState")]
    pub exec_state: Option<String>,
    #[serde(rename = "retryBackoff", default)]
    pub retry_backoff: Option<u64>,
    #[serde(default)]
    pub entries: Vec<LogEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CursorValue {
    String(String),
    Signed(i64),
    Unsigned(u64),
}

fn deserialize_cursor<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(
        Option::<CursorValue>::deserialize(deserializer)?.map(|value| match value {
            CursorValue::String(value) => value,
            CursorValue::Signed(value) => value.to_string(),
            CursorValue::Unsigned(value) => value.to_string(),
        }),
    )
}

#[derive(Serialize, Clone)]
pub struct LogTick {
    pub entries: Vec<LogEntry>,
    pub offset: String,
    pub completed: bool,
    pub failed: bool,
    pub error: Option<String>,
}

const POLL_INTERVAL: Duration = Duration::from_millis(1500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const ERROR_GIVEUP: u32 = 8;
const MAX_LINES: u32 = 2000;
const MAX_BACK_TO_BACK_READS: u32 = 10;

fn failure_tick(offset: &str, error: String, consecutive_errors: u32) -> LogTick {
    LogTick {
        entries: Vec::new(),
        offset: offset.to_string(),
        completed: false,
        failed: consecutive_errors >= ERROR_GIVEUP,
        error: Some(error),
    }
}

/// How long to wait before the next read. `None` means read again at once,
/// because the last read was a full page and more output is waiting.
fn next_delay(chunk: &LogChunk, back_to_back_reads: u32) -> Option<Duration> {
    let server_backoff = chunk
        .retry_backoff
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis);
    if server_backoff.is_none()
        && !chunk.entries.is_empty()
        && back_to_back_reads < MAX_BACK_TO_BACK_READS
    {
        return None;
    }
    Some(server_backoff.map_or(POLL_INTERVAL, |backoff| backoff.max(POLL_INTERVAL)))
}

/// Tail log output. Resumes at `offset` when given; otherwise `backlog`
/// replays the last N lines, and 0 or None reads from the beginning.
pub async fn logs(
    execution_id: u64,
    resume_offset: Option<String>,
    backlog: Option<u32>,
    sink: StreamSink,
) -> PluginResult<()> {
    let resuming = resume_offset.is_some();
    let mut offset = resume_offset.unwrap_or_else(|| String::from("0"));
    let mut last_modified: Option<String> = None;
    let mut first_pass = true;
    let mut consecutive_errors: u32 = 0;
    let mut back_to_back_reads: u32 = 0;

    loop {
        let mut query: Vec<(&str, String)> = vec![
            ("format", "json".into()),
            ("maxlines", MAX_LINES.to_string()),
        ];
        match backlog {
            Some(n) if first_pass && !resuming && n > 0 => query.push(("lastlines", n.to_string())),
            _ => {
                query.push(("offset", offset.clone()));
                if let Some(lm) = &last_modified {
                    query.push(("lastmod", lm.clone()));
                }
            }
        }
        first_pass = false;

        let res: RundeckResult<LogChunk> =
            get_json(&format!("/execution/{execution_id}/output"), &query).await;

        let sleep_dur = match res {
            Ok(chunk) => {
                consecutive_errors = 0;
                if let Some(o) = &chunk.offset {
                    offset = o.clone();
                }
                if chunk.last_modified.is_some() {
                    last_modified = chunk.last_modified.clone();
                }
                let completed = log_stream_completed(&chunk);
                let delay = next_delay(&chunk, back_to_back_reads);
                sink.send(reply(LogTick {
                    entries: chunk.entries,
                    offset: offset.clone(),
                    completed,
                    failed: false,
                    error: None,
                })?)?;
                if completed {
                    return Ok(());
                }
                delay
            }
            Err(e) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                let tick = failure_tick(&offset, e.to_string(), consecutive_errors);
                let giving_up = tick.failed;
                sink.send(reply(tick)?)?;
                if giving_up {
                    return Ok(());
                }
                // 3s, 6s, 12s, 24s, then 30s.
                let exp = 1u64 << consecutive_errors.min(6);
                Some(
                    Duration::from_millis((POLL_INTERVAL.as_millis() as u64).saturating_mul(exp))
                        .min(MAX_BACKOFF),
                )
            }
        };
        match sleep_dur {
            Some(delay) => {
                back_to_back_reads = 0;
                sleep(delay).await;
            }
            None => back_to_back_reads += 1,
        }
    }
}

fn log_stream_completed(chunk: &LogChunk) -> bool {
    chunk.completed.unwrap_or(false) && chunk.exec_completed.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_numeric_and_string_output_cursors() {
        let numeric: LogChunk = serde_json::from_str(
            r#"{"completed":false,"offset":12,"lastModified":34,"execCompleted":false,"entries":[]}"#,
        )
        .unwrap();
        let string: LogChunk = serde_json::from_str(
            r#"{"completed":false,"offset":"56","lastModified":"78","execCompleted":false,"entries":[]}"#,
        )
        .unwrap();

        assert_eq!(numeric.offset.as_deref(), Some("12"));
        assert_eq!(numeric.last_modified.as_deref(), Some("34"));
        assert_eq!(string.offset.as_deref(), Some("56"));
        assert_eq!(string.last_modified.as_deref(), Some("78"));
    }

    #[test]
    fn keeps_tailing_until_execution_and_output_are_complete() {
        let mut chunk: LogChunk = serde_json::from_str(
            r#"{"completed":true,"offset":12,"lastModified":34,"execCompleted":false,"entries":[]}"#,
        )
        .unwrap();

        assert!(!log_stream_completed(&chunk));
        chunk.exec_completed = Some(true);
        assert!(log_stream_completed(&chunk));
    }

    fn chunk(entries: usize, retry_backoff: Option<u64>) -> LogChunk {
        let mut chunk: LogChunk = serde_json::from_str(r#"{"entries":[]}"#).unwrap();
        chunk.retry_backoff = retry_backoff;
        chunk.entries = (0..entries)
            .map(|_| LogEntry {
                time: None,
                level: None,
                log: Some("line".into()),
                user: None,
                step_ctx: None,
                node: None,
            })
            .collect();
        chunk
    }

    #[test]
    fn reads_again_at_once_while_output_keeps_coming() {
        assert_eq!(next_delay(&chunk(5, None), 0), None);
        assert_eq!(
            next_delay(&chunk(5, None), MAX_BACK_TO_BACK_READS),
            Some(POLL_INTERVAL)
        );
        assert_eq!(next_delay(&chunk(0, None), 0), Some(POLL_INTERVAL));
    }

    #[test]
    fn honours_the_servers_retry_backoff() {
        assert_eq!(
            next_delay(&chunk(5, Some(5000)), 0),
            Some(Duration::from_millis(5000))
        );
        assert_eq!(next_delay(&chunk(0, Some(10)), 0), Some(POLL_INTERVAL));
    }

    #[test]
    fn only_the_last_error_marks_the_stream_failed_and_never_completed() {
        let early = failure_tick("12", "boom".into(), 1);
        assert!(!early.failed && !early.completed);
        assert_eq!(early.offset, "12");
        let last = failure_tick("12", "boom".into(), ERROR_GIVEUP);
        assert!(last.failed && !last.completed);
    }
}
