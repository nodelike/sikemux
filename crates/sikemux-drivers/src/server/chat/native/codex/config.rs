//! The session's permission modes, models and effort levels: what the app
//! picks from, and what Codex is told on every turn.

use agent_client_protocol::schema::v1::McpServer;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::super::models::{self, Effort, Model, Models, Speed};
use super::super::servers;
use crate::launch::PermissionMode;

pub(super) fn approval_policy(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::WorkspaceWrite | PermissionMode::Auto => "on-request",
        PermissionMode::Bypass => "never",
    }
}

/// Who answers what Codex would ask: in Auto its own reviewer, which asks the person only
/// what it finds risky.
pub(super) fn approvals_reviewer(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Auto => "auto_review",
        PermissionMode::WorkspaceWrite | PermissionMode::Bypass => "user",
    }
}

pub(super) fn sandbox_policy(mode: PermissionMode) -> Value {
    match mode {
        PermissionMode::WorkspaceWrite | PermissionMode::Auto => json!({
            "type": "workspaceWrite",
            "writableRoots": [],
            "networkAccess": false,
            "excludeTmpdirEnvVar": false,
            "excludeSlashTmp": false,
        }),
        PermissionMode::Bypass => json!({ "type": "dangerFullAccess" }),
    }
}

pub(super) fn sandbox_name(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::WorkspaceWrite | PermissionMode::Auto => "workspace-write",
        PermissionMode::Bypass => "danger-full-access",
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    id: String,
    display_name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    supported_reasoning_efforts: Vec<ListedEffort>,
    default_reasoning_effort: Option<String>,
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    is_default: bool,
    #[serde(default)]
    service_tiers: Vec<ListedTier>,
}

