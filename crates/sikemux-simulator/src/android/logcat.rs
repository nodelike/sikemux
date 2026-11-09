//! A device's log as `adb logcat` prints it, kept as numbered lines so an agent
//! can ask for what came after the last line it read, as the iOS helper keeps its log.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::io::{AsyncBufReadExt, BufReader};

const CAPACITY: usize = 2000;

pub(crate) struct Logcat {
    generation: i64,
    lines: Arc<Mutex<Lines>>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct Lines {
    ring: VecDeque<(i64, String)>,
    last: u64,
}

impl Drop for Logcat {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Logcat {
    /// Follows the device's log from now on.
    pub fn follow(adb: &Path, serial: &str) -> Result<Arc<Self>, String> {
        let mut child =
            tokio::process::Command::from(sikemux_process::user_environment::command(adb))
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
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let pid = line
                    .split_whitespace()
                    .nth(2)
                    .and_then(|pid| pid.parse().ok())
                    .unwrap_or(-1);
                let mut lines = filled.locked();
                if lines.ring.len() == CAPACITY {
                    lines.ring.pop_front();
                }
                lines.ring.push_back((pid, line));
                lines.last += 1;
            }
        });
        Ok(Arc::new(Self {
            generation: rand_generation(),
            lines,
            task,
        }))
    }

    /// The lines after `cursor`, at most `limit`, of every process or only of `pids`.
    pub fn read(
        &self,
        pids: Option<&[i64]>,
        cursor: u64,
        limit: usize,
        generation: Option<i64>,
    ) -> Value {
        let lines = self.lines.locked();
        let reset = generation.is_some_and(|generation| generation != self.generation)
            || cursor > lines.last;
        let after = if reset { 0 } else { cursor };
        let first = lines.last + 1 - lines.ring.len() as u64;
        let mut picked = Vec::new();
        let mut at = after;
        for (offset, (pid, line)) in lines.ring.iter().enumerate() {
            let number = first + offset as u64;
            if number <= after {
                continue;
            }
            if picked.len() == limit {
                break;
            }
            at = number;
            if pids.is_none_or(|pids| pids.contains(pid)) {
                picked.push(Value::String(line.clone()));
            }
        }
        if at == after {
            at = lines.last.max(after);
        }
        json!({
            "lines": picked,
            "cursor": at,
            "dropped": first.saturating_sub(after + 1),
            "more": at < lines.last,
            "reset": reset,
            "generation": self.generation,
        })
    }
}

fn rand_generation() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |since| since.subsec_nanos());
    i64::from(nanos % i32::MAX as u32) + 1
}
