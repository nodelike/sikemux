//! What the `sikemux` CLI and the agents' tool server hand the core to pass on.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_CLI_TARGETS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliOpenTarget {
    pub id: String,
    pub kind: CliTargetKind,
    pub path: String,
    pub project_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CliTargetKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliOpenRequest {
    pub id: String,
    pub cwd: String,
    pub wait: bool,
    pub targets: Vec<CliOpenTarget>,
}

impl CliOpenRequest {
    pub fn validate(&self) -> Result<(), String> {
        use std::path::Path;
        if self.id.trim().is_empty() || self.targets.is_empty() {
            return Err("CLI open request has no targets".into());
        }
        if !Path::new(&self.cwd).is_absolute() {
            return Err("CLI working directory must be an absolute path".into());
        }
        if self.targets.len() > MAX_CLI_TARGETS {
            return Err(format!(
                "CLI open request exceeds {MAX_CLI_TARGETS} targets"
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for target in &self.targets {
            if target.id.trim().is_empty() {
                return Err("CLI open request has an empty target id".into());
            }
            if !ids.insert(&target.id) {
                return Err("CLI open request has duplicate target ids".into());
            }
            if !Path::new(&target.path).is_absolute()
                || !Path::new(&target.project_root).is_absolute()
            {
                return Err("CLI targets and project roots must be absolute paths".into());
            }
            if target.kind == CliTargetKind::Directory
                && (target.line.is_some() || target.column.is_some())
            {
                return Err("directory targets cannot include a line or column".into());
            }
        }
        if self.wait
            && self
                .targets
                .iter()
                .all(|target| target.kind == CliTargetKind::Directory)
        {
            return Err("--wait requires at least one file target".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessRequest {
    pub id: String,
    pub project: String,
    pub agent_id: Option<String>,
    pub method: String,
    pub params: Value,
}
