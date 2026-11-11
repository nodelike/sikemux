//! Rows for calls to other MCP servers: Jira and browsers in the words our own tools use,
//! and any other server by the verb its tool's name starts with.

use serde_json::{Map, Value};
use sikemux_view::chat::{ToolAction, ToolRow};

use super::row::{basename, first_short_argument, said};

const MAX_VERB_CHARS: usize = 12;

/// Services whose own mark the clients draw, by a word of the server's name.
const BRANDS: &[&str] = &[
    "github",
    "gitlab",
    "bitbucket",
    "linear",
    "slack",
    "vercel",
    "aws",
    "rundeck",
];

/// Atlassian's tools that only find the site or the person a later call needs.
pub(super) const JIRA_SETUP: &[&str] = &[
    "atlassianUserInfo",
    "getAccessibleAtlassianResources",
    "discover",
];

pub(super) fn row(server: &str, name: &str, args: &Map<String, Value>) -> ToolRow {
    let name = name.lines().next().unwrap_or_default().trim();
    let shown = shown_server(server);
    let mut row = if is_jira(server, name) {
        jira(name, args).unwrap_or_else(|| any(name, args, shown.as_deref(), Some("jira")))
    } else if is_browser(server) {
        browser(name, args).unwrap_or_else(|| any(name, args, shown.as_deref(), None))
    } else {
        let mark = shown.as_deref().and_then(brand);
        any(name, args, shown.as_deref(), mark)
    };
    row.server = Some(server.to_owned());
    row
}

pub(super) fn is_browser(server: &str) -> bool {
    let server = server.to_lowercase();
    server.contains("browser") || server.contains("chrome")
}

pub(super) fn is_jira(server: &str, name: &str) -> bool {
    let server = server.to_lowercase();
    server.contains("atlassian")
        || server.contains("jira")
        || name.contains("Jira")
        || (JIRA_SETUP.contains(&name) && name != "discover")
}

/// A server's name as a reader knows it: without `plugin_` or `claude_ai_` and the plugin
/// named twice, and none at all for a server known only by a generated id.
fn shown_server(server: &str) -> Option<String> {
    let generated = server.len() == 36 && server.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if generated {
        return None;
    }
    let name = server.strip_prefix("plugin_").unwrap_or(server);
    let name = name.strip_prefix("claude_ai_").unwrap_or(name);
    let name = name.strip_suffix("_MCP").unwrap_or(name);
    let name = match name.split_once('_') {
        Some((plugin, own)) if own.starts_with(plugin) => own,
        _ => name,
    };
    Some(name.to_lowercase())
}

fn brand(server: &str) -> Option<&'static str> {
    BRANDS.iter().copied().find(|brand| server.contains(brand))
}

