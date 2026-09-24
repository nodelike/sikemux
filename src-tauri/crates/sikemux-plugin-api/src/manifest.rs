use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::PluginError;

const MAX_ID_LENGTH: usize = 128;
const MAX_TOOL_NAME_LENGTH: usize = 64;
const MAX_CALL_TIMEOUT_SECS: u64 = 600;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: Version,
    pub sikemux: VersionReq,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_timeout_secs: Option<u64>,
    #[serde(default)]
    pub tools: Vec<AgentTool>,
}

/// A plugin method that agents may call as a tool. Only methods named here are
/// reachable from an agent; everything else stays behind the plugin's own UI.
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
}

impl Manifest {
    pub fn from_json(source: &str) -> Result<Self, PluginError> {
        let manifest: Manifest = serde_json::from_str(source)
            .map_err(|error| PluginError::new("manifest", error.to_string()))?;
        if !is_valid_id(&manifest.id) {
            return Err(PluginError::new(
                "manifest",
                format!("`{}` is not a reverse-DNS plugin id", manifest.id),
            ));
        }
        if let Some(secs) = manifest.call_timeout_secs {
            if !(1..=MAX_CALL_TIMEOUT_SECS).contains(&secs) {
                return Err(PluginError::new(
                    "manifest",
                    format!("callTimeoutSecs must be 1 to {MAX_CALL_TIMEOUT_SECS}, not {secs}"),
                ));
            }
        }
        for (index, tool) in manifest.tools.iter().enumerate() {
            if !is_valid_tool_name(&tool.name) {
                return Err(PluginError::new(
                    "manifest",
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
                    "manifest",
                    format!("agent tool `{}` is declared twice", tool.name),
                ));
            }
            if let Some(missing) = tool
                .required
                .iter()
                .find(|name| !tool.properties.contains_key(*name))
            {
                return Err(PluginError::new(
                    "manifest",
                    format!(
                        "agent tool `{}` requires `{missing}`, which it never declares",
                        tool.name
                    ),
                ));
            }
        }
        Ok(manifest)
    }

    /// Nightly builds carry a pre-release tag, which a plain range like `>=0.4`
    /// would never match. Plugins target the release line, so compare against
    /// the version with the tag removed.
    pub fn supports(&self, sikemux: &Version) -> bool {
        let release_line = Version::new(sikemux.major, sikemux.minor, sikemux.patch);
        self.sikemux.matches(&release_line)
    }
}

pub fn is_valid_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ID_LENGTH {
        return false;
    }
    let segments: Vec<&str> = id.split('.').collect();
    segments.len() >= 2 && segments.iter().all(|segment| is_valid_segment(segment))
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

    fn manifest(sikemux: &str) -> Result<Manifest, PluginError> {
        Manifest::from_json(&format!(
            r#"{{"id":"sikemux.rundeck","name":"Rundeck","version":"1.0.0","sikemux":"{sikemux}"}}"#
        ))
    }

    #[test]
    fn parses_a_manifest() -> Result<(), PluginError> {
        let manifest = manifest(">=0.4")?;
        assert_eq!(manifest.id, "sikemux.rundeck");
        Ok(())
    }

    #[test]
    fn a_nightly_satisfies_its_release_line() -> Result<(), PluginError> {
        let manifest = manifest(">=0.4")?;
        let nightly = Version::parse("0.4.0-nightly.10")
            .map_err(|e| PluginError::new("test", e.to_string()))?;
        let older = Version::parse("0.3.9").map_err(|e| PluginError::new("test", e.to_string()))?;
        assert!(manifest.supports(&nightly));
        assert!(!manifest.supports(&older));
        Ok(())
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
            Manifest::from_json(&format!(
                r#"{{"id":"a.b","name":"B","version":"1.0.0","sikemux":"*","tools":{tools}}}"#
            ))
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
                r#"{{"id":"a.b","name":"B","version":"1.0.0","sikemux":"*","callTimeoutSecs":{secs}}}"#
            ))
        };
        assert_eq!(with_timeout("1")?.call_timeout_secs, Some(1));
        assert_eq!(with_timeout("600")?.call_timeout_secs, Some(600));
        assert_eq!(manifest("*")?.call_timeout_secs, None);
        for refused in ["0", "601", "-1", "1.5", "\"60\""] {
            assert!(
                with_timeout(refused).is_err(),
                "{refused} should be refused"
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_unknown_fields() {
        let result = Manifest::from_json(
            r#"{"id":"a.b","name":"B","version":"1.0.0","sikemux":"*","extra":1}"#,
        );
        assert!(result.is_err());
    }
}
