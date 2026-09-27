use std::collections::{HashMap, VecDeque};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Emitter, State};

use crate::cli_server::CliBrokerState;

mod terminal_text;

pub const MAX_PENDING: usize = 64;
pub const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessRequest {
    pub id: String,
    pub project: String,
    pub agent_id: Option<String>,
    pub method: String,
    pub params: Value,
}

impl HarnessRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.len() > 128 {
            return Err("request ID must contain 1 to 128 bytes".into());
        }
        if self.project.len() > 4096 || !std::path::Path::new(&self.project).is_absolute() {
            return Err("project must be an absolute path".into());
        }
        if self
            .agent_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return Err("invalid agent ID".into());
        }
        if !crate::generated_agent_tools::HARNESS_METHODS.contains(&self.method.as_str())
            && !crate::browser::tools::is_browser_method(&self.method)
            && !crate::plugins::agent::is_agent_method(&self.method)
        {
            return Err("unknown harness method".into());
        }
        if !self.params.is_object() {
            return Err("params must be an object".into());
        }
        Ok(())
    }
}

struct Pending {
    request: HarnessRequest,
    claimed: bool,
    reply: mpsc::Sender<Result<Value, String>>,
}

#[derive(Default)]
pub struct HarnessBroker {
    pending: Mutex<HashMap<String, Pending>>,
}

impl HarnessBroker {
    pub fn enqueue(
        &self,
        request: HarnessRequest,
    ) -> Result<mpsc::Receiver<Result<Value, String>>, String> {
        request.validate()?;
        let mut pending = self.pending.lock().map_err(|_| "harness lock poisoned")?;
        if pending.len() >= MAX_PENDING || pending.contains_key(&request.id) {
            return Err("harness request capacity reached or duplicate request ID".into());
        }
        let (reply, receiver) = mpsc::channel();
        pending.insert(
            request.id.clone(),
            Pending {
                request,
                claimed: false,
                reply,
            },
        );
        Ok(receiver)
    }

    fn claim(&self) -> Vec<HarnessRequest> {
        let Ok(mut pending) = self.pending.lock() else {
            return vec![];
        };
        pending
            .values_mut()
            .filter_map(|entry| {
                if entry.claimed {
                    return None;
                }
                entry.claimed = true;
                Some(entry.request.clone())
            })
            .collect()
    }

    pub fn remove(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }

    fn reply(&self, id: &str, result: Result<Value, String>) {
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(entry) = pending.remove(id) {
                let _ = entry.reply.send(result);
            }
        }
    }

    pub fn shutdown(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            for (_, entry) in pending.drain() {
                let _ = entry.reply.send(Err("Sikemux closed".into()));
            }
        }
    }
}

pub fn execute(
    app: &tauri::AppHandle,
    broker: &HarnessBroker,
    mut request: HarnessRequest,
) -> Result<Value, String> {
    request.validate()?;
    request.project = std::fs::canonicalize(&request.project)
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();
    if crate::browser::tools::is_browser_method(&request.method) {
        return crate::browser::tools::execute(app, &request);
    }
    if crate::plugins::agent::is_agent_method(&request.method) {
        return crate::plugins::agent::execute(app, &request.method, &request.params);
    }
    let id = request.id.clone();
    let inspecting = request.method == "workspace.inspect";
    let receiver = broker.enqueue(request)?;
    let _ = app.emit_to("main", "harness-request", ());
    let result = receiver.recv_timeout(Duration::from_secs(65))
        .unwrap_or_else(|_| Err("Harness request timed out; task.start may still complete. Retry with the same idempotencyKey.".into()));
    broker.remove(&id);
    match result {
        Ok(mut value) if inspecting => {
            if let (Some(object), Some(cli)) =
                (value.as_object_mut(), crate::cli_server::cli_command_path())
            {
                object.insert("cli".into(), cli.to_string_lossy().into());
            }
            Ok(value)
        }
        other => other,
    }
}

#[tauri::command]
pub fn harness_claim(state: State<'_, CliBrokerState>) -> Vec<HarnessRequest> {
    state
        .0
        .as_ref()
        .map(|broker| broker.harness().claim())
        .unwrap_or_default()
}

