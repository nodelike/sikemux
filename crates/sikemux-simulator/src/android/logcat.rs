//! A device's log as `adb logcat` prints it, kept as numbered lines so an agent
//! can ask for what came after the last line it read, as the iOS helper keeps its log.

use sikemux_process::command::Stdio;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::tools::{clip, LogFilter, MAX_LOG_LINE};

const CAPACITY: usize = 2000;
const PID_LIMIT: usize = 64;
const BATCH_LIMIT: usize = 64;
const READ_BUFFER: usize = 64 * 1024;

pub(crate) struct Logcat {
    generation: i64,
    lines: Arc<Mutex<Lines>>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct Lines {
    all: Ring,
    by_pid: HashMap<i64, PidRing>,
    uses: u64,
}

#[derive(Default)]
struct Ring {
    entries: VecDeque<String>,
    last: u64,
}

#[derive(Default)]
struct PidRing {
    entries: VecDeque<(u64, String)>,
    last: u64,
    used: u64,
}

impl Ring {
    fn push(&mut self, line: String) {
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(line);
        self.last += 1;
    }

    fn read(
        &self,
        cursor: u64,
        limit: usize,
        generation_mismatch: bool,
        filter: &LogFilter,
        max_bytes: Option<usize>,
    ) -> (Vec<Value>, u64, u64, bool, bool) {
        let reset = generation_mismatch || cursor > self.last;
        let after = if reset { 0 } else { cursor };
        let first = self.last + 1 - self.entries.len() as u64;
        let start = (after + 1).max(first);
        let dropped = first.saturating_sub(after + 1);
        let (picked, at, more) = if start <= self.last {
            let from = (start - first) as usize;
            let iter = self
                .entries
                .range(from..)
                .enumerate()
                .map(|(offset, line)| (start + offset as u64, line.as_str()));
            pick_filtered(iter, self.last, after, limit, filter, max_bytes)
        } else {
            (Vec::new(), after.max(start.saturating_sub(1)), false)
        };
        (picked, at, dropped, more, reset)
    }
}

impl PidRing {
    fn push(&mut self, seq: u64, line: String, used: u64) {
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back((seq, line));
        self.last += 1;
        self.used = used;
    }

    fn read(
        &self,
        cursor: u64,
        limit: usize,
        generation_mismatch: bool,
        filter: &LogFilter,
        max_bytes: Option<usize>,
    ) -> (Vec<Value>, u64, u64, bool, bool) {
        let reset = generation_mismatch || cursor > self.last;
        let after = if reset { 0 } else { cursor };
        let first = self.last + 1 - self.entries.len() as u64;
        let start = (after + 1).max(first);
        let dropped = first.saturating_sub(after + 1);
        let (picked, at, more) = if start <= self.last {
            let from = (start - first) as usize;
            let iter = self
                .entries
                .range(from..)
                .enumerate()
                .map(|(offset, (_, line))| (start + offset as u64, line.as_str()));
            pick_filtered(iter, self.last, after, limit, filter, max_bytes)
        } else {
            (Vec::new(), after.max(start.saturating_sub(1)), false)
        };
        (picked, at, dropped, more, reset)
    }
}

fn pick_filtered<'a>(
    iter: impl Iterator<Item = (u64, &'a str)>,
    last_scanned: u64,
    after: u64,
    limit: usize,
    filter: &LogFilter,
    max_bytes: Option<usize>,
) -> (Vec<Value>, u64, bool) {
    let mut picked = Vec::new();
    let mut used = 0usize;
    let mut last_picked = after;
    let mut more = false;
    for (seq, line) in iter {
        if !filter.matches_android(line) {
            continue;
        }
        if picked.len() >= limit {
            more = true;
            break;
        }
        let clipped = clip(line, MAX_LOG_LINE);
        let cost = clipped.len() + 4;
        if max_bytes.is_some_and(|budget| used + cost > budget) && !picked.is_empty() {
            more = true;
            break;
        }
        used += cost;
        last_picked = seq;
        picked.push(Value::String(clipped));
    }
    let at = if more {
        last_picked
    } else {
        last_scanned.max(after)
    };
    (picked, at, more)
}

impl Lines {
    fn push_batch(&mut self, batch: &mut Vec<(i64, String)>) {
        for (pid, line) in batch.drain(..) {
            self.all.push(line.clone());
            if pid <= 0 {
                continue;
            }
            self.uses += 1;
            let seq = self.all.last;
            let used = self.uses;
            self.by_pid.entry(pid).or_default().push(seq, line, used);
            if self.by_pid.len() > PID_LIMIT {
                if let Some(stalest) = self
                    .by_pid
                    .iter()
                    .min_by_key(|(_, ring)| ring.used)
                    .map(|(pid, _)| *pid)
                {
                    self.by_pid.remove(&stalest);
                }
            }
        }
    }

