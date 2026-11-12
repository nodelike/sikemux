//! MCP Apps through Claude Code. When a call to an MCP tool that names a `ui://` page ends, the
//! page is read through Claude Code's control channel and kept before the call's end is told,
//! and what the page later asks of its server goes back through the same channel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use super::control::Control;
use crate::server::mcp_apps::{Record, ServerRequest};

/// What Claude Code must say it can do at the start of a turn: give tools their `_meta.ui`, read
/// a server's resources, and run a tool for an app without the model seeing the result.
const NEEDS: [&str; 3] = [
    "mcp_tool_ui_meta_v1",
    "mcp_read_resource_v1",
    "mcp_call_result_not_for_model_v1",
];
/// A slow server's page is given up on after this, and its call shows as any other.
pub(super) const READ_WAIT: Duration = Duration::from_secs(10);

/// The servers Claude Code is connected to, as its `mcp_status` lists them, and which server and
/// tool each MCP call in flight went to.
#[derive(Clone, Default)]
pub(crate) struct Apps(Arc<Mutex<Known>>);

#[derive(Default)]
struct Known {
    supported: bool,
    servers: Option<Vec<Value>>,
    calls: HashMap<String, (String, String)>,
}

/// What a finished MCP call shows.
pub(super) enum Shows {
    Plain,
    /// The tool names a page, or the servers are not known yet and may say it does.
    MaybeApp {
        server: String,
        tool: String,
    },
}

impl Apps {
    pub fn started_turn(&self, capabilities: &[String]) {
        self.0.locked().supported = NEEDS
            .iter()
            .all(|need| capabilities.iter().any(|said| said == need));
    }

    /// Notes the server and tool Claude Code said a call `id` went to.
    pub fn called(&self, id: &str, server: &str, tool: &str) {
        let mut known = self.0.locked();
        if known.supported {
            known
                .calls
                .insert(id.to_owned(), (server.to_owned(), tool.to_owned()));
        }
    }

    pub fn ended(&self, id: &str) -> Shows {
        let mut known = self.0.locked();
        let Some((server, tool)) = known.calls.remove(id) else {
            return Shows::Plain;
        };
        match &known.servers {
            Some(servers) if page_of(servers, &server, &tool).is_none() => Shows::Plain,
            _ => Shows::MaybeApp { server, tool },
        }
    }

