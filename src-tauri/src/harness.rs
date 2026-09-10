use std::collections::{HashMap, VecDeque};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Emitter, State};

use crate::cli_server::CliBrokerState;

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
        if !matches!(
            self.method.as_str(),
            "workspace.inspect"
                | "task.start"
                | "task.read"
                | "task.stop"
                | "ui.open"
                | "events.wait"
        ) {
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
    let id = request.id.clone();
    let receiver = broker.enqueue(request)?;
    let _ = app.emit("harness-request", ());
    let result = receiver.recv_timeout(Duration::from_secs(65))
        .unwrap_or_else(|_| Err("Harness request timed out; task.start may still complete. Retry with the same idempotencyKey.".into()));
    broker.remove(&id);
    result
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPage {
    pub bytes: Vec<u8>,
    pub cursor: u64,
    pub truncated: bool,
    pub has_more: bool,
}

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

    pub fn read(&self, cursor: u64, limit: usize) -> Result<OutputPage, String> {
        if cursor > self.end {
            return Err("output cursor is ahead of this execution".into());
        }
        if limit == 0 || limit > 8192 {
            return Err("output limit must be between 1 and 8192 bytes".into());
        }
        let start = self.end - self.bytes.len() as u64;
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
            truncated: cursor < start,
            has_more: next < self.end,
        })
    }
}

#[tauri::command]
pub fn harness_resolve_path(project: String, path: String) -> Result<String, String> {
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
        assert!(super::harness_resolve_path(
            project.path().to_string_lossy().into_owned(),
            outside.path().to_string_lossy().into_owned()
        )
        .is_err());
        let file = project.path().join("test.txt");
        std::fs::write(&file, "ok").unwrap();
        assert!(super::harness_resolve_path(
            project.path().to_string_lossy().into_owned(),
            "test.txt".into()
        )
        .is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), project.path().join("escape")).unwrap();
            assert!(super::harness_resolve_path(
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
}
