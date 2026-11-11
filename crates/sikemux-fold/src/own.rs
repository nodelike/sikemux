//! Rows for the tools a coding agent brings itself, such as Claude Code's subagents, web
//! searches and wakeups, and for shell commands, each in the verb of what it did rather
//! than the first word of its title.

use serde_json::{Map, Value};
use sikemux_view::chat::{ToolAction, ToolCall, ToolRow};

use super::kinds;
use super::row::{activity, basename, duration};

pub(super) fn reword(tool: &ToolCall, args: &Map<String, Value>, row: &mut ToolRow) {
    if row.server.is_some() {
        return;
    }
    if row.action == ToolAction::Execute {
        if let Some(said) = kinds::typed_command(tool).as_deref().and_then(kinds::said) {
            row.verb = Some(said.verb.to_owned());
            row.icon = Some(said.icon.to_owned());
            row.target = said.target;
            row.detail = said.detail;
            row.activity = Some(activity(said.verb, &row.target));
        }
        return;
    }
    let Some(name) = tool.name.as_deref() else {
        return;
    };
    let text = |key: &str| {
        let line = args.get(key)?.as_str()?.lines().next()?.trim();
        (!line.is_empty()).then(|| line.to_owned())
    };
    let (verb, icon) = match name {
        "Agent" | "Task" => {
            row.detail = text("subagent_type");
            ("delegate", "agents")
        }
        "Write" => ("write", "file-plus"),
        "WebSearch" => {
            if let Some(target) = text("query") {
                row.target = target;
            }
            row.detail = Some("web".into());
            ("search", "search")
        }
        "WebFetch" => {
            if let Some(url) = text("url") {
                let rest = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
                let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
                row.target = host.to_owned();
                row.detail = (!path.is_empty()).then(|| format!("/{path}"));
            }
            row.path = None;
            ("read", "globe")
        }
        "ToolSearch" => {
            if let Some(target) = text("query").map(|query| loaded(&query)) {
                row.target = target;
            }
            ("load", "plug")
        }
        "Skill" => {
            if let Some(target) = text("skill") {
                row.target = target;
            }
            ("skill", "book")
        }
        "AskUserQuestion" => ("ask", "message"),
        "Monitor" => {
            if let Some(target) = text("description").or_else(|| text("command")) {
                row.target = target;
            }
            ("watch", "eye")
        }
        "TaskStop" | "KillShell" => {
            if let Some(target) = text("task_id").or_else(|| text("shell_id")) {
                row.target = target;
            }
            ("stop", "stop")
        }
        "SendMessage" => {
            if let Some(target) = text("to") {
                row.target = target;
            }
            row.detail = text("summary");
            ("message", "message")
        }
        "ListAgents" => {
            row.target = "agents".into();
            ("list", "agents")
        }
        "ScheduleWakeup" => {
            let seconds = args.get("delaySeconds").and_then(Value::as_f64);
            row.target = match seconds {
                Some(seconds) => duration(seconds * 1000.),
                None => "no more".into(),
            };
            row.detail = text("reason");
            ("sleep", "timer")
        }
        "Artifact" => {
            if let Some(target) = text("title")
                .or_else(|| text("file_path").map(|path| basename(&path).to_owned()))
                .or_else(|| text("url"))
            {
                row.target = target;
            }
            let verb = match text("action").as_deref() {
                None | Some("publish") => "publish",
                Some("read") => "read",
                Some("list") => "list",
                Some("delete") => "delete",
                Some(_) => "open",
            };
            (verb, "share")
        }
        "StructuredOutput" => {
            row.target = "structured result".into();
            ("answer", "check")
        }
        "Workflow" => {
            if let Some(target) =
                text("name").or_else(|| text("scriptPath").map(|path| basename(&path).to_owned()))
            {
                row.target = target;
            }
            ("workflow", "workflow")
        }
        "SendUserFile" => {
            let files = args.get("files").and_then(Value::as_array);
            row.target = files
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(basename)
                .collect::<Vec<_>>()
                .join(", ");
            ("send", "upload")
        }
        "PushNotification" => {
            if let Some(target) = text("message") {
                row.target = target;
            }
            ("notify", "message")
        }
        _ => return,
    };
    row.verb = Some(verb.to_owned());
    row.icon = Some(icon.to_owned());
    row.activity = Some(activity(verb, &row.target));
    if row.detail.as_deref() == Some(row.target.as_str()) {
        row.detail = None;
    }
}

/// The tools a search for tools loaded, by their own names: `select:mcp__s__a,b` as `a, b`.
fn loaded(query: &str) -> String {
    match query.strip_prefix("select:") {
        Some(names) => names
            .split(',')
            .map(|name| name.trim().rsplit("__").next().unwrap_or_default())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        None => query.to_owned(),
    }
}
