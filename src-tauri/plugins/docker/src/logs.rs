//! A container's logs, read once or followed as they are written.
//!
//! A container without a terminal sends its output in frames, each with an
//! 8-byte header saying whether it is stdout or stderr and how long it is. One
//! with a terminal sends plain bytes. Both arrive here as lines.

use std::time::{SystemTime, UNIX_EPOCH};

use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use sikemux_plugin_api::{reply, PluginResult, StreamSink};

use crate::engine::{segment, Engine};
use crate::error::{DockerError, DockerResult};

const DEFAULT_TAIL: u32 = 200;
const MAX_TAIL: u32 = 5000;
/// A line longer than this is cut, so one runaway line can't fill the pane.
const MAX_LINE_CHARS: usize = 2000;
const MULTIPLEXED: &str = "application/vnd.docker.multiplexed-stream";
const RAW: &str = "application/vnd.docker.raw-stream";

#[derive(Debug, Serialize, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    /// When the engine wrote it, as RFC 3339.
    pub at: Option<String>,
    /// stdout or stderr. A container with a terminal writes everything to stdout.
    pub stream: &'static str,
    pub text: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Logs {
    pub lines: Vec<Line>,
    /// Lines dropped because they didn't contain the words asked for.
    pub filtered: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogParams {
    pub id: String,
    /// The last this many lines. Default 500, at most 5000.
    pub tail: Option<u32>,
    /// Only lines written in the last this many seconds.
    pub since_secs: Option<u64>,
    /// Only lines containing these words, ignoring case.
    pub grep: Option<String>,
    /// Only stderr.
    #[serde(default)]
    pub errors_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    Multiplexed,
    Raw,
    /// An engine too old to say; the first bytes decide.
    Unknown,
}

fn framing_of(content_type: Option<&str>) -> Framing {
    match content_type {
        Some(kind) if kind.starts_with(MULTIPLEXED) => Framing::Multiplexed,
        Some(kind) if kind.starts_with(RAW) => Framing::Raw,
        _ => Framing::Unknown,
    }
}

/// Turns the bytes of a log, as they arrive, into whole lines.
pub struct Decoder {
    framing: Framing,
    pending: Vec<u8>,
    /// What each stream has written since its last newline.
    partial: [Vec<u8>; 2],
}

impl Decoder {
    fn new(framing: Framing) -> Self {
        Self {
            framing,
            pending: Vec::new(),
            partial: [Vec::new(), Vec::new()],
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Line> {
        self.pending.extend_from_slice(bytes);
        if self.framing == Framing::Unknown {
            if self.pending.len() < 8 {
                return Vec::new();
            }
            self.framing = if looks_multiplexed(&self.pending) {
                Framing::Multiplexed
            } else {
                Framing::Raw
            };
        }
        let mut lines = Vec::new();
        match self.framing {
            Framing::Raw | Framing::Unknown => {
                let bytes = std::mem::take(&mut self.pending);
                self.split(0, &bytes, &mut lines);
            }
            Framing::Multiplexed => {
                while let Some(header) = self.pending.get(..8) {
                    let stream = usize::from(header.first().copied().unwrap_or(1) == 2);
                    let size = header
                        .get(4..8)
                        .and_then(|size| <[u8; 4]>::try_from(size).ok())
                        .map_or(0, u32::from_be_bytes) as usize;
                    let Some(payload) = self.pending.get(8..8 + size).map(<[u8]>::to_vec) else {
                        break;
                    };
                    self.pending.drain(..8 + size);
                    self.split(stream, &payload, &mut lines);
                }
            }
        }
        lines
    }

    /// What is left once the log has ended, even without a final newline.
    pub fn finish(&mut self) -> Vec<Line> {
        let mut lines = Vec::new();
        let leftover = std::mem::take(&mut self.pending);
        if !leftover.is_empty() {
            self.split(0, &leftover, &mut lines);
        }
        for stream in 0..2 {
            if let Some(partial) = self.partial.get_mut(stream) {
                let rest = std::mem::take(partial);
                if !rest.is_empty() {
                    lines.push(line_of(stream, &rest));
                }
            }
        }
        lines
    }

    fn split(&mut self, stream: usize, bytes: &[u8], lines: &mut Vec<Line>) {
        let Some(partial) = self.partial.get_mut(stream) else {
            return;
        };
        for &byte in bytes {
            if byte == b'\n' {
                let whole = std::mem::take(partial);
                lines.push(line_of(stream, &whole));
            } else {
                partial.push(byte);
            }
        }
    }
}

/// A frame header starts with 0, 1 or 2 for the stream and three zero bytes.
fn looks_multiplexed(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([0..=2, 0, 0, 0]))
}

fn line_of(stream: usize, bytes: &[u8]) -> Line {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_suffix('\r').unwrap_or(&text);
    let (at, text) = match text.split_once(' ') {
        Some((stamp, rest)) if is_timestamp(stamp) => (Some(stamp.to_owned()), rest),
        _ => (None, text),
    };
    let mut text = strip_ansi(text);
    if text.chars().count() > MAX_LINE_CHARS {
        text = text.chars().take(MAX_LINE_CHARS).collect::<String>() + " …";
    }
    Line {
        at,
        stream: if stream == 1 { "stderr" } else { "stdout" },
        text,
    }
}

/// The engine's own timestamps, such as `2026-10-18T10:00:01.123456789Z`.
fn is_timestamp(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() >= 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && (word.ends_with('Z') || word.contains('+'))
}

/// Colours and cursor moves a program wrote for a terminal, which mean nothing here.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn path_of(id: &str, tail: u32, since_secs: Option<u64>, follow: bool) -> String {
    let mut path = format!("/containers/{id}/logs?stdout=1&stderr=1&timestamps=1&tail={tail}");
    if let Some(secs) = since_secs {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        path.push_str(&format!("&since={}", now.saturating_sub(secs)));
    }
    if follow {
        path.push_str("&follow=1");
    }
    path
}

fn tail_of(tail: Option<u32>) -> u32 {
    tail.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL)
}

