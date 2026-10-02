//! The tasks agents started through the harness, and the idempotency keys that
//! name them.

use serde::{Deserialize, Serialize};

pub const MAX_RUNS: usize = 128;
pub const MAX_KEYS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    AwaitingTrust,
    Starting,
    Running,
    Completed,
    Failed,
    Stopping,
    Stopped,
}

impl RunStatus {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::AwaitingTrust | Self::Starting | Self::Running | Self::Stopping
        )
    }

    pub fn event_kind(self) -> &'static str {
        match self {
            Self::AwaitingTrust => "task.awaiting-trust",
            Self::Starting => "task.starting",
            Self::Running => "task.running",
            Self::Completed => "task.completed",
            Self::Failed => "task.failed",
            Self::Stopping => "task.stopping",
            Self::Stopped => "task.stopped",
        }
    }
}

/// What a `command` task runs, so the same task id can start it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandLaunch {
    pub command: String,
    /// Relative to the project; empty for the project root.
    pub cwd: String,
    pub label: String,
}

/// Where the window got to with launching a run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Launch {
    #[default]
    Pending,
    Done,
    Failed(String),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub execution_id: String,
    pub task_id: String,
    pub project: String,
    pub status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pty_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    #[serde(skip)]
    pub agent_id: Option<String>,
    #[serde(skip)]
    pub launch_spec: Option<CommandLaunch>,
    #[serde(skip)]
    pub launch: Launch,
}

