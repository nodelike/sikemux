use std::collections::BTreeMap;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use sikemux_wire::protocol::{CiAbilities, PullAbilities};

use crate::{Category, PluginError};

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
    /// Variables the plugin reads from the person's shell. An app opened from
    /// the Dock starts without them, so the host copies them in from the login shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    /// The reads a pane shows, by method, which the core keeps and reads again
    /// while a pane shows one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub views: BTreeMap<String, View>,
    /// The methods that change what the views show, so the core reads them
    /// again after one succeeds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    /// The methods that sign the person in or out or pick another account, after
    /// which clients read the plugins again and its views are read afresh.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub account_methods: Vec<String>,
    /// What the core and clients know of the code host the plugin speaks for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_host: Option<CodeHostSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_form: Option<TokenForm>,
}

/// The form a person signs in to the plugin's service with by pasting a token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenForm {
    /// What the form calls with what was typed, under each field's name.
    pub method: String,
    /// What signing in brings, said above the form.
    pub lede: String,
    pub fields: Vec<FormField>,
    /// Where a token is made, `{server}` standing for the server typed in the
    /// `host` field, or `default_server` while it is empty.
    #[serde(default)]
    pub token_page: Option<String>,
    #[serde(default)]
    pub default_server: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub placeholder: String,
    #[serde(default)]
    pub secret: bool,
}

impl TokenForm {
    /// Where a token is made for the server typed in, if the service says.
    pub fn token_page(&self, server: &str) -> Option<String> {
        let server = server.trim().trim_start_matches("https://");
        let server = server.split('/').next().unwrap_or_default();
        let server = match server {
            "" => self.default_server.as_deref().unwrap_or_default(),
            typed => typed,
        };
        Some(self.token_page.as_ref()?.replace("{server}", server))
    }
}

/// A plugin as clients list it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Whether its sign-in works; none for a plugin with no sign-in, or one
    /// switched off.
    pub signed_in: Option<bool>,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct View {
    /// How often it is read again while shown, in seconds.
    pub every_secs: u64,
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
            .map_err(|error| PluginError::new(Category::Manifest, error.to_string()))?;
        if !is_valid_id(&manifest.id) {
            return Err(PluginError::new(
                Category::Manifest,
                format!("`{}` is not a reverse-DNS plugin id", manifest.id),
            ));
        }
        if let Some(secs) = manifest.call_timeout_secs {
            if !(1..=MAX_CALL_TIMEOUT_SECS).contains(&secs) {
                return Err(PluginError::new(
                    Category::Manifest,
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
                Category::Manifest,
                format!("`{name}` is not an environment variable name"),
            ));
        }
        for (index, tool) in manifest.tools.iter().enumerate() {
            if !is_valid_tool_name(&tool.name) {
                return Err(PluginError::new(
                    Category::Manifest,
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
                    Category::Manifest,
                    format!("agent tool `{}` is declared twice", tool.name),
                ));
            }
            if let Some(missing) = tool
                .required
                .iter()
                .find(|name| !tool.properties.contains_key(*name))
            {
                return Err(PluginError::new(
                    Category::Manifest,
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

    fn manifest(sikemux: &str) -> Result<Manifest, PluginError> {
        Manifest::from_json(&format!(
            r#"{{"id":"sikemux.rundeck","name":"Rundeck","version":"1.0.0","sikemux":"{sikemux}"}}"#
        ))
    }

    #[test]
    fn a_nightly_satisfies_its_release_line() -> Result<(), PluginError> {
        let manifest = manifest(">=0.4")?;
        let nightly = Version::parse("0.4.0-nightly.10")
            .map_err(|e| PluginError::new(Category::own("test"), e.to_string()))?;
        let older = Version::parse("0.3.9")
            .map_err(|e| PluginError::new(Category::own("test"), e.to_string()))?;
        assert!(manifest.supports(&nightly));
        assert!(!manifest.supports(&older));
        Ok(())
    }

    #[test]
    fn environment_names_must_be_variables() {
        let with = |names: &str| {
            Manifest::from_json(&format!(
                r#"{{"id":"sikemux.github","name":"GitHub","version":"1.0.0","sikemux":">=0.4","environment":{names}}}"#
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
}