pub async fn read(engine: &Engine, params: LogParams) -> DockerResult<Logs> {
    let id = segment(&params.id)?;
    let opened = engine
        .open(&path_of(id, tail_of(params.tail), params.since_secs, false))
        .await?;
    let mut decoder = Decoder::new(framing_of(opened.content_type.as_deref()));
    let body = opened.body.collect().await?.to_bytes();
    let mut lines = decoder.feed(&body);
    lines.extend(decoder.finish());
    let before = lines.len();
    let words = params
        .grep
        .as_deref()
        .map(str::to_lowercase)
        .filter(|words| !words.trim().is_empty());
    lines.retain(|line| {
        (!params.errors_only || line.stream == "stderr")
            && words
                .as_deref()
                .is_none_or(|words| line.text.to_lowercase().contains(words))
    });
    Ok(Logs {
        filtered: before - lines.len(),
        lines,
    })
}

#[derive(Deserialize)]
pub struct FollowParams {
    pub id: String,
    pub tail: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Batch<'a> {
    lines: &'a [Line],
    /// The log ended, as it does when the container stops.
    ended: bool,
}

/// Sends the last `tail` lines, then each new batch as the container writes it,
/// until the pane stops listening or the container stops.
pub async fn follow(engine: &Engine, params: FollowParams, sink: &StreamSink) -> PluginResult<()> {
    let id = segment(&params.id)?;
    let opened = engine
        .open(&path_of(id, tail_of(params.tail), None, true))
        .await?;
    let mut decoder = Decoder::new(framing_of(opened.content_type.as_deref()));
    let mut body = opened.body;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(DockerError::from)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        let lines = decoder.feed(&data);
        if !lines.is_empty()
            && sink
                .send(reply(Batch {
                    lines: &lines,
                    ended: false,
                })?)
                .is_err()
        {
            return Ok(());
        }
    }
    let rest = decoder.finish();
    let _ = sink.send(reply(Batch {
        lines: &rest,
        ended: true,
    })?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::fake_engine;

    fn frame(stream: u8, text: &str) -> Vec<u8> {
        let mut bytes = vec![stream, 0, 0, 0];
        bytes.extend_from_slice(&(text.len() as u32).to_be_bytes());
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }

    #[test]
    fn frames_split_anywhere_still_come_out_as_whole_lines_on_their_stream() {
        let mut bytes = frame(
            1,
            "2026-10-18T10:00:01.000000001Z listening on :3000\n2026-10-18T10:00:02.5Z GET /",
        );
        bytes.extend(frame(
            2,
            "2026-10-18T10:00:03Z \u{1b}[31mError: boom\u{1b}[0m\r\n",
        ));
        bytes.extend(frame(1, " 200\n"));
        let mut decoder = Decoder::new(Framing::Multiplexed);
        let mut lines = Vec::new();
        for chunk in bytes.chunks(5) {
            lines.extend(decoder.feed(chunk));
        }
        lines.extend(decoder.finish());
        assert_eq!(
            lines,
            [
                Line {
                    at: Some("2026-10-18T10:00:01.000000001Z".to_owned()),
                    stream: "stdout",
                    text: "listening on :3000".to_owned()
                },
                Line {
                    at: Some("2026-10-18T10:00:03Z".to_owned()),
                    stream: "stderr",
                    text: "Error: boom".to_owned()
                },
                Line {
                    at: Some("2026-10-18T10:00:02.5Z".to_owned()),
                    stream: "stdout",
                    text: "GET / 200".to_owned()
                },
            ]
        );
    }

    #[test]
    fn a_terminal_containers_plain_bytes_are_told_apart_from_frames() {
        let mut decoder = Decoder::new(Framing::Unknown);
        let mut lines = decoder.feed(b"hello from a tty\nno newline");
        lines.extend(decoder.finish());
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["hello from a tty", "no newline"]);
        assert!(looks_multiplexed(&frame(2, "x")));
        assert!(!looks_multiplexed(b"2026-10-18"));
    }

    #[test]
    fn colours_and_window_titles_are_dropped_and_long_lines_cut() {
        assert_eq!(
            strip_ansi("\u{1b}[1;32mok\u{1b}[0m \u{1b}]0;title\u{7}done"),
            "ok done"
        );
        let long = line_of(0, "x".repeat(MAX_LINE_CHARS + 10).as_bytes());
        assert!(long.text.ends_with(" …"));
        assert_eq!(long.text.chars().count(), MAX_LINE_CHARS + 2);
    }

    #[tokio::test]
    async fn reading_asks_for_the_tail_and_keeps_only_the_lines_asked_for() {
        let mut body = frame(1, "2026-10-18T10:00:01Z started\n");
        body.extend(frame(2, "2026-10-18T10:00:02Z ERROR db refused\n"));
        body.extend(frame(2, "2026-10-18T10:00:03Z warning: slow\n"));
        let mut fake = fake_engine(vec![(200, MULTIPLEXED, String::from_utf8(body).unwrap())]);
        let logs = read(
            &fake.engine,
            LogParams {
                id: "web".to_owned(),
                tail: Some(9000),
                since_secs: None,
                grep: Some("error".to_owned()),
                errors_only: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(logs.lines.len(), 1);
        assert_eq!(logs.lines[0].text, "ERROR db refused");
        assert_eq!(logs.filtered, 2);
        let request = fake.seen.recv().await.unwrap();
        assert!(
            request.starts_with(
                "GET /containers/web/logs?stdout=1&stderr=1&timestamps=1&tail=5000 HTTP/1.1"
            ),
            "{request}"
        );
    }

    #[tokio::test]
    async fn following_sends_batches_and_ends_when_the_log_does() {
        let body = String::from_utf8(frame(
            1,
            "2026-10-18T10:00:01Z one\n2026-10-18T10:00:02Z two\nlast",
        ))
        .unwrap();
        let mut fake = fake_engine(vec![(200, MULTIPLEXED, body)]);
        let (tell, mut heard) = tokio::sync::mpsc::unbounded_channel();
        let sink = StreamSink::new(move |item| tell.send(item).is_ok());
        follow(
            &fake.engine,
            FollowParams {
                id: "web".to_owned(),
                tail: None,
            },
            &sink,
        )
        .await
        .unwrap();
        let mut texts = Vec::new();
        let mut ended = false;
        while let Ok(batch) = heard.try_recv() {
            ended = batch["ended"].as_bool().unwrap();
            for line in batch["lines"].as_array().unwrap() {
                texts.push(line["text"].as_str().unwrap().to_owned());
            }
        }
        assert_eq!(texts, ["one", "two", "last"]);
        assert!(ended);
        assert!(fake
            .seen
            .recv()
            .await
            .unwrap()
            .contains("&tail=200&follow=1 HTTP/1.1"));
    }
}
