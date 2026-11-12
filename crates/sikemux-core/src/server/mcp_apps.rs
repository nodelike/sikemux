//! MCP Apps: a tool an agent called whose MCP server gave it a `ui://` page. The agent's driver
//! reads that page as the call ends and keeps it here by the call's id, so the chat draws the
//! page in place of the call, also when the chat is read again later. The page talks only to
//! its own server, through the agent that called it.

use crate::server::parts::Parts;
pub(crate) use sikemux_drivers::server::mcp_apps::{Record, ServerRequest};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use super::{Core, CoreError, CoreResult};
use sikemux_fold::UpdateKind;

const FOLDER: &str = "mcp-apps";
/// Where an update says which kept app its call shows.
pub(crate) const MARK: &str = "sikemuxMcpApp";
const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_ID_CHARS: usize = 128;

/// An app as a client draws it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpApp {
    pub server: String,
    pub tool: Value,
    /// The page, with the policy that keeps it to the domains it named.
    pub html: String,
    pub prefers_border: bool,
    pub input: Value,
    pub result: Value,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppParams {
    pub agent_id: String,
    pub id: String,
}

/// A request the page made of its server.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppRequestParams {
    pub agent_id: String,
    pub id: String,
    /// `tools/call` or `resources/read`.
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

pub(crate) struct McpApps {
    dir: Option<PathBuf>,
    /// The ids kept, read from the folder the first time one is asked for.
    kept: Mutex<Option<HashSet<String>>>,
}

pub(crate) fn is_app_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_CHARS
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

impl McpApps {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            dir: data_dir.map(|dir| dir.join(FOLDER)),
            kept: Mutex::new(None),
        }
    }

    fn file(&self, id: &str) -> Option<PathBuf> {
        is_app_id(id)
            .then(|| self.dir.as_ref().map(|dir| dir.join(format!("{id}.json"))))
            .flatten()
    }

    fn has(&self, id: &str) -> bool {
        let mut kept = self.kept.locked();
        let kept = kept.get_or_insert_with(|| {
            let Some(dir) = &self.dir else {
                return HashSet::new();
            };
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    entry
                        .file_name()
                        .to_str()?
                        .strip_suffix(".json")
                        .map(str::to_owned)
                })
                .collect()
        });
        kept.contains(id)
    }

    /// Keeps the app the call `id` showed, before the call's end is told.
    pub(crate) fn keep(&self, id: &str, record: &Record) -> Result<(), String> {
        if record.html.len() > MAX_HTML_BYTES {
            return Err("its page is bigger than 4 MiB".into());
        }
        let file = self.file(id).ok_or("this core keeps no apps")?;
        let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        }
        let temporary = file.with_extension("json.part");
        std::fs::write(&temporary, bytes)
            .and_then(|()| std::fs::rename(&temporary, &file))
            .map_err(|error| error.to_string())?;
        self.has(id);
        if let Some(kept) = self.kept.locked().as_mut() {
            kept.insert(id.to_owned());
        }
        Ok(())
    }

    pub(crate) fn read(&self, id: &str) -> CoreResult<Record> {
        let file = self.file(id).ok_or("that is not an app")?;
        let bytes = std::fs::read(file).map_err(|_| CoreError::from("that app is gone"))?;
        serde_json::from_slice(&bytes).map_err(|error| CoreError::from(error.to_string()))
    }

    /// Names the kept app on the update that ends its call.
    pub(crate) fn mark(&self, update: &mut Value) {
        if UpdateKind::of(update) != Some(UpdateKind::ToolCallUpdate)
            || update.get("status").and_then(Value::as_str) != Some("completed")
        {
            return;
        }
        let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        if !self.has(id) {
            return;
        }
        let id = id.to_owned();
        let Some(fields) = update.as_object_mut() else {
            return;
        };
        let meta = fields.entry("_meta").or_insert_with(|| json!({}));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(MARK.into(), json!({ "id": id }));
        }
    }
}

/// Keeps the page to the domains it declared and nothing else, the app's own host included.
fn policy(csp: Option<&Value>) -> String {
    let domains = |key: &str| -> Vec<String> {
        csp.and_then(|csp| csp.get(key))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|domain| is_domain(domain))
            .map(str::to_owned)
            .collect()
    };
    let or_none = |list: Vec<String>| {
        if list.is_empty() {
            "'none'".to_owned()
        } else {
            list.join(" ")
        }
    };
    let resources = domains("resourceDomains").join(" ");
    let with = |base: &str| format!("{base} {resources}").trim().to_owned();
    [
        "default-src 'none'".to_owned(),
        with("script-src 'unsafe-inline'"),
        with("style-src 'unsafe-inline'"),
        with("img-src data: blob:"),
        with("font-src data:"),
        with("media-src data: blob:"),
        format!("connect-src {}", or_none(domains("connectDomains"))),
        format!("frame-src {}", or_none(domains("frameDomains"))),
        "object-src 'none'".to_owned(),
        format!("base-uri {}", or_none(domains("baseUriDomains"))),
        "form-action 'none'".to_owned(),
    ]
    .join("; ")
}

/// An origin such as `https://cdn.example.com` or `https://*.example.com`, with nothing that
/// could close the policy's directive.
fn is_domain(domain: &str) -> bool {
    let Some(host) = domain
        .strip_prefix("https://")
        .or_else(|| domain.strip_prefix("wss://"))
    else {
        return false;
    };
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-*:/_".contains(&byte))
}

fn escape_attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

pub(crate) async fn read(core: &Core, params: McpAppParams) -> CoreResult<McpApp> {
    let core_held = core.arc().ok_or("the core is stopping")?;
    let record = tokio::task::spawn_blocking(move || core_held.mcp_apps().read(&params.id))
        .await
        .map_err(|error| CoreError::from(error.to_string()))??;
    let meta = format!(
        r#"<meta http-equiv="Content-Security-Policy" content="{}">"#,
        escape_attribute(&policy(record.csp.as_ref()))
    );
    Ok(McpApp {
        server: record.server,
        tool: record.tool,
        html: crate::pages::insert_head(&record.html, &meta),
        prefers_border: record.prefers_border,
        input: record.input,
        result: record.result,
    })
}

/// Passes a request the page made on to its server, through the agent whose call it came from.
pub(crate) async fn request(core: &Core, params: McpAppRequestParams) -> CoreResult<Value> {
    if !matches!(params.method.as_str(), "tools/call" | "resources/read") {
        return Err(format!("an app cannot ask for {}", params.method).into());
    }
    let core_held = core.arc().ok_or("the core is stopping")?;
    let id = params.id.clone();
    let record = tokio::task::spawn_blocking(move || core_held.mcp_apps().read(&id))
        .await
        .map_err(|error| CoreError::from(error.to_string()))??;
    let request = ServerRequest {
        server: record.server,
        method: params.method,
        params: params.params,
    };
    super::chat::mcp_app_request(core, &params.agent_id, request).await
}