fn text(args: &Map<String, Value>, key: &str) -> Option<String> {
    let line = match args.get(key)? {
        Value::String(text) => text.lines().next()?.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    (!line.is_empty()).then_some(line)
}

fn jira(name: &str, args: &Map<String, Value>) -> Option<ToolRow> {
    let key = || text(args, "issueIdOrKey").unwrap_or_default();
    let mark = Some("jira");
    let row = match name {
        "getJiraIssue" => said(ToolAction::Read, "read", mark, key(), None),
        "searchJiraIssuesUsingJql" => said(
            ToolAction::Search,
            "search",
            mark,
            text(args, "jql").unwrap_or_default(),
            None,
        ),
        "createJiraIssue" => said(
            ToolAction::Edit,
            "create",
            mark,
            text(args, "summary").unwrap_or_default(),
            text(args, "projectKey"),
        ),
        "editJiraIssue" => said(ToolAction::Edit, "edit", mark, key(), None),
        "transitionJiraIssue" => said(
            ToolAction::Move,
            "move",
            mark,
            key(),
            text(args, "transitionName").map(|status| format!("to {status}")),
        ),
        "addCommentToJiraIssue" | "addOrEditJiraIssueComment" => {
            said(ToolAction::Edit, "comment", mark, key(), None)
        }
        "atlassianUserInfo" => said(ToolAction::Read, "read", mark, "your account".into(), None),
        "getAccessibleAtlassianResources" => {
            said(ToolAction::Read, "read", mark, "Jira sites".into(), None)
        }
        "discover" => said(
            ToolAction::Search,
            "find",
            mark,
            text(args, "query").unwrap_or_default(),
            None,
        ),
        _ => return None,
    };
    Some(row)
}

/// A browser server's tool in the verbs and icons our own browser's rows use.
fn browser(name: &str, args: &Map<String, Value>) -> Option<ToolRow> {
    use ToolAction as A;
    let arg = |key: &str| text(args, key);
    let quoted = |key: &str| arg(key).map(|text| format!("“{text}”"));
    let tab = || arg("tabId").map(|tab| format!("tab {tab}"));
    let name = name.strip_suffix("_mcp").unwrap_or(name);
    let (action, verb, icon, target) = match name {
        "navigate" | "preview_start" => (A::Fetch, "open", "globe", arg("url").map(page)),
        "javascript_tool" => (A::Execute, "eval", "code", arg("text")),
        "computer" => return computer(args),
        "browser_batch" => {
            let steps = args.get("actions").and_then(Value::as_array).map(Vec::len);
            let target = steps.map(|steps| format!("{steps}-step sequence"));
            (A::Edit, "act", "steps", target)
        }
        "find" => (A::Search, "find", "search", quoted("query")),
        "read_page" | "get_page_text" => (A::Read, "read", "eye", Some("the page".into())),
        "form_input" => (
            A::Edit,
            "type",
            "text",
            arg("ref").map(|field| format!("into {field}")),
        ),
        "tabs_context" => (A::Read, "list", "tabs", Some("tabs".into())),
        "tabs_create" => (A::Fetch, "open", "tabs", Some("a new tab".into())),
        "tabs_close" => (A::Edit, "close", "close", tab()),
        "tabs_select" => (A::Move, "switch", "tabs", tab()),
        "resize_window" => (
            A::Edit,
            "resize",
            "resize",
            arg("width")
                .zip(arg("height"))
                .map(|(width, height)| format!("{width}×{height}")),
        ),
        "read_console_messages" => (A::Read, "read", "terminal", Some("the page console".into())),
        "read_network_requests" => (
            A::Read,
            "network",
            "network",
            quoted("urlPattern").or(Some("recent requests".into())),
        ),
        "preview_logs" => (A::Read, "read", "log-lines", Some("preview logs".into())),
        "preview_stop" => (A::Execute, "stop", "stop", Some("the preview".into())),
        "preview_list" => (A::Read, "list", "window", Some("previews".into())),
        _ => return None,
    };
    Some(said(
        action,
        verb,
        Some(icon),
        target.unwrap_or_default(),
        None,
    ))
}

/// A page by its address, or a file opened in the browser by its name.
fn page(url: String) -> String {
    match url.strip_prefix("file://") {
        Some(path) => basename(path).to_owned(),
        None => url,
    }
}

/// The browser's one tool for the mouse, keyboard and screen, by the action it was given.
fn computer(args: &Map<String, Value>) -> Option<ToolRow> {
    use ToolAction as A;
    let arg = |key: &str| text(args, key);
    let place = || {
        arg("ref").or_else(|| {
            let point = args.get("coordinate")?.as_array()?;
            let [x, y] = point.as_slice() else {
                return None;
            };
            Some(format!("{x}, {y}"))
        })
    };
    let (action, verb, icon, target) = match arg("action")?.as_str() {
        "left_click" | "right_click" | "double_click" | "triple_click" | "middle_click" => {
            (A::Edit, "click", "click", place())
        }
        "type" => (
            A::Edit,
            "type",
            "text",
            Some("into the focused field".into()),
        ),
        "key" => (A::Edit, "press", "keyboard", arg("text")),
        "screenshot" => (A::Read, "shot", "camera", Some("the page".into())),
        "zoom" => (A::Read, "zoom", "eye", place()),
        "scroll" | "scroll_to" => (A::Read, "scroll", "mouse", place()),
        "left_click_drag" => (A::Edit, "drag", "pointer", place()),
        "hover" | "mouse_move" => (A::Edit, "hover", "pointer", place()),
        "wait" => (
            A::Other,
            "wait",
            "hourglass",
            arg("duration").map(|seconds| format!("{seconds}s")),
        ),
        _ => return None,
    };
    Some(said(
        action,
        verb,
        Some(icon),
        target.unwrap_or_default(),
        None,
    ))
}

/// Any other server's tool: the first word of its name as the verb, what it was given or the
/// rest of its name as the target, and the server at the quiet end.
fn any(name: &str, args: &Map<String, Value>, server: Option<&str>, mark: Option<&str>) -> ToolRow {
    let words = words(name);
    let (first, rest) = words
        .split_first()
        .map_or(("", &[][..]), |(first, rest)| (first.as_str(), rest));
    let verb: String = match first {
        "get" | "fetch" => "read".to_owned(),
        first => first.chars().take(MAX_VERB_CHARS).collect(),
    };
    let given = args
        .iter()
        .filter(|(key, _)| key.as_str() != "cloudId")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let target = first_short_argument(&given).unwrap_or_else(|| rest.join(" "));
    let mut row = said(
        ToolAction::Mcp,
        &verb,
        mark,
        target,
        server.map(str::to_owned),
    );
    if row.target.is_empty() {
        row.activity = None;
    }
    row
}

/// A tool's name as lowercase words, split at underscores, dashes and capitals.
fn words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in name.chars() {
        if c == '_' || c == '-' || c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        if c.is_uppercase() && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}
