//! The session's permission modes, models and effort levels: what the app
//! picks from, and what Codex is told on every turn.

use serde_json::{json, Value};

/// The app's two permission modes, as the presets Codex's ACP adapter names
/// them by. "read-only" still writes inside the workspace; it asks first for
/// anything outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Ask,
    FullAccess,
}

impl Mode {
    pub fn from_app(mode: &str) -> Result<Self, String> {
        match mode {
            "workspace-write" => Ok(Self::Ask),
            "bypass" => Ok(Self::FullAccess),
            _ => Err(format!("Unsupported permission mode: {mode}")),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Ask => "read-only",
            Self::FullAccess => "agent-full-access",
        }
    }

    pub fn approval_policy(self) -> &'static str {
        match self {
            Self::Ask => "on-request",
            Self::FullAccess => "never",
        }
    }

    pub fn sandbox_policy(self) -> Value {
        match self {
            Self::Ask => json!({
                "type": "workspaceWrite",
                "writableRoots": [],
                "networkAccess": false,
                "excludeTmpdirEnvVar": false,
                "excludeSlashTmp": false,
            }),
            Self::FullAccess => json!({ "type": "dangerFullAccess" }),
        }
    }

    pub fn sandbox_name(self) -> &'static str {
        match self {
            Self::Ask => "workspace-write",
            Self::FullAccess => "danger-full-access",
        }
    }
}

/// The session's `modes`, as Codex's ACP adapter offers them.
pub(super) fn modes(current: Mode) -> Value {
    json!({
        "availableModes": [
            {
                "id": "read-only",
                "name": "Ask for approval",
                "description": "Always ask to edit external files and use the internet",
                "_meta": { "kind": "standard" },
            },
            {
                "id": "agent",
                "name": "Approve for me",
                "description": "Only ask for actions detected as potentially unsafe",
                "_meta": { "kind": "auto_review" },
            },
            {
                "id": "agent-full-access",
                "name": "Full access",
                "description": "Unrestricted access to the internet and any file on your computer",
                "_meta": { "kind": "full_access" },
            },
        ],
        "currentModeId": current.id(),
    })
}

/// The models `model/list` offered, in its order.
#[derive(Clone, Debug, Default)]
pub(super) struct Models(pub Vec<Value>);

impl Models {
    fn find(&self, model: &str) -> Option<&Value> {
        self.0.iter().find(|entry| entry["id"] == model)
    }

    pub fn offers(&self, model: &str) -> bool {
        self.find(model).is_some()
    }

    fn efforts(&self, model: &str) -> &[Value] {
        self.find(model)
            .and_then(|entry| entry["supportedReasoningEfforts"].as_array())
            .map_or(&[], Vec::as_slice)
    }

    pub fn reasons(&self, model: &str) -> bool {
        !self.efforts(model).is_empty()
    }

    pub fn supports_effort(&self, model: &str, effort: &str) -> bool {
        self.efforts(model)
            .iter()
            .any(|option| option["reasoningEffort"] == effort)
    }

    /// Whether the model reads images. A model missing from the list is given
    /// the benefit of the doubt and Codex says if it cannot.
    pub fn takes_images(&self, model: &str) -> bool {
        self.find(model)
            .and_then(|entry| entry["inputModalities"].as_array())
            .is_none_or(|modalities| modalities.iter().any(|modality| modality == "image"))
    }

    /// The effort to run `model` at: `wanted` when the model offers it,
    /// otherwise the model's own default.
    pub fn effort_for(&self, model: &str, wanted: Option<&str>) -> Option<String> {
        if !self.reasons(model) {
            return None;
        }
        if let Some(wanted) = wanted.filter(|wanted| self.supports_effort(model, wanted)) {
            return Some(wanted.to_owned());
        }
        self.find(model)
            .and_then(|entry| entry["defaultReasoningEffort"].as_str())
            .filter(|effort| self.supports_effort(model, effort))
            .or_else(|| self.efforts(model)[0]["reasoningEffort"].as_str())
            .map(str::to_owned)
    }