#[tauri::command]
pub fn harness_reply(
    state: State<'_, CliBrokerState>,
    id: String,
    result: Option<Value>,
    error: Option<String>,
) {
    if let Some(broker) = &state.0 {
        broker.harness().reply(
            &id,
            match error {
                Some(message) => Err(message),
                None => Ok(result.unwrap_or(Value::Null)),
            },
        );
    }
}

#[derive(Default)]
pub struct OutputLog {
    bytes: VecDeque<u8>,
    end: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct OutputQuery {
    pub cursor: u64,
    pub limit: usize,
    pub tail: Option<usize>,
    pub search: Option<String>,
    pub context: usize,
    pub plain: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPage {
    pub bytes: Vec<u8>,
    pub cursor: u64,
    pub end: u64,
    pub truncated: bool,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<usize>,
}

const PLAIN_WINDOW: u64 = 256 * 1024;

impl OutputLog {
    pub fn push(&mut self, bytes: &[u8]) {
        self.end += bytes.len() as u64;
        if bytes.len() >= MAX_OUTPUT {
            self.bytes.clear();
            self.bytes.extend(&bytes[bytes.len() - MAX_OUTPUT..]);
        } else {
            let overflow = (self.bytes.len() + bytes.len()).saturating_sub(MAX_OUTPUT);
            self.bytes.drain(..overflow);
            self.bytes.extend(bytes);
        }
    }

    fn start(&self) -> u64 {
        self.end - self.bytes.len() as u64
    }

    fn slice(&self, from: u64, to: u64) -> Vec<u8> {
        let start = self.start();
        self.bytes
            .range((from - start) as usize..(to - start) as usize)
            .copied()
            .collect()
    }

    pub fn query(&self, query: &OutputQuery) -> Result<OutputPage, String> {
        if query.cursor > self.end {
            return Err(format!(
                "output cursor {} is ahead of this execution (end {})",
                query.cursor, self.end
            ));
        }
        if query.limit == 0 || query.limit > 8192 {
            return Err("output limit must be between 1 and 8192 bytes".into());
        }
        if query.tail.is_some_and(|lines| lines == 0 || lines > 10_000) {
            return Err("tail must be between 1 and 10000 lines".into());
        }
        if query.context > 20 {
            return Err("context must be at most 20 lines".into());
        }
        match (&query.search, query.tail, query.plain) {
            (Some(needle), _, _) => self.search(needle, query),
            (None, Some(lines), false) => Ok(self.raw_tail(lines, query.limit)),
            (None, Some(lines), true) => Ok(self.plain_tail(lines, query.limit)),
            (None, None, true) => Ok(self.plain_page(query.cursor, query.limit)),
            (None, None, false) => self.read(query.cursor, query.limit),
        }
    }

    pub fn read(&self, cursor: u64, limit: usize) -> Result<OutputPage, String> {
        if cursor > self.end {
            return Err(format!(
                "output cursor {cursor} is ahead of this execution (end {})",
                self.end
            ));
        }
        if limit == 0 || limit > 8192 {
            return Err("output limit must be between 1 and 8192 bytes".into());
        }
        let start = self.start();
        let from = cursor.max(start);
        let mut bytes: Vec<u8> = self
            .bytes
            .iter()
            .skip((from - start) as usize)
            .take(limit)
            .copied()
            .collect();
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_none() {
                bytes.truncate(error.valid_up_to());
            }
        }
        let next = from + bytes.len() as u64;
        Ok(OutputPage {
            bytes,
            cursor: next,
            end: self.end,
            truncated: cursor < start,
            has_more: next < self.end,
            matches: None,
        })
    }

    fn tail_start(&self, lines: usize) -> u64 {
        let last = self.bytes.len().saturating_sub(1);
        let mut seen = 0;
        for (index, byte) in self.bytes.iter().enumerate().rev() {
            if *byte == b'\n' && index != last {
                seen += 1;
                if seen == lines {
                    return self.start() + index as u64 + 1;
                }
            }
        }
        self.start()
    }

    fn raw_tail(&self, lines: usize, limit: usize) -> OutputPage {
        let wanted = self.tail_start(lines);
        let mut from = wanted.max(self.end.saturating_sub(limit as u64));
        while from < self.end && self.bytes[(from - self.start()) as usize] & 0xc0 == 0x80 {
            from += 1;
        }
        OutputPage {
            bytes: self.slice(from, self.end),
            cursor: self.end,
            end: self.end,
            truncated: from > wanted || (wanted == self.start() && self.start() > 0),
            has_more: false,
            matches: None,
        }
    }

    fn plain_page(&self, cursor: u64, limit: usize) -> OutputPage {
        let start = self.start();
        let from = cursor.max(start);
        let to = self.end.min(from + PLAIN_WINDOW);
        let reaches_end = to == self.end;
        let mut lines = terminal_text::render(&self.slice(from, to), from);
        if !reaches_end && lines.len() > 1 {
            lines.pop();
        }
        let lines = terminal_text::collapse(lines);
        let (text, taken) = fit_from_start(&lines, limit);
        let next = match taken {
            all if all == lines.len() && reaches_end => self.end,
            0 => to,
            some => lines[some - 1].end,
        };
        OutputPage {
            bytes: text.into_bytes(),
            cursor: next,
            end: self.end,
            truncated: cursor < start,
            has_more: next < self.end,
            matches: None,
        }
    }

    fn plain_tail(&self, lines: usize, limit: usize) -> OutputPage {
        let from = self.start().max(self.end.saturating_sub(PLAIN_WINDOW));
        let rendered =
            terminal_text::collapse(terminal_text::render(&self.slice(from, self.end), from));
        let wanted = &rendered[rendered.len().saturating_sub(lines)..];
        let (text, taken) = fit_from_end(wanted, limit);
        OutputPage {
            bytes: text.into_bytes(),
            cursor: self.end,
            end: self.end,
            truncated: taken < wanted.len() || (wanted.len() < lines && from > 0),
            has_more: false,
            matches: None,
        }
    }

    fn search(&self, needle: &str, query: &OutputQuery) -> Result<OutputPage, String> {
        if needle.is_empty() || needle.len() > 4096 {
            return Err("search must be nonempty text of at most 4096 bytes".into());
        }
        let start = self.start();
        let from = query.cursor.max(start);
        let lines = terminal_text::render(&self.slice(from, self.end), from);
        let needle = needle.to_lowercase();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.text.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect();
        let chosen = match query.tail {
            Some(count) => &hits[hits.len().saturating_sub(count)..],
            None => &hits[..],
        };
        let mut groups: Vec<(usize, usize)> = Vec::new();
        for &hit in chosen {
            let low = hit.saturating_sub(query.context);
            let high = (hit + query.context + 1).min(lines.len());
            match groups.last_mut() {
                Some(last) if low <= last.1 => last.1 = last.1.max(high),
                _ => groups.push((low, high)),
            }
        }
        let mut shown: Vec<terminal_text::Line> = Vec::new();
        for (low, high) in groups {
            if let Some(previous) = shown.last() {
                shown.push(terminal_text::Line {
                    text: "--".into(),
                    end: previous.end,
                });
            }
            shown.extend_from_slice(&lines[low..high]);
        }
        let page = if query.tail.is_some() {
            let (text, taken) = fit_from_end(&shown, query.limit);
            OutputPage {
                bytes: text.into_bytes(),
                cursor: self.end,
                end: self.end,
                truncated: taken < shown.len(),
                has_more: false,
                matches: Some(hits.len()),
            }
        } else {
            let (text, taken) = fit_from_start(&shown, query.limit);
            let next = if taken == shown.len() {
                self.end
            } else {
                shown[taken - 1].end
            };
            OutputPage {
                bytes: text.into_bytes(),
                cursor: next,
                end: self.end,
                truncated: query.cursor < start,
                has_more: next < self.end,
                matches: Some(hits.len()),
            }
        };
        Ok(page)
    }
}

fn fit_from_start(lines: &[terminal_text::Line], limit: usize) -> (String, usize) {
    let mut text = String::new();
    let mut taken = 0;
    for line in lines {
        if text.len() + line.text.len() + 1 > limit {
            break;
        }
        text.push_str(&line.text);
        text.push('\n');
        taken += 1;
    }
    if taken == 0 {
        if let Some(line) = lines.first() {
            let mut cut = limit.min(line.text.len());
            while !line.text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.push_str(&line.text[..cut]);
            taken = 1;
        }
    }
    (text, taken)
}

fn fit_from_end(lines: &[terminal_text::Line], limit: usize) -> (String, usize) {
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0;
    for line in lines.iter().rev() {
        if size + line.text.len() + 1 > limit {
            break;
        }
        size += line.text.len() + 1;
        kept.push(&line.text);
    }
    if kept.is_empty() {
        if let Some(line) = lines.last() {
            let mut cut = line.text.len().saturating_sub(limit);
            while !line.text.is_char_boundary(cut) {
                cut += 1;
            }
            return (line.text[cut..].to_string(), 1);
        }
    }
    let taken = kept.len();
    let mut text = String::with_capacity(size);
    for line in kept.into_iter().rev() {
        text.push_str(line);
        text.push('\n');
    }
    (text, taken)
}

#[tauri::command]
pub async fn harness_resolve_path(project: String, path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || resolve_project_path(project, path))
        .await
        .map_err(|error| format!("harness_resolve_path join: {error}"))?
}