    fn read_many(
        &mut self,
        pids: &[i64],
        cursor: u64,
        limit: usize,
        generation_mismatch: bool,
        filter: &LogFilter,
        max_bytes: Option<usize>,
    ) -> (Vec<Value>, u64, u64, bool, bool) {
        let mut max_seq = 0u64;
        for pid in pids {
            if let Some(ring) = self.by_pid.get_mut(pid) {
                self.uses += 1;
                ring.used = self.uses;
                if let Some((seq, _)) = ring.entries.back() {
                    max_seq = max_seq.max(*seq);
                }
            }
        }
        let reset = generation_mismatch || cursor > max_seq;
        let after = if reset { 0 } else { cursor };
        let mut merged: Vec<(u64, &str)> = Vec::new();
        let mut dropped = 0u64;
        for pid in pids {
            let Some(ring) = self.by_pid.get(pid) else {
                continue;
            };
            let evicted = ring.last.saturating_sub(ring.entries.len() as u64);
            if evicted > 0
                && ring
                    .entries
                    .front()
                    .is_some_and(|(seq, _)| *seq > after + 1)
            {
                dropped += evicted;
            }
            for (seq, line) in &ring.entries {
                if *seq > after {
                    merged.push((*seq, line.as_str()));
                }
            }
        }
        merged.sort_unstable_by_key(|(seq, _)| *seq);
        let (picked, at, more) =
            pick_filtered(merged.into_iter(), max_seq, after, limit, filter, max_bytes);
        (picked, at, dropped, more, reset)
    }
}

impl Drop for Logcat {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Logcat {
    /// Follows the device's log from now on.
    pub fn follow(adb: &Path, serial: &str) -> Result<Arc<Self>, String> {
        let mut child = sikemux_process::command::Command::into_async(
            sikemux_process::user_environment::command(adb),
        )
        .args(["-s", serial, "logcat", "-v", "threadtime", "-T", "1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not read the device's log: {error}"))?;
        let stdout = child.stdout.take().ok_or("the log has no output")?;
        let lines = Arc::new(Mutex::new(Lines::default()));
        let filled = Arc::clone(&lines);
        let task = tokio::spawn(async move {
            let _child = child;
            let mut reader = BufReader::with_capacity(READ_BUFFER, stdout);
            let mut raw = Vec::new();
            let mut batch = Vec::with_capacity(BATCH_LIMIT);
            loop {
                raw.clear();
                match reader.read_until(b'\n', &mut raw).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                while raw
                    .last()
                    .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
                {
                    raw.pop();
                }
                let line = String::from_utf8_lossy(&raw).into_owned();
                batch.push((parse_pid(&line), line));
                if batch.len() >= BATCH_LIMIT || reader.buffer().is_empty() {
                    let full = batch.len() >= BATCH_LIMIT;
                    filled.locked().push_batch(&mut batch);
                    if full {
                        tokio::task::yield_now().await;
                    }
                }
            }
            if !batch.is_empty() {
                filled.locked().push_batch(&mut batch);
            }
        });
        Ok(Arc::new(Self {
            generation: rand_generation(),
            lines,
            task,
        }))
    }

    pub fn is_alive(&self) -> bool {
        !self.task.is_finished()
    }

    /// The lines after `cursor`, at most `limit`, of every process or only of `pids`.
    pub fn read(
        &self,
        pids: Option<&[i64]>,
        cursor: u64,
        limit: usize,
        generation: Option<i64>,
        filter: &LogFilter,
        max_bytes: Option<usize>,
    ) -> Value {
        let mut guard = self.lines.locked();
        let lines = &mut *guard;
        let generation_mismatch =
            generation.is_some_and(|generation| generation != self.generation);
        let (picked, at, dropped, more, reset) = match pids {
            None => lines
                .all
                .read(cursor, limit, generation_mismatch, filter, max_bytes),
            Some([]) => {
                let reset = generation_mismatch || cursor > 0;
                (Vec::new(), 0, 0, false, reset)
            }
            Some([pid]) => {
                if let Some(ring) = lines.by_pid.get_mut(pid) {
                    lines.uses += 1;
                    ring.used = lines.uses;
                    ring.read(cursor, limit, generation_mismatch, filter, max_bytes)
                } else {
                    let reset = generation_mismatch || cursor > 0;
                    (Vec::new(), 0, 0, false, reset)
                }
            }
            Some(many) => {
                lines.read_many(many, cursor, limit, generation_mismatch, filter, max_bytes)
            }
        };
        json!({
            "lines": picked,
            "cursor": at,
            "dropped": dropped,
            "more": more,
            "reset": reset,
            "generation": self.generation,
        })
    }
}

fn parse_pid(line: &str) -> i64 {
    line.split_whitespace()
        .nth(2)
        .and_then(|pid| pid.parse().ok())
        .unwrap_or(-1)
}

fn rand_generation() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |since| since.subsec_nanos());
    i64::from(nanos % i32::MAX as u32) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_pid_buffer_survives_system_log_spam() {
        let mut lines = Lines::default();
        let mut batch = Vec::new();
        for i in 0..8 {
            batch.push((
                6484,
                format!("11-28 01:28:00.00{i}  6484  6484 I Academy: msg {i}"),
            ));
        }
        for i in 0..5000 {
            batch.push((
                1000,
                format!("11-28 01:28:01.000  1000  1001 D System: spam {i}"),
            ));
        }
        lines.push_batch(&mut batch);

        let none = LogFilter::default();
        let all = lines.all.read(0, 50, false, &none, None);
        assert_eq!(all.2, 5008 - CAPACITY as u64);

        let app = lines
            .by_pid
            .get(&6484)
            .expect("app ring kept")
            .read(0, 50, false, &none, None);
        assert_eq!(app.0.len(), 8);
        assert_eq!(app.1, 8);
        assert_eq!(app.2, 0);
        assert!(!app.3);
        assert!(!app.4);

        let next = lines
            .by_pid
            .get(&6484)
            .unwrap()
            .read(8, 50, false, &none, None);
        assert!(next.0.is_empty());
        assert_eq!(next.1, 8);
        assert_eq!(next.2, 0);
    }