    async fn servers(&self, control: &Control, fresh: bool) -> Result<Vec<Value>, String> {
        if !fresh {
            if let Some(servers) = self.0.locked().servers.clone() {
                return Ok(servers);
            }
        }
        let status = control.request(json!({ "subtype": "mcp_status" })).await?;
        let servers = status
            .get("mcpServers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.0.locked().servers = Some(servers.clone());
        Ok(servers)
    }

    /// Reads the page a finished call names, if its tool names one.
    pub async fn read(
        &self,
        control: &Control,
        server: &str,
        tool: &str,
        input: Value,
        result: Value,
    ) -> Result<Option<Record>, String> {
        let known = self.0.locked().servers.is_some();
        let mut servers = self.servers(control, false).await?;
        if known && page_of(&servers, server, tool).is_none() {
            servers = self.servers(control, true).await?;
        }
        let Some((uri, described)) = page_of(&servers, server, tool) else {
            return Ok(None);
        };
        let read = control
            .request(json!({ "subtype": "mcp_read_resource", "serverName": server, "uri": uri }))
            .await?;
        let content = read
            .get("contents")
            .and_then(Value::as_array)
            .and_then(|contents| contents.first())
            .ok_or("the server sent no page")?;
        let html = match content.get("text").and_then(Value::as_str) {
            Some(text) => text.to_owned(),
            None => {
                use base64::Engine as _;
                let blob = content
                    .get("blob")
                    .and_then(Value::as_str)
                    .ok_or("the server's page holds no HTML")?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(blob)
                    .map_err(|error| error.to_string())?;
                String::from_utf8(bytes).map_err(|error| error.to_string())?
            }
        };
        let ui = content.pointer("/_meta/ui");
        Ok(Some(Record {
            server: server.to_owned(),
            tool: described,
            uri,
            html,
            csp: ui.and_then(|ui| ui.get("csp")).cloned(),
            prefers_border: ui
                .and_then(|ui| ui.get("prefersBorder"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            input,
            result,
        }))
    }

    /// Passes on what an app asked of its server: a call to one of the server's tools that is
    /// open to apps, or a read of one of its resources.
    pub async fn serve(&self, control: &Control, request: ServerRequest) -> Result<Value, String> {
        let servers = self.servers(control, false).await?;
        if !servers.iter().any(|status| name(status) == request.server) {
            return Err(format!("{} is not connected", request.server));
        }
        if overlaps(&servers, &request.server) {
            return Err("Another MCP server's tool names overlap this app's server".into());
        }
        match request.method.as_str() {
            "tools/call" => {
                let tool = request
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("the call names no tool")?;
                let described = tools_of(&servers, &request.server)
                    .find(|described| name(described) == tool)
                    .ok_or_else(|| format!("{} has no tool {tool}", request.server))?;
                let visibility = described.pointer("/_meta/ui/visibility");
                if visibility
                    .and_then(Value::as_array)
                    .is_some_and(|seen_by| !seen_by.iter().any(|by| by.as_str() == Some("app")))
                {
                    return Err(format!("{tool} is not open to apps"));
                }
                let arguments = request
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                control
                    .request(json!({
                        "subtype": "mcp_call",
                        "tool": tool_name(&request.server, tool),
                        "arguments": arguments,
                        "result_not_for_model": true,
                        "check_permissions": true,
                    }))
                    .await
            }
            "resources/read" => {
                let uri = request
                    .params
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or("the read names no resource")?;
                control
                    .request(json!({
                        "subtype": "mcp_read_resource",
                        "serverName": request.server,
                        "uri": uri,
                    }))
                    .await
            }
            other => Err(format!("an app cannot ask for {other}")),
        }
    }
}

fn name(value: &Value) -> &str {
    value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn tools_of<'a>(servers: &'a [Value], server: &'a str) -> impl Iterator<Item = &'a Value> {
    servers
        .iter()
        .filter(move |status| name(status) == server)
        .filter_map(|status| status.get("tools").and_then(Value::as_array))
        .flatten()
}

/// The page a tool names, and the tool as its server described it.
fn page_of(servers: &[Value], server: &str, tool: &str) -> Option<(String, Value)> {
    let described = tools_of(servers, server).find(|described| name(described) == tool)?;
    let meta = described.get("_meta")?;
    let uri = meta
        .pointer("/ui/resourceUri")
        .or_else(|| meta.get("ui/resourceUri"))
        .and_then(Value::as_str)
        .filter(|uri| uri.starts_with("ui://"))?;
    Some((uri.to_owned(), described.clone()))
}

fn name_part(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The `mcp__<server>__` Claude Code puts before a server's tools; it also squeezes and trims
/// the `_` in the names of claude.ai's connectors.
fn prefix(server: &str) -> String {
    let part = name_part(server);
    let part = if server.starts_with("claude.ai ") {
        part.split('_')
            .filter(|piece| !piece.is_empty())
            .collect::<Vec<_>>()
            .join("_")
    } else {
        part
    };
    format!("mcp__{part}__")
}

fn tool_name(server: &str, tool: &str) -> String {
    format!("{}{}", prefix(server), name_part(tool))
}

/// Claude Code sends `mcp__a__b__tool` to the server `a` even with `a__b` connected, so a call
/// meant for one could reach the other.
fn overlaps(servers: &[Value], server: &str) -> bool {
    let own = prefix(server);
    servers
        .iter()
        .map(name)
        .filter(|other| *other != server)
        .map(prefix)
        .any(|other| other.starts_with(&own) || own.starts_with(&other))
}

/// The result a model turn's call had, as MCP's `CallToolResult`: Claude Code flattens its text
/// to a string, which goes back into a text block.
pub(super) fn call_result(reported: Option<&Value>, block: &Value) -> Value {
    let source = reported.unwrap_or(block);
    let content = match source.get("content") {
        Some(Value::String(text)) => json!([{ "type": "text", "text": text }]),
        Some(Value::Array(blocks)) => Value::Array(blocks.clone()),
        _ => json!([]),
    };
    let mut result = json!({ "content": content });
    for key in ["structuredContent", "_meta"] {
        if let Some(value) = source.get(key).filter(|value| !value.is_null()) {
            result[key] = value.clone();
        }
    }
    result
}
