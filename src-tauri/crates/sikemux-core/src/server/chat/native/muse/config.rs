//! The session's permission modes, models and effort levels, and the chat's
//! own tool servers, in the shapes Muse's session protocol takes them.

use agent_client_protocol::schema::v1::McpServer;
use serde_json::{json, Map, Value};

use super::super::servers;
use crate::acp::pickers;
use crate::launch::PermissionMode;

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
    pub fn from_app(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::WorkspaceWrite => Self::Ask,
            PermissionMode::Bypass => Self::FullAccess,
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
        entry["reasoningEffortVariants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tier| {
                Some((
                    tier["tier"].as_str()?.to_owned(),
                    tier["description"].clone(),
                ))
            })
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
        let models = self
            .0
            .iter()
            .filter_map(|entry| {
                let id = entry["modelId"].as_str()?;
                let name = entry["displayLabel"].as_str().unwrap_or(id);
                Some(pickers::choice(id, name, entry["description"].as_str()))
            })
            .collect();
        let mut options = vec![pickers::model_select(
            Some("Model Muse uses for the session"),
            model,
            models,
        )];
        if let Some(effort) = effort {
            let efforts = self
                .efforts(model)
                .into_iter()
                .map(|(tier, about)| {
                    pickers::choice(&tier, &pickers::level_name(&tier), about.as_str())
                })
                .collect();
            options.push(pickers::effort_select(
                "reasoning_effort",
                "Reasoning effort",
                "How much reasoning effort the model should use",
                effort,
                efforts,
            ));
        }
        Value::Array(options)
    }
}

/// The chat's own tool servers as Muse's per-session ones. A server that
/// fails to start is left out with a warning rather than failing the chat.
pub(super) fn tool_servers(tools: &[McpServer]) -> Map<String, Value> {
    let mut entries = Map::new();
    for server in tools {
        let (name, entry) = match server {
            McpServer::Http(http) => (
                &http.name,
                json!({
                    "transport": "streamableHttp",
                    "url": http.url,
                    "headers": servers::headers(&http.headers),
                    "mode": "optional",
                }),
            ),
            McpServer::Stdio(stdio) => (
                &stdio.name,
                json!({
                    "transport": "stdio",
                    "command": stdio.command,
                    "args": stdio.args,
                    "env": servers::variables(&stdio.env),
                    "mode": "optional",
                }),
            ),
            McpServer::Sse(sse) => {
                eprintln!(
                    "sikemux core: Muse skips tool server {}: it does not take SSE servers",
                    sse.name
                );
                continue;
            }
            _ => continue,
        };
        entries.insert(servers::config_key(name), entry);
    }
    entries
}