    /// The session's `configOptions`: the model picker, and the effort picker
    /// when the model reasons.
    pub fn options(&self, model: &str, effort: Option<&str>) -> Value {
        let mut models: Vec<Value> = self
            .0
            .iter()
            .map(|entry| {
                json!({
                    "value": entry["id"],
                    "name": entry["displayName"],
                    "description": entry["description"],
                })
            })
            .collect();
        if !self.offers(model) {
            models.insert(
                0,
                json!({ "value": model, "name": model, "description": null }),
            );
        }
        let mut options = vec![json!({
            "id": "model",
            "name": "Model",
            "description": "Model Codex uses for the session",
            "category": "model",
            "type": "select",
            "currentValue": model,
            "options": models,
        })];
        if let Some(effort) = effort.filter(|_| self.reasons(model)) {
            let efforts: Vec<Value> = self
                .efforts(model)
                .iter()
                .map(|option| {
                    let value = option["reasoningEffort"].as_str().unwrap_or_default();
                    json!({
                        "value": value,
                        "name": capitalized(value),
                        "description": option["description"],
                    })
                })
                .collect();
            options.push(json!({
                "id": "reasoning_effort",
                "name": "Reasoning effort",
                "description": "How much reasoning effort the model should use",
                "category": "thought_level",
                "type": "select",
                "currentValue": effort,
                "options": efforts,
            }));
        }
        Value::Array(options)
    }
}

fn capitalized(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// Codex's config overrides for a session: the folder is trusted, and the
/// chat's own tool servers are added unless Codex's config already has one
/// by that name.
pub(super) fn session_config(cwd: &str, servers: &[Value], configured: &[String]) -> Value {
    let mut config = json!({ "projects": { cwd: { "trust_level": "trusted" } } });
    let mut tools = serde_json::Map::new();
    for server in servers {
        let Some(name) = server["name"].as_str() else {
            continue;
        };
        let name = name.replace(char::is_whitespace, "_");
        if configured.contains(&name) {
            continue;
        }
        match tool_server(server) {
            Ok(entry) => {
                tools.insert(name, entry);
            }
            Err(error) => eprintln!("sikemux core: Codex skips tool server {name}: {error}"),
        }
    }
    if !tools.is_empty() {
        config["mcp_servers"] = Value::Object(tools);
    }
    config
}

/// The names in an ACP name and value list, as a map.
fn pairs(list: &Value) -> Value {
    Value::Object(
        list.as_array()
            .into_iter()
            .flatten()
            .filter_map(|pair| Some((pair["name"].as_str()?.to_owned(), pair["value"].clone())))
            .collect(),
    )
}

fn tool_server(server: &Value) -> Result<Value, String> {
    match server["type"].as_str() {
        Some("http") => Ok(json!({
            "url": server["url"],
            "http_headers": pairs(&server["headers"]),
        })),
        Some(kind @ ("sse" | "acp")) => Err(format!("Codex does not support {kind} tool servers")),
        _ => Ok(json!({
            "command": server["command"],
            "args": server["args"],
            "env": pairs(&server["env"]),
        })),
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
        Models(vec![
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
    fn the_app_modes_map_to_the_adapter_presets() {
        assert_eq!(Mode::from_app("bypass").unwrap().id(), "agent-full-access");
        assert_eq!(Mode::from_app("workspace-write").unwrap().id(), "read-only");
        assert!(Mode::from_app("plan").is_err());
        assert_eq!(Mode::FullAccess.approval_policy(), "never");
        assert_eq!(
            Mode::FullAccess.sandbox_policy()["type"],
            "dangerFullAccess"
        );
        assert_eq!(Mode::Ask.approval_policy(), "on-request");
        assert_eq!(Mode::Ask.sandbox_policy()["type"], "workspaceWrite");
        let ids: Vec<_> = modes(Mode::Ask)["availableModes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|mode| mode["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, ["read-only", "agent", "agent-full-access"]);
    }

    #[test]
    fn the_effort_picker_follows_the_model() {
        let models = models();
        let options = models.options("gpt-6-astra", Some("high"));
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
                .options("baseten/kimi", None)
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn an_unlisted_current_model_is_offered_first() {
        let options = models().options("gpt-old", None);
        assert_eq!(
            options[0]["options"][0],
            json!({ "value": "gpt-old", "name": "gpt-old", "description": null })
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
