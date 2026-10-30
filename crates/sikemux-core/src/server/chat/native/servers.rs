//! The tool servers a chat hands its agent, read once from the launch.

use agent_client_protocol::schema::v1::{EnvVariable, HttpHeader, McpServer};
use serde_json::{Map, Value};

use crate::launch::tools::TOOLS_SERVER;
use crate::protocol::ChatLaunch;

/// Whether Sikemux's own tools are among `servers`.
pub(super) fn offers_sikemux_tools(servers: &[McpServer]) -> bool {
    servers.iter().any(|server| {
        let name = match server {
            McpServer::Http(http) => &http.name,
            McpServer::Stdio(stdio) => &stdio.name,
            McpServer::Sse(sse) => &sse.name,
            _ => return false,
        };
        name == TOOLS_SERVER
    })
}

/// The launch's tool servers, skipping any that cannot be read.
pub(super) fn tool_servers(launch: &ChatLaunch) -> Vec<McpServer> {
    launch
        .mcp_servers
        .iter()
        .filter_map(|server| match serde_json::from_value(server.clone()) {
            Ok(server) => Some(server),
            Err(error) => {
                eprintln!(
                    "sikemux core: chat {} skips a tool server it cannot read: {error}",
                    launch.agent_id
                );
                None
            }
        })
        .collect()
}

/// A server's name as a config key, which takes no spaces.
pub(super) fn config_key(name: &str) -> String {
    name.replace(char::is_whitespace, "_")
}

pub(super) fn variables(env: &[EnvVariable]) -> Map<String, Value> {
    env.iter()
        .map(|variable| (variable.name.clone(), Value::from(variable.value.as_str())))
        .collect()
}

pub(super) fn headers(headers: &[HttpHeader]) -> Map<String, Value> {
    headers
        .iter()
        .map(|header| (header.name.clone(), Value::from(header.value.as_str())))
        .collect()
}
