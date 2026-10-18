//! The session's permission modes, models and effort levels, and the chat's
//! own tool servers, in the shapes Muse's session protocol takes them.

use serde_json::{json, Map, Value};

/// The app's two permission modes. Muse's sandbox is fixed for the life of
/// the `muse serve` process, so the mode also decides how it is started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum Mode {
    /// Muse's own default: the sandbox keeps commands inside the folder, and
    /// Muse asks before going past it.
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

    pub fn approval_mode(self) -> &'static str {
        match self {
            Self::Ask => "onRequest",
            Self::FullAccess => "allowAll",
        }
    }

    /// `muse serve` and the flags it starts with. The folder's own rules and
    /// skills load, as they do for a folder the person opened Muse in and
    /// trusted.
    pub fn serve_args(self) -> Vec<String> {
        let sandbox: &[&str] = match self {
            Self::Ask => &["--sandbox-network", "enabled"],
            Self::FullAccess => &["--disable-sandbox"],
        };
        ["serve", "--trust-workspace"]
            .iter()
            .chain(sandbox)
            .map(|arg| (*arg).to_owned())
            .collect()
    }

    pub fn sandbox_name(self) -> &'static str {
        match self {
            Self::Ask => "on, network allowed",
            Self::FullAccess => "off",
        }
    }
}

/// The models `model/list` offered, newest first.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Models(pub Vec<Value>);

impl Models {
    fn find(&self, model: &str) -> Option<&Value> {
        self.0.iter().find(|entry| entry["modelId"] == model)
    }

    pub fn offers(&self, model: &str) -> bool {
        self.find(model).is_some()
    }

    pub fn default_model(&self) -> Option<String> {
        self.0
            .iter()
            .find(|entry| entry["isActive"] == true)
            .or_else(|| self.0.iter().find(|entry| entry["isDefault"] == true))
            .or_else(|| self.0.first())
            .and_then(|entry| entry["modelId"].as_str())
            .map(str::to_owned)
    }

    /// The model's effort tiers with what each says of itself.
    fn efforts(&self, model: &str) -> Vec<(String, Value)> {
        let Some(entry) = self.find(model) else {
            return Vec::new();
        };
        if let Some(described) = entry["reasoningEffortVariants"].as_array() {
            return described
                .iter()
                .filter_map(|tier| {
                    Some((
                        tier["tier"].as_str()?.to_owned(),
                        tier["description"].clone(),
                    ))
                })
                .collect();
        }
        entry["variants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tier| Some((tier.as_str()?.to_owned(), Value::Null)))
            .collect()
    }

    pub fn supports_effort(&self, model: &str, effort: &str) -> bool {
        self.efforts(model).iter().any(|(tier, _)| tier == effort)
    }

    /// The effort to run `model` at: `wanted` when the model offers it,
    /// otherwise the model's own default.
    pub fn effort_for(&self, model: &str, wanted: Option<&str>) -> Option<String> {
        let efforts = self.efforts(model);
        if efforts.is_empty() {
            return None;
        }
        if let Some(wanted) = wanted.filter(|wanted| self.supports_effort(model, wanted)) {
            return Some(wanted.to_owned());
        }
        self.find(model)
            .and_then(|entry| entry["defaultReasoningEffort"].as_str())
            .filter(|effort| self.supports_effort(model, effort))
            .map(str::to_owned)
            .or_else(|| {
                efforts
                    .iter()
                    .find(|(tier, _)| tier == "high")
                    .or(efforts.first())
                    .map(|(tier, _)| tier.clone())
            })
    }

    /// The session's `configOptions`: the model picker, and the effort picker
    /// when the model has tiers.
    pub fn options(&self, model: &str, effort: Option<&str>) -> Value {
        let mut models: Vec<Value> = self
            .0
            .iter()
            .map(|entry| {
                json!({
                    "value": entry["modelId"],
                    "name": entry["displayLabel"],
                    "description": entry["description"],
                })
            })
            .collect();
        if !self.offers(model) && !model.is_empty() {
            models.insert(
                0,
                json!({ "value": model, "name": model, "description": null }),
            );
        }
        let mut options = vec![json!({
            "id": "model",
            "name": "Model",
            "description": "Model Muse uses for the session",
            "category": "model",
            "type": "select",
            "currentValue": model,
            "options": models,
        })];
        if let Some(effort) = effort {
            let efforts: Vec<Value> = self
                .efforts(model)
                .into_iter()
                .map(|(tier, description)| {
                    json!({ "value": tier, "name": capitalized(&tier), "description": description })
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

/// The names in an ACP name and value list, as a map.
fn pairs(list: &Value) -> Map<String, Value> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| Some((pair["name"].as_str()?.to_owned(), pair["value"].clone())))
        .collect()
}

/// The chat's own tool servers as Muse's per-session ones. A server that
/// fails to start is left out with a warning rather than failing the chat.
pub(super) fn tool_servers(servers: &[Value]) -> Map<String, Value> {
    let mut tools = Map::new();
    for server in servers {
        let Some(name) = server["name"].as_str() else {
            continue;
        };
        let name = name.replace(char::is_whitespace, "_");
        let entry = match server["type"].as_str() {
            Some("http") => json!({
                "transport": "streamableHttp",
                "url": server["url"],
                "headers": pairs(&server["headers"]),
                "mode": "optional",
            }),
            Some(kind @ ("sse" | "acp")) => {
                eprintln!(
                    "sikemux core: Muse skips tool server {name}: it does not take {kind} servers"
                );
                continue;
            }
            _ => json!({
                "transport": "stdio",
                "command": server["command"],
                "args": server["args"],
                "env": pairs(&server["env"]),
                "mode": "optional",
            }),
        };
        tools.insert(name, entry);
    }
    tools
}