fn resolve_project_path(project: String, path: String) -> Result<String, String> {
    let root = std::fs::canonicalize(project).map_err(|error| error.to_string())?;
    let target = std::fs::canonicalize(root.join(path)).map_err(|error| error.to_string())?;
    if !target.starts_with(&root) || !target.is_file() {
        return Err("File must be inside the project".into());
    }
    Ok(target.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str) -> HarnessRequest {
        HarnessRequest {
            id: id.into(),
            project: "/tmp".into(),
            agent_id: None,
            method: "workspace.inspect".into(),
            params: serde_json::json!({}),
        }
    }
    #[test]
    fn queue_claims_once_replies_and_releases_capacity() {
        let broker = HarnessBroker::default();
        let receiver = broker.enqueue(request("one")).unwrap();
        assert!(broker.enqueue(request("one")).is_err());
        assert_eq!(broker.claim().len(), 1);
        assert!(broker.claim().is_empty());
        broker.reply("one", Ok(Value::Bool(true)));
        assert_eq!(receiver.recv().unwrap().unwrap(), Value::Bool(true));
        assert!(broker.enqueue(request("one")).is_ok());
        broker.shutdown();
    }
    #[test]
    fn output_pages_preserve_split_utf8() {
        let mut log = OutputLog::default();
        log.push("abcé".as_bytes());
        let first = log.read(0, 4).unwrap();
        assert_eq!(first.bytes, b"abc");
        assert_eq!(first.cursor, 3);
        assert_eq!(log.read(first.cursor, 4).unwrap().bytes, "é".as_bytes());
        log.push(&[0xe2, 0x82]);
        assert!(log.read(5, 8).unwrap().bytes.is_empty());
        log.push(&[0xac]);
        assert_eq!(log.read(5, 8).unwrap().bytes, "€".as_bytes());
    }

    #[test]
    fn file_open_rejects_paths_outside_project() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(super::resolve_project_path(
            project.path().to_string_lossy().into_owned(),
            outside.path().to_string_lossy().into_owned()
        )
        .is_err());
        let file = project.path().join("test.txt");
        std::fs::write(&file, "ok").unwrap();
        assert!(super::resolve_project_path(
            project.path().to_string_lossy().into_owned(),
            "test.txt".into()
        )
        .is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), project.path().join("escape")).unwrap();
            assert!(super::resolve_project_path(
                project.path().to_string_lossy().into_owned(),
                "escape".into()
            )
            .is_err());
        }
    }

    #[test]
    fn invalid_requests_and_full_queue_are_rejected() {
        let broker = HarnessBroker::default();
        let mut invalid = request("bad");
        invalid.method = "pty_kill".into();
        assert!(broker.enqueue(invalid).is_err());
        for i in 0..MAX_PENDING {
            broker.enqueue(request(&i.to_string())).unwrap();
        }
        assert!(broker.enqueue(request("overflow")).is_err());
        broker.remove("0");
        assert!(broker.enqueue(request("new")).is_ok());
    }
    #[test]
    fn output_cursors_page_without_repeating_and_report_eviction() {
        let mut log = OutputLog::default();
        log.push(b"abc");
        let page = log.read(0, 2).unwrap();
        assert_eq!(page.bytes, b"ab");
        assert!(page.has_more);
        assert_eq!(log.read(page.cursor, 2).unwrap().bytes, b"c");
        log.push(&vec![b'x'; MAX_OUTPUT + 5]);
        let page = log.read(0, 8192).unwrap();
        assert!(page.truncated);
        assert_eq!(page.cursor, 8 + 8192);
        assert!(log.read(u64::MAX, 1).is_err());
        assert!(log.read(0, 0).is_err());
    }

    fn query(log: &OutputLog, query: OutputQuery) -> (String, OutputPage) {
        let page = log
            .query(&OutputQuery {
                limit: 8192,
                ..query
            })
            .unwrap();
        (String::from_utf8(page.bytes.clone()).unwrap(), page)
    }

    #[test]
    fn every_page_and_the_ahead_error_report_the_end() {
        let mut log = OutputLog::default();
        log.push(b"hello\n");
        assert_eq!(log.read(0, 2).unwrap().end, 6);
        assert_eq!(
            log.read(99, 2).unwrap_err(),
            "output cursor 99 is ahead of this execution (end 6)"
        );
    }

    #[test]
    fn tail_reads_the_last_lines_within_the_limit() {
        let mut log = OutputLog::default();
        log.push(b"one\ntwo\nthree\nfour\n");
        let (text, page) = query(
            &log,
            OutputQuery {
                tail: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(text, "three\nfour\n");
        assert_eq!(
            (page.cursor, page.has_more, page.truncated),
            (19, false, false)
        );
        let cut = log
            .query(&OutputQuery {
                tail: Some(3),
                limit: 6,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(cut.bytes, b"\nfour\n");
        assert!(cut.truncated);
        log.push("\u{e9}\u{e9}".as_bytes());
        let split = log
            .query(&OutputQuery {
                tail: Some(1),
                limit: 3,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(split.bytes, "\u{e9}".as_bytes());
        assert!(log
            .query(&OutputQuery {
                tail: Some(0),
                limit: 10,
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn search_returns_matching_lines_with_context() {
        let mut log = OutputLog::default();
        log.push(b"a\nb\n\x1b[31mERROR\x1b[0m one\nc\nd\ne\nf\nerror two\ng\n");
        let (text, page) = query(
            &log,
            OutputQuery {
                search: Some("error".into()),
                context: 1,
                ..Default::default()
            },
        );
        assert_eq!(text, "b\nERROR one\nc\n--\nf\nerror two\ng\n");
        assert_eq!((page.matches, page.has_more), (Some(2), false));
        let (latest, _) = query(
            &log,
            OutputQuery {
                search: Some("error".into()),
                tail: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(latest, "error two\n");
        let first = log
            .query(&OutputQuery {
                search: Some("error".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first.bytes, b"ERROR one\n");
        assert!(first.has_more);
        let (rest, _) = query(
            &log,
            OutputQuery {
                search: Some("error".into()),
                cursor: first.cursor,
                ..Default::default()
            },
        );
        assert_eq!(rest, "error two\n");
    }

    #[test]
    fn plain_pages_replay_redraws_and_collapse_repeats() {
        let mut log = OutputLog::default();
        log.push(b"\x1b[1mstep 1/2\x1b[0m\rstep 2/2\n");
        log.push(&b"same\n".repeat(50));
        log.push(b"done\n");
        let (text, page) = query(
            &log,
            OutputQuery {
                plain: true,
                ..Default::default()
            },
        );
        assert_eq!(text, "step 2/2\nsame\n[repeated 49 more times]\ndone\n");
        assert_eq!((page.cursor, page.has_more), (log.end, false));
        let (tail, _) = query(
            &log,
            OutputQuery {
                plain: true,
                tail: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(tail, "[repeated 49 more times]\ndone\n");
        let first = log
            .query(&OutputQuery {
                plain: true,
                limit: 14,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first.bytes, b"step 2/2\nsame\n");
        assert!(first.has_more);
        let (rest, _) = query(
            &log,
            OutputQuery {
                plain: true,
                cursor: first.cursor,
                ..Default::default()
            },
        );
        assert_eq!(rest, "same\n[repeated 48 more times]\ndone\n");
    }

    #[test]
    fn plain_reads_do_not_loop_on_trailing_escape_codes() {
        let mut log = OutputLog::default();
        log.push(b"ready\n\x1b[0m");
        let page = log
            .query(&OutputQuery {
                plain: true,
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.bytes, b"ready\n");
        assert_eq!((page.cursor, page.has_more), (log.end, false));
    }
}