impl Run {
    pub fn new(execution_id: String, task_id: String, project: String) -> Self {
        Self {
            execution_id,
            task_id,
            project,
            status: RunStatus::Starting,
            label: None,
            command: None,
            pty_id: None,
            exit_code: None,
            signal: None,
            error: None,
            preview_url: None,
            agent_id: None,
            launch_spec: None,
            launch: Launch::Pending,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Key {
    project: String,
    key: String,
    task_id: String,
    execution_id: String,
}

/// A run with every field, for handing over to a replacement core.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub execution_id: String,
    pub task_id: String,
    pub project: String,
    pub status: RunStatus,
    pub label: Option<String>,
    pub command: Option<String>,
    pub pty_id: Option<u64>,
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
    pub error: Option<String>,
    pub preview_url: Option<String>,
    pub agent_id: Option<String>,
    pub launch_spec: Option<CommandLaunch>,
    pub launch: Launch,
}

impl From<&Run> for RunRecord {
    fn from(run: &Run) -> Self {
        let run = run.clone();
        Self {
            execution_id: run.execution_id,
            task_id: run.task_id,
            project: run.project,
            status: run.status,
            label: run.label,
            command: run.command,
            pty_id: run.pty_id,
            exit_code: run.exit_code,
            signal: run.signal,
            error: run.error,
            preview_url: run.preview_url,
            agent_id: run.agent_id,
            launch_spec: run.launch_spec,
            launch: run.launch,
        }
    }
}

impl From<RunRecord> for Run {
    fn from(record: RunRecord) -> Self {
        Self {
            execution_id: record.execution_id,
            task_id: record.task_id,
            project: record.project,
            status: record.status,
            label: record.label,
            command: record.command,
            pty_id: record.pty_id,
            exit_code: record.exit_code,
            signal: record.signal,
            error: record.error,
            preview_url: record.preview_url,
            agent_id: record.agent_id,
            launch_spec: record.launch_spec,
            launch: record.launch,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunsRecord {
    pub runs: Vec<RunRecord>,
    pub keys: Vec<Key>,
}

/// Runs in the order they started. Room is made by forgetting the oldest
/// finished run, never one still going.
#[derive(Default)]
pub struct Runs {
    runs: Vec<Run>,
    keys: Vec<Key>,
}

pub const NOT_IN_PROJECT: &str = "Task execution does not belong to this project";

impl Runs {
    pub fn record(&self) -> RunsRecord {
        RunsRecord {
            runs: self.runs.iter().map(RunRecord::from).collect(),
            keys: self.keys.clone(),
        }
    }

    pub fn restored(record: RunsRecord) -> Self {
        Self {
            runs: record.runs.into_iter().map(Run::from).collect(),
            keys: record.keys,
        }
    }

    pub fn all_mut(&mut self) -> impl Iterator<Item = &mut Run> {
        self.runs.iter_mut()
    }

    pub fn get(&self, project: &str, execution_id: &str) -> Result<&Run, String> {
        self.runs
            .iter()
            .find(|run| run.execution_id == execution_id && run.project == project)
            .ok_or_else(|| NOT_IN_PROJECT.into())
    }

    pub fn by_execution(&self, execution_id: &str) -> Option<&Run> {
        self.runs
            .iter()
            .find(|run| run.execution_id == execution_id)
    }

    pub fn by_execution_mut(&mut self, execution_id: &str) -> Option<&mut Run> {
        self.runs
            .iter_mut()
            .find(|run| run.execution_id == execution_id)
    }

    pub fn by_pty_mut(&mut self, pty_id: u64) -> Option<&mut Run> {
        self.runs.iter_mut().find(|run| run.pty_id == Some(pty_id))
    }

    pub fn latest(&self, project: &str, task_id: &str) -> Option<&Run> {
        self.runs
            .iter()
            .rev()
            .find(|run| run.project == project && run.task_id == task_id)
    }

    /// The command the task ran last, for a task started from a `command`.
    pub fn launch_spec(&self, project: &str, task_id: &str) -> Option<&CommandLaunch> {
        self.runs
            .iter()
            .rev()
            .filter(|run| run.project == project && run.task_id == task_id)
            .find_map(|run| run.launch_spec.as_ref())
    }

    pub fn list(&self, project: &str) -> Vec<Run> {
        self.runs
            .iter()
            .filter(|run| run.project == project)
            .cloned()
            .collect()
    }

    pub fn active(&self, project: &str, task_id: &str) -> Option<&Run> {
        self.runs
            .iter()
            .find(|run| run.project == project && run.task_id == task_id && run.status.is_active())
    }

    pub fn matching(&self, keep: impl Fn(&Run) -> bool) -> Vec<String> {
        self.runs
            .iter()
            .filter(|run| keep(run))
            .map(|run| run.execution_id.clone())
            .collect()
    }

    /// The execution a key already names, if any.
    pub fn keyed(&self, project: &str, key: &str, task_id: &str) -> Result<Option<String>, String> {
        let Some(entry) = self
            .keys
            .iter()
            .find(|entry| entry.project == project && entry.key == key)
        else {
            return Ok(None);
        };
        if entry.task_id != task_id {
            return Err("idempotencyKey was already used for another task".into());
        }
        Ok(Some(entry.execution_id.clone()))
    }

    fn finished(&self, execution_id: &str) -> bool {
        self.by_execution(execution_id)
            .is_none_or(|run| !run.status.is_active())
    }

    pub fn remember_key(
        &mut self,
        project: &str,
        key: &str,
        task_id: &str,
        execution_id: &str,
    ) -> Result<(), String> {
        if self.keys.len() >= MAX_KEYS {
            let oldest = self
                .keys
                .iter()
                .position(|entry| self.finished(&entry.execution_id))
                .ok_or("Too many idempotency keys name tasks that are still running; stop one with task_stop first")?;
            self.keys.remove(oldest);
        }
        self.keys.push(Key {
            project: project.to_owned(),
            key: key.to_owned(),
            task_id: task_id.to_owned(),
            execution_id: execution_id.to_owned(),
        });
        Ok(())
    }

    pub fn remove(&mut self, execution_id: &str) {
        self.runs.retain(|run| run.execution_id != execution_id);
        self.keys.retain(|entry| entry.execution_id != execution_id);
    }

    /// Adds a run, forgetting the oldest finished one and its keys when full.
    pub fn insert(&mut self, run: Run) -> Result<(), String> {
        if self.runs.len() >= MAX_RUNS {
            let oldest = self
                .runs
                .iter()
                .position(|run| !run.status.is_active())
                .ok_or("Too many harness tasks are still running; stop one with task_stop first")?;
            let evicted = self.runs.remove(oldest);
            self.keys
                .retain(|entry| entry.execution_id != evicted.execution_id);
        }
        self.runs.push(run);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, task: &str) -> Run {
        Run::new(id.into(), task.into(), "/one".into())
    }

    #[test]
    fn keys_name_one_task_and_runs_are_scoped_to_their_project() {
        let mut runs = Runs::default();
        runs.insert(run("a", "dev")).unwrap();
        runs.remember_key("/one", "first", "dev", "a").unwrap();
        assert_eq!(
            runs.keyed("/one", "first", "dev").unwrap().as_deref(),
            Some("a")
        );
        assert!(runs.keyed("/one", "first", "test").is_err());
        assert_eq!(runs.keyed("/two", "first", "dev").unwrap(), None);
        assert!(runs.get("/one", "a").is_ok());
        assert_eq!(runs.get("/two", "a").unwrap_err(), NOT_IN_PROJECT);
        assert_eq!(
            runs.active("/one", "dev")
                .map(|run| run.execution_id.as_str()),
            Some("a")
        );
    }

    #[test]
    fn a_full_table_forgets_the_oldest_finished_run_and_its_keys() {
        let mut runs = Runs::default();
        for index in 0..MAX_RUNS {
            runs.insert(run(&index.to_string(), "dev")).unwrap();
        }
        assert!(runs.insert(run("late", "dev")).is_err());

        runs.by_execution_mut("5").unwrap().status = RunStatus::Completed;
        runs.by_execution_mut("9").unwrap().status = RunStatus::Stopped;
        runs.remember_key("/one", "five", "dev", "5").unwrap();
        runs.insert(run("late", "dev")).unwrap();
        assert!(runs.by_execution("5").is_none());
        assert!(runs.by_execution("9").is_some());
        assert_eq!(runs.keyed("/one", "five", "dev").unwrap(), None);
        assert_eq!(runs.latest("/one", "dev").unwrap().execution_id, "late");
    }

    #[test]
    fn a_full_key_list_forgets_keys_of_finished_runs_first() {
        let mut runs = Runs::default();
        runs.insert(run("live", "dev")).unwrap();
        runs.insert(run("done", "test")).unwrap();
        runs.by_execution_mut("done").unwrap().status = RunStatus::Failed;
        runs.remember_key("/one", "kept", "dev", "live").unwrap();
        for index in 1..MAX_KEYS {
            runs.remember_key("/one", &index.to_string(), "test", "done")
                .unwrap();
        }
        runs.remember_key("/one", "new", "test", "done").unwrap();
        assert!(runs.keyed("/one", "kept", "dev").unwrap().is_some());
        assert_eq!(runs.keyed("/one", "1", "test").unwrap(), None);

        let mut busy = Runs::default();
        busy.insert(run("live", "dev")).unwrap();
        for index in 0..MAX_KEYS {
            busy.remember_key("/one", &index.to_string(), "dev", "live")
                .unwrap();
        }
        assert!(busy.remember_key("/one", "more", "dev", "live").is_err());
    }

    #[test]
    fn a_command_task_remembers_what_it_ran() {
        let mut runs = Runs::default();
        let mut first = run("a", "sh:dev-123456");
        first.launch_spec = Some(CommandLaunch {
            command: "pnpm dev".into(),
            cwd: String::new(),
            label: "pnpm dev".into(),
        });
        runs.insert(first).unwrap();
        runs.insert(run("b", "sh:dev-123456")).unwrap();
        assert_eq!(
            runs.launch_spec("/one", "sh:dev-123456")
                .map(|spec| spec.command.as_str()),
            Some("pnpm dev")
        );
        assert_eq!(
            serde_json::to_value(runs.get("/one", "a").unwrap()).unwrap(),
            serde_json::json!({
                "executionId": "a",
                "taskId": "sh:dev-123456",
                "project": "/one",
                "status": "starting"
            })
        );
    }
}
