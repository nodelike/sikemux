use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use sikemux_plugin_api::{FailureCategory, PluginError};
use sikemux_view::plugins::TokenForm;
use sikemux_view::workspace::{CiAbilities, PullAbilities};

const MAX_ID_LENGTH: usize = 128;
const MAX_TOOL_NAME_LENGTH: usize = 64;
const MAX_CALL_TIMEOUT_SECS: u64 = 600;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_timeout_secs: Option<u64>,
    #[serde(default)]
    pub tools: Vec<AgentTool>,
    /// Variables the plugin reads from the person's shell. An app opened from
    /// the Dock starts without them, so the host copies them in from the login shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    /// The methods that sign the person in or out or pick another account, after
    /// which clients read the plugins again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub account_methods: Vec<String>,
    /// What the core and clients know of the code host the plugin speaks for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_host: Option<CodeHostSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_form: Option<TokenForm>,
}

/// A code host as its plugin describes it, so the core and clients reach and
/// show it without naming it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeHostSpec {
    /// What people call it, such as GitHub.
    pub label: String,
    /// What it calls its CI, such as Actions or Pipelines.
    pub ci_name: String,
    /// What comes before a run's number in an address on the host.
    pub run_path: String,
    /// What comes before a job's number, where an address names one.
    #[serde(default)]
    pub job_path: Option<String>,
    /// Where the host keeps a pull request's head for fetching, before its
    /// number, when it keeps one; a fork's branch is fetched from there.
    #[serde(default)]
    pub pull_heads: Option<String>,
    /// The ways to merge, tried in order while the repository refuses one;
    /// none tries the repository's own default.
    #[serde(default)]
    pub merges: Vec<String>,
    /// The names its website goes by for everyone, a trailing `*` standing for
    /// any ending, as `gitlab.*` does for a company's own GitLab.
    #[serde(default)]
    pub web_hosts: Vec<String>,
    /// Where a new pull request from a branch is opened on the host's website.
    pub compare: Compare,
    pub pulls: PullAbilities,
    pub ci: CiAbilities,
}

/// The address after a repository's own that opens a new pull request, with
/// `{branch}` standing for the branch it comes from.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compare {
    pub path: String,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
}

/// A plugin method that agents may call as a tool. Only methods named here are
/// reachable from an agent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentTool {
    pub name: String,
    pub method: String,
    pub description: String,
    #[serde(default)]
    pub properties: Map<String, Value>,
    #[serde(default)]
    pub required: Vec<String>,
    /// How the chat pane draws a call to this tool; `pnpm agent-tools:check` validates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<Value>,
    /// Puts each call to the person first, and runs `method` only once they approve it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approve: Option<Approve>,
    /// Offered even where the plugin offers none of its other tools, such as before
    /// anything is set up in it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub always: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Approve {
    /// The method that checks a call before the person sees it and answers what they are
    /// shown; its failure goes back to the agent and nothing is asked.
    pub check: String,
}

impl Manifest {
    pub fn from_json(source: &str) -> Result<Self, PluginError> {
        let manifest: Manifest = serde_json::from_str(source)
            .map_err(|error| PluginError::new(FailureCategory::Manifest, error.to_string()))?;
        if !is_valid_id(&manifest.id) {
            return Err(PluginError::new(
                FailureCategory::Manifest,
                format!("`{}` is not a reverse-DNS plugin id", manifest.id),
            ));
        }
        if let Some(secs) = manifest.call_timeout_secs {
            if !(1..=MAX_CALL_TIMEOUT_SECS).contains(&secs) {
                return Err(PluginError::new(
                    FailureCategory::Manifest,
                    format!("callTimeoutSecs must be 1 to {MAX_CALL_TIMEOUT_SECS}, not {secs}"),
                ));
            }
        }
        if let Some(name) = manifest
            .environment
            .iter()
            .find(|name| !is_valid_variable(name))
        {
            return Err(PluginError::new(
                FailureCategory::Manifest,
                format!("`{name}` is not an environment variable name"),
            ));
        }
        for (index, tool) in manifest.tools.iter().enumerate() {
            if !is_valid_tool_name(&tool.name) {
                return Err(PluginError::new(
                    FailureCategory::Manifest,
                    format!("`{}` is not a usable agent tool name", tool.name),
                ));
            }
            if manifest
                .tools
                .iter()
                .take(index)
                .any(|earlier| earlier.name == tool.name)
            {
                return Err(PluginError::new(
                    FailureCategory::Manifest,
                    format!("agent tool `{}` is declared twice", tool.name),
                ));
            }
            if let Some(missing) = tool
                .required
                .iter()
                .find(|name| !tool.properties.contains_key(*name))
            {
                return Err(PluginError::new(
                    FailureCategory::Manifest,
                    format!(
                        "agent tool `{}` requires `{missing}`, which it never declares",
                        tool.name
                    ),
                ));
            }
        }
        Ok(manifest)
    }
}

