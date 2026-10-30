//! The session's permission modes, models and effort levels, and the chat's
//! own tool servers, in the shapes Muse's session protocol takes them.

use agent_client_protocol::schema::v1::McpServer;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::super::models::{self, Effort, Model, Models};
use super::super::servers;
use crate::launch::PermissionMode;

/// How Muse asks before acting under the app's permission mode. Muse's
/// sandbox is fixed for the life of the `muse serve` process, so the mode
/// also decides how it is started, by `serve_args`.
pub(super) fn approval_mode(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::WorkspaceWrite => "onRequest",
        PermissionMode::Bypass => "allowAll",
    }
}

/// `muse serve` and the flags it starts with. The folder's own rules and
/// skills load, as they do for a folder the person opened Muse in and
/// trusted. Asking keeps Muse's own sandbox, with commands inside the
/// folder.
pub(super) fn serve_args(mode: PermissionMode) -> Vec<String> {
    let sandbox: &[&str] = match mode {
        PermissionMode::WorkspaceWrite => &["--sandbox-network", "enabled"],
        PermissionMode::Bypass => &["--disable-sandbox"],
    };
    ["serve", "--trust-workspace"]
        .iter()
        .chain(sandbox)
        .map(|arg| (*arg).to_owned())
        .collect()
}

pub(super) fn sandbox_name(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::WorkspaceWrite => "on, network allowed",
        PermissionMode::Bypass => "off",
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    model_id: String,
    display_label: Option<String>,
    description: Option<String>,
    #[serde(default)]
    reasoning_effort_variants: Vec<ListedEffort>,
    default_reasoning_effort: Option<String>,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    is_default: bool,
}

#[derive(Deserialize)]
struct ListedEffort {
    tier: String,
    description: Option<String>,
}

/// The models `model/list` offered, newest first. The one the session is
/// on goes first as Muse's default, before the one Muse calls its default.
pub(super) fn models(entries: Vec<Value>) -> Models {
    let listed: Vec<Listed> = models::read_listing("Muse", entries);
    let active = listed.iter().any(|entry| entry.is_active);
    let listed = listed
        .into_iter()
        .map(|listed| {
            let efforts: Vec<Effort> = listed
                .reasoning_effort_variants
                .into_iter()
                .map(|tier| Effort {
                    value: tier.tier,
                    description: tier.description,
                })
                .collect();
            Model {
                name: listed
                    .display_label
                    .unwrap_or_else(|| listed.model_id.clone()),
                id: listed.model_id,
                description: listed.description,
                default_effort: models::default_effort(
                    &efforts,
                    &[listed.default_reasoning_effort.as_deref(), Some("high")],
                ),
                efforts,
                takes_images: true,
                default: if active {
                    listed.is_active
                } else {
                    listed.is_default
                },
            }
        })
        .collect();
    Models {
        listed,
        takes_unlisted: false,
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