#[derive(Deserialize)]
struct ListedTier {
    id: String,
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedEffort {
    reasoning_effort: String,
    description: Option<String>,
}

/// The models `model/list` offered, in its order. Codex lists none on
/// another provider, and then any model it names goes.
pub fn models(entries: Vec<Value>) -> Models {
    let listed: Vec<Model> = models::read_listing("Codex", entries)
        .into_iter()
        .map(|listed: Listed| {
            let efforts: Vec<Effort> = listed
                .supported_reasoning_efforts
                .into_iter()
                .map(|effort| Effort {
                    value: effort.reasoning_effort,
                    description: effort.description,
                })
                .collect();
            Model {
                name: listed.display_name.unwrap_or_else(|| listed.id.clone()),
                id: listed.id,
                description: listed.description,
                default_effort: models::default_effort(
                    &efforts,
                    &[listed.default_reasoning_effort.as_deref()],
                ),
                efforts,
                takes_images: listed
                    .input_modalities
                    .is_none_or(|modalities| modalities.iter().any(|modality| modality == "image")),
                default: listed.is_default,
                speeds: listed
                    .service_tiers
                    .into_iter()
                    .map(|tier| Speed {
                        value: tier.id,
                        name: tier.name,
                        description: tier.description.filter(|about| !about.is_empty()),
                    })
                    .collect(),
            }
        })
        .collect();
    Models {
        takes_unlisted: listed.is_empty(),
        listed,
    }
}

/// Codex's config overrides for a session: the folder is trusted, and the
/// chat's own tool servers are added unless Codex's config already has one
/// by that name.
pub(super) fn session_config(cwd: &str, tools: &[McpServer], configured: &[String]) -> Value {
    let mut config = json!({ "projects": { cwd: { "trust_level": "trusted" } } });
    let mut entries = Map::new();
    for server in tools {
        let (name, entry) = match server {
            McpServer::Http(http) => (
                &http.name,
                json!({ "url": http.url, "http_headers": servers::headers(&http.headers) }),
            ),
            McpServer::Stdio(stdio) => (
                &stdio.name,
                json!({
                    "command": stdio.command,
                    "args": stdio.args,
                    "env": servers::variables(&stdio.env),
                }),
            ),
            McpServer::Sse(sse) => {
                eprintln!(
                    "sikemux core: Codex skips tool server {}: it does not take SSE servers",
                    sse.name
                );
                continue;
            }
            _ => continue,
        };
        let name = servers::config_key(name);
        if !configured.contains(&name) {
            entries.insert(name, entry);
        }
    }
    if !entries.is_empty() {
        config["mcp_servers"] = Value::Object(entries);
    }
    config
}

/// The developer instructions a new thread is sent: the person's own from
/// Codex's config, which sending any replaces, then `brief`.
pub(super) fn developer_instructions(config: &Value, brief: &str) -> String {
    match config["config"]["developer_instructions"]
        .as_str()
        .map(str::trim)
        .filter(|own| !own.is_empty())
    {
        Some(own) => format!("{own}\n\n{brief}"),
        None => brief.to_owned(),
    }
}

/// The tool server names Codex's own config already has, in any layer.
pub(super) fn configured_servers(config: &Value) -> Vec<String> {
    let layers = config["layers"].as_array().into_iter().flatten();
    std::iter::once(&config["config"])
        .chain(layers.map(|layer| &layer["config"]))
        .filter_map(|config| config["mcp_servers"].as_object())
        .flat_map(|servers| servers.keys().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Models {
        super::models(vec![
            json!({
                "id": "gpt-6-astra", "displayName": "GPT-6-Astra", "description": "Most capable",
                "supportedReasoningEfforts": [
                    { "reasoningEffort": "low", "description": "Fast" },
                    { "reasoningEffort": "high", "description": "Deep" },
                ],
                "defaultReasoningEffort": "low", "inputModalities": ["text", "image"],
            }),
            json!({
                "id": "baseten/kimi", "displayName": "Kimi", "description": "Open",
                "supportedReasoningEfforts": [], "defaultReasoningEffort": "none",
                "inputModalities": ["text"],
            }),
        ])
    }

    #[test]
    fn the_effort_picker_follows_the_model() {
        let models = models();
        let options = models.options("Codex", "gpt-6-astra", Some("high"));
        assert_eq!(options[0]["id"], "model");
        assert_eq!(options[0]["currentValue"], "gpt-6-astra");
        assert_eq!(options[0]["options"][1]["value"], "baseten/kimi");
        assert_eq!(options[1]["id"], "reasoning_effort");
        assert_eq!(options[1]["category"], "thought_level");
        assert_eq!(options[1]["currentValue"], "high");
        assert_eq!(
            options[1]["options"][0],
            json!({ "value": "low", "name": "Low", "description": "Fast" })
        );
        assert_eq!(
            models
                .options("Codex", "baseten/kimi", None)
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn an_unlisted_current_model_is_offered_first() {
        let options = models().options("Codex", "gpt-old", None);
        assert_eq!(
            options[0]["options"][0],
            json!({ "value": "gpt-old", "name": "gpt-old" })
        );
    }

    #[test]
    fn an_effort_the_model_lacks_falls_back_to_its_default() {
        let models = models();
        assert_eq!(
            models.effort_for("gpt-6-astra", Some("high")).as_deref(),
            Some("high")
        );
        assert_eq!(
            models.effort_for("gpt-6-astra", Some("ultra")).as_deref(),
            Some("low")
        );
        assert_eq!(models.effort_for("baseten/kimi", Some("high")), None);
        assert!(models.takes_images("gpt-6-astra"));
        assert!(!models.takes_images("baseten/kimi"));
        assert!(models.takes_images("unlisted"));
    }

    #[test]
    fn tool_servers_become_codex_config() {
        let servers = [
            json!({
                "name": "sikemux tools", "command": "/bin/tools", "args": ["--mcp"],
                "env": [{ "name": "TOKEN", "value": "t" }],
            }),
            json!({
                "type": "http", "name": "web", "url": "https://x/mcp",
                "headers": [{ "name": "Authorization", "value": "Bearer t" }],
            }),
            json!({ "type": "http", "name": "mine", "url": "https://y", "headers": [] }),
        ];
        let servers: Vec<McpServer> = servers
            .into_iter()
            .map(|server| serde_json::from_value(server).unwrap())
            .collect();
        let config = session_config("/work", &servers, &["mine".to_owned()]);
        assert_eq!(config["projects"]["/work"]["trust_level"], "trusted");
        assert_eq!(
            config["mcp_servers"]["sikemux_tools"],
            json!({ "command": "/bin/tools", "args": ["--mcp"], "env": { "TOKEN": "t" } })
        );
        assert_eq!(
            config["mcp_servers"]["web"],
            json!({ "url": "https://x/mcp", "http_headers": { "Authorization": "Bearer t" } })
        );
        assert!(config["mcp_servers"].get("mine").is_none());
    }

    #[test]
    fn configured_servers_are_read_from_every_layer() {
        let read = json!({
            "config": { "mcp_servers": { "a": {} } },
            "layers": [{ "config": { "mcp_servers": { "b": {} } } }, { "config": {} }],
        });
        assert_eq!(configured_servers(&read), ["a", "b"]);
    }
}