fn is_valid_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ID_LENGTH {
        return false;
    }
    let segments: Vec<&str> = id.split('.').collect();
    segments.len() >= 2 && segments.iter().all(|segment| is_valid_segment(segment))
}

fn is_valid_variable(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Lowercase words joined by underscores, which every agent host accepts in a tool name.
fn is_valid_tool_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_TOOL_NAME_LENGTH
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_valid_segment(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_names_must_be_variables() {
        let with = |names: &str| {
            Manifest::from_json(&format!(
                r#"{{"id":"sikemux.github","name":"GitHub","environment":{names}}}"#
            ))
        };
        assert!(with(r#"["GH_TOKEN","_X1"]"#).is_ok());
        for bad in [
            r#"[""]"#,
            r#"["gh_token"]"#,
            r#"["1TOKEN"]"#,
            r#"["GH-TOKEN"]"#,
        ] {
            assert!(with(bad).is_err(), "{bad} should be turned down");
        }
    }

    #[test]
    fn rejects_ids_that_are_not_reverse_dns() {
        for id in [
            "rundeck",
            "Sikemux.rundeck",
            "sikemux..rundeck",
            "sikemux.1deck",
            "",
        ] {
            assert!(!is_valid_id(id), "{id} should be rejected");
        }
        assert!(is_valid_id("dev.someone.signoz-lite"));
    }

    #[test]
    fn reads_agent_tools_and_refuses_bad_ones() -> Result<(), PluginError> {
        let with_tools = |tools: &str| {
            Manifest::from_json(&format!(r#"{{"id":"a.b","name":"B","tools":{tools}}}"#))
        };
        let manifest = with_tools(
            r#"[{"name":"b_logs","method":"searchLogs","description":"Search logs.","properties":{"text":{"type":"string"}},"required":["text"]}]"#,
        )?;
        assert_eq!(
            manifest.tools.first().map(|tool| tool.method.as_str()),
            Some("searchLogs")
        );
        assert!(with_tools(r#"[{"name":"B-Logs","method":"x","description":"x"}]"#).is_err());
        assert!(with_tools(
            r#"[{"name":"b","method":"x","description":"x"},{"name":"b","method":"y","description":"y"}]"#
        )
        .is_err());
        assert!(
            with_tools(r#"[{"name":"b","method":"x","description":"x","required":["text"]}]"#)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn a_call_timeout_must_be_between_one_second_and_ten_minutes() -> Result<(), PluginError> {
        let with_timeout = |secs: &str| {
            Manifest::from_json(&format!(
                r#"{{"id":"a.b","name":"B","callTimeoutSecs":{secs}}}"#
            ))
        };
        assert_eq!(with_timeout("1")?.call_timeout_secs, Some(1));
        assert_eq!(with_timeout("600")?.call_timeout_secs, Some(600));
        let without = Manifest::from_json(r#"{"id":"a.b","name":"B"}"#)?;
        assert_eq!(without.call_timeout_secs, None);
        for refused in ["0", "601", "-1", "1.5", "\"60\""] {
            assert!(
                with_timeout(refused).is_err(),
                "{refused} should be refused"
            );
        }
        Ok(())
    }
}
