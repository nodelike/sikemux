//! Agents that call a tool server's tool under a name of their own: OMP writes
//! the arguments as JSON to the path `xd://mcp__<server>_<tool>` with its own
//! `write` tool, and OpenCode names the call `<server>_<tool>`. Each is read as
//! the tool call it is, titled `mcp__<server>__<tool>` as Claude's and Codex's
//! are, so the chat shows it, and a page it shows, alike.

use serde_json::{json, Value};
use sikemux_fold::UpdateKind;

use crate::launch::tools::TOOLS_SERVER;

const TOOL_PATH: &str = "xd://mcp__";

/// Makes a `tool_call` or `tool_call_update` of a tool server's tool, under
/// its agent's own name for it, into that tool's call. `servers` are the tool
/// servers the chat handed its agent.
pub(super) fn as_mcp_call(update: &mut Value, servers: &[String]) {
    if !UpdateKind::of(update).is_some_and(UpdateKind::is_tool) {
        return;
    }
    if !omp_write(update, servers) {
        opencode_call(update, servers);
    }
}

/// OMP's write to a tool's path, read from the path it writes or the device its result names.
fn omp_write(update: &mut Value, servers: &[String]) -> bool {
    let device = update.pointer("/rawOutput/details/xdev").cloned();
    let inner = device.as_ref().map(|device| &device["inner"]);
    let named = |pointer: &str, prefix: &str| {
        update
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(|path| path.strip_prefix(prefix))
            .map(str::to_owned)
    };
    let written = update["rawInput"]
        .as_object()
        .and_then(|input| {
            input
                .values()
                .filter_map(Value::as_str)
                .find_map(|value| value.strip_prefix(TOOL_PATH))
        })
        .map(str::to_owned);
    let (server, tool) = match inner.map(|inner| (&inner["serverName"], &inner["mcpToolName"])) {
        Some((Value::String(server), Value::String(tool))) => (server.clone(), tool.clone()),
        _ => match written
            .clone()
            .or_else(|| named("/rawOutput/details/xdev/tool", "mcp__"))
        {
            Some(name) => split(&name, servers).unwrap_or_else(|| match name.split_once('_') {
                Some((server, tool)) => (server.to_owned(), tool.to_owned()),
                None => (String::new(), name),
            }),
            None => return false,
        },
    };
    update["title"] = json!(format!("mcp__{server}__{tool}"));
    update["kind"] = json!("other");
    if written.is_some() {
        let content = update["rawInput"]["content"].as_str().unwrap_or_default();
        update["rawInput"] = serde_json::from_str::<Value>(content)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({ "input": content }));
    } else if let Some(args) = device.as_ref().map(|device| &device["args"]) {
        if args.is_object() {
            update["rawInput"] = args.clone();
        }
    }
    if let Some(blocks) = update.pointer("/rawOutput/content").cloned() {
        update["rawOutput"] = blocks;
    }
    true
}

/// OpenCode's call titled `<server>_<tool>`, which it sends as a tool of no particular kind.
fn opencode_call(update: &mut Value, servers: &[String]) {
    if update["kind"].as_str().is_some_and(|kind| kind != "other") {
        return;
    }
    let Some((server, tool)) = update["title"]
        .as_str()
        .and_then(|title| split(title, servers))
    else {
        return;
    };
    update["title"] = json!(format!("mcp__{server}__{tool}"));
}

/// `sikemux_tools_page_show` or `sikemux-tools_page_show` as the server
/// `sikemux-tools` and its tool `page_show`. A server's name is joined to its
/// tool's with an underscore, OMP making its dashes underscores too, so only the
/// servers' own names tell where one ends.
fn split(name: &str, servers: &[String]) -> Option<(String, String)> {
    servers
        .iter()
        .map(String::as_str)
        .chain([TOOLS_SERVER])
        .filter_map(|server| {
            let joined: String = server
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            let tool = [server, joined.as_str()]
                .into_iter()
                .find_map(|prefix| name.strip_prefix(prefix)?.strip_prefix('_'))?;
            (!tool.is_empty()).then_some((server, tool))
        })
        .max_by_key(|(server, _)| server.len())
        .map(|(server, tool)| (server.to_owned(), tool.to_owned()))
}