    #[test]
    fn multi_pid_read_merges_in_sequence_order() {
        let mut lines = Lines::default();
        let mut batch = vec![
            (100, "11-28 01:28:00.001  100  100 I App: a1".into()),
            (999, "11-28 01:28:00.002  999  999 D Sys: noise".into()),
            (200, "11-28 01:28:00.003  200  200 I App: b1".into()),
            (100, "11-28 01:28:00.004  100  100 I App: a2".into()),
        ];
        lines.push_batch(&mut batch);

        let none = LogFilter::default();
        let first = lines.read_many(&[100, 200], 0, 2, false, &none, None);
        assert_eq!(
            first.0,
            vec![
                Value::String("11-28 01:28:00.001  100  100 I App: a1".into()),
                Value::String("11-28 01:28:00.003  200  200 I App: b1".into()),
            ]
        );
        assert_eq!(first.1, 3);
        assert_eq!(first.2, 0);
        assert!(first.3);

        let rest = lines.read_many(&[100, 200], first.1, 10, false, &none, None);
        assert_eq!(
            rest.0,
            vec![Value::String(
                "11-28 01:28:00.004  100  100 I App: a2".into()
            )]
        );
        assert_eq!(rest.1, 4);
        assert!(!rest.3);
    }

    #[test]
    fn level_and_tag_filter_before_limit_and_preserve_cursor_paging() {
        use crate::tools::LogLevel;
        let mut lines = Lines::default();
        let mut batch = vec![
            (-1, "--------- beginning of main".into()),
            (
                6484,
                "11-28 01:28:00.001  6484  6484 D ReactNativeJS: verbose noise".into(),
            ),
            (
                6484,
                "11-28 01:28:00.002  6484  6484 I ReactNativeJS: info noise".into(),
            ),
            (
                6484,
                "11-28 01:28:00.003  6484  6484 W ReactNativeJS: first warn".into(),
            ),
            (
                6484,
                "11-28 01:28:00.004  6484  6484 E OtherTag: wrong tag".into(),
            ),
            (
                6484,
                "11-28 01:28:00.005  6484  6484 E ReactNativeJS: first err".into(),
            ),
            (
                6484,
                "11-28 01:28:00.006  6484  6484 F ReactNativeJS: fatal err".into(),
            ),
            (
                6484,
                "11-28 01:28:00.007  6484  6484 D ReactNativeJS: trailing noise".into(),
            ),
        ];
        lines.push_batch(&mut batch);

        let filter = LogFilter {
            level: Some(LogLevel::Warn),
            tag: Some("reactnativejs".into()),
        };
        let pid_ring = lines.by_pid.get(&6484).expect("pid ring");
        let page1 = pid_ring.read(0, 2, false, &filter, None);
        assert_eq!(
            page1.0,
            vec![
                Value::String("11-28 01:28:00.003  6484  6484 W ReactNativeJS: first warn".into()),
                Value::String("11-28 01:28:00.005  6484  6484 E ReactNativeJS: first err".into()),
            ]
        );
        assert_eq!(page1.1, 5);
        assert!(page1.3);

        let page2 = pid_ring.read(page1.1, 2, false, &filter, None);
        assert_eq!(
            page2.0,
            vec![Value::String(
                "11-28 01:28:00.006  6484  6484 F ReactNativeJS: fatal err".into()
            )]
        );
        assert_eq!(page2.1, 7);
        assert!(!page2.3);

        let page3 = pid_ring.read(page2.1, 2, false, &filter, None);
        assert!(page3.0.is_empty());
        assert_eq!(page3.1, 7);
        assert!(!page3.3);
    }
}
