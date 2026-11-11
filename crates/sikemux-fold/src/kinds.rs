//! What kind of tool a call is and what it did, which a run of calls is counted and
//! worded by: a shell command by what the command does, our tools by their name, an MCP
//! server's by the service it reaches.

use serde_json::{Map, Value};
use sikemux_view::chat::{ToolAction, ToolCall, ToolFamily, ToolIntent};

use super::tools::sikemux_tool_name;

/// Calls the reader never needs to see as calls: they show as themselves, or only load
/// what the agent goes on to use.
const HOUSEKEEPING: &[&str] = &[
    "ToolSearch",
    "Skill",
    "StructuredOutput",
    "guide",
    "skills",
    "react",
    "gif",
    "page_preview",
    "page_show",
    "ask",
    "AskUserQuestion",
    "mark_chapter",
];

const PICTURES: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "heic", "bmp", "tiff", "avif",
];

pub(super) struct Kind {
    pub family: ToolFamily,
    pub intent: ToolIntent,
    pub subject: Option<String>,
}

pub(super) fn kind(
    tool: &ToolCall,
    args: &Map<String, Value>,
    action: ToolAction,
    server: Option<&str>,
) -> Kind {
    let ours = sikemux_tool_name(&tool.title);
    let ours = (ours != tool.title.trim()).then_some(ours);
    let name = ours
        .or(tool.name.as_deref())
        .or_else(|| server.and(tool.title.rsplit("__").next()))
        .unwrap_or_default();
    let plain = |family, intent| Kind {
        family,
        intent,
        subject: None,
    };
    if HOUSEKEEPING.contains(&name) {
        return plain(ToolFamily::Housekeeping, ToolIntent::Use);
    }
    if let Some(kind) = named(name, tool, args) {
        return kind;
    }
    if let Some(server) = server.filter(|_| ours.is_none()) {
        return plain(service(server, name), service_intent(name));
    }
    match action {
        ToolAction::Execute => command(tool),
        ToolAction::Read if tool.path.as_deref().is_some_and(is_picture) => {
            plain(ToolFamily::Files, ToolIntent::View)
        }
        ToolAction::Read => plain(ToolFamily::Files, ToolIntent::Read),
        ToolAction::Edit if tool.name.as_deref() == Some("Write") => {
            plain(ToolFamily::Files, ToolIntent::Write)
        }
        ToolAction::Edit => plain(ToolFamily::Files, ToolIntent::Edit),
        ToolAction::Delete => plain(ToolFamily::Files, ToolIntent::Delete),
        ToolAction::Move => plain(ToolFamily::Files, ToolIntent::Move),
        ToolAction::Search => plain(ToolFamily::Files, ToolIntent::Search),
        ToolAction::Fetch => plain(ToolFamily::Web, ToolIntent::Fetch),
        ToolAction::SwitchMode => plain(ToolFamily::Other, ToolIntent::Mode),
        ToolAction::Think | ToolAction::Mcp | ToolAction::Other => {
            plain(ToolFamily::Other, ToolIntent::Use)
        }
    }
}

/// A tool known by its name: ours, an agent's own, or Claude's browser's.
fn named(name: &str, tool: &ToolCall, args: &Map<String, Value>) -> Option<Kind> {
    use ToolFamily as F;
    use ToolIntent as I;
    let arg = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(|text| text.trim().trim_start_matches('@').to_owned())
            .filter(|text| !text.is_empty())
    };
    let (family, intent, subject) = match name {
        "Bash" | "PowerShell" => return Some(command(tool)),
        "WebSearch" => (F::Web, I::Search, arg("query")),
        "WebFetch" => (F::Web, I::Fetch, None),
        "Write" => (F::Files, I::Write, None),
        "Glob" | "Grep" => (F::Files, I::Search, None),
        "ExitPlanMode" | "EnterPlanMode" => (F::Other, I::Mode, None),
        "agent_send" | "SendMessage" => (F::Agents, I::Message, arg("to").or_else(|| arg("agent"))),
        "agent_spawn" => (F::Agents, I::Start, arg("name")),
        "Agent" | "Task" => (F::Agents, I::Start, arg("subagent_type")),
        "agent_close" => (F::Agents, I::Stop, arg("agent")),
        "agent_read" | "agent_list" | "agent_sessions" | "ListAgents" => {
            (F::Agents, I::Read, arg("agent"))
        }
        "agent_wait" => (F::Waiting, I::Wait, arg("agent").or_else(|| arg("agents"))),
        "events_wait" | "Monitor" | "ScheduleWakeup" | "process_read" | "task_read"
        | "BashOutput" | "TaskOutput" => (F::Waiting, I::Wait, None),
        "process_start" | "process_restart" | "task_start" | "task_restart" => {
            (F::Shell, I::Start, arg("label").or_else(|| arg("name")))
        }
        "process_stop" | "task_stop" | "TaskStop" | "KillShell" => (F::Shell, I::Stop, None),
        "git_commit" => (F::Git, I::Commit, None),
        "who_wrote" => (F::Git, I::Query, None),
        "browser_navigate" | "browser_switch_tab" | "navigate" | "preview_start" => {
            (F::Browser, I::Open, None)
        }
        "browser_click" | "browser_act" | "browser_drag" | "browser_scroll" | "computer" => {
            (F::Browser, I::Click, None)
        }
        "browser_type" | "browser_press" | "browser_upload" | "form_input" => {
            (F::Browser, I::Type, None)
        }
        "browser_screenshot" | "browser_record" | "browser_annotate" => (F::Browser, I::Look, None),
        "browser_wait" => (F::Browser, I::Wait, None),
        "browser_state"
        | "browser_find"
        | "browser_evaluate"
        | "browser_console"
        | "browser_network"
        | "find"
        | "read_page"
        | "get_page_text"
        | "javascript_tool"
        | "read_console_messages"
        | "read_network_requests"
        | "preview_logs" => (F::Browser, I::Read, None),
        "browser_viewport" | "browser_close_tab" | "browser_dialog" | "browser_batch"
        | "resize_window" | "preview_stop" => (F::Browser, I::Use, None),
        "sim_tap" | "sim_swipe" | "sim_button" | "sim_touch_path" | "sim_touch2_path" => {
            (F::Simulator, I::Click, None)
        }
        "sim_type" => (F::Simulator, I::Type, None),
        "sim_screenshot" => (F::Simulator, I::Look, None),
        "sim_state" | "sim_logs" | "sim_devices" => (F::Simulator, I::Read, None),
        "sim_launch" | "sim_open_url" | "sim_install" | "sim_attach" => {
            (F::Simulator, I::Open, None)
        }
        "sim_detach" | "sim_terminate" | "sim_rotate" => (F::Simulator, I::Use, None),
        _ => {
            let (prefix, _) = name.split_once('_')?;
            let family = match prefix {
                "bitbucket" | "github" | "gitlab" => F::Git,
                "signoz" | "aws" | "rundeck" | "vercel" | "db" | "app" => F::Ops,
                "jira" | "board" | "linear" => F::Trackers,
                _ => return None,
            };
            (family, service_intent(name), None)
        }
    };
    Some(Kind {
        family,
        intent,
        subject,
    })
}

/// The family of a call to another MCP server, by the service it reaches.
fn service(server: &str, name: &str) -> ToolFamily {
    let said = format!("{server} {name}").to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| said.contains(word));
    if has(&["browser", "chrome", "playwright", "puppeteer"]) {
        ToolFamily::Browser
    } else if has(&["github", "gitlab", "bitbucket"]) {
        ToolFamily::Git
    } else if has(&["jira", "atlassian", "linear", "confluence"]) {
        ToolFamily::Trackers
    } else if has(&[
        "sentry", "datadog", "grafana", "vercel", "aws", "postgres", "supabase",
    ]) {
        ToolFamily::Ops
    } else {
        ToolFamily::Other
    }
}

/// What a service's tool did, by the words its name starts with.
fn service_intent(name: &str) -> ToolIntent {
    let name = name.to_lowercase();
    let first = name
        .split(['_', '-'])
        .find(|word| {
            !matches!(
                *word,
                "bitbucket"
                    | "github"
                    | "jira"
                    | "board"
                    | "signoz"
                    | "aws"
                    | "rundeck"
                    | "vercel"
                    | "db"
                    | "app"
            )
        })
        .unwrap_or(&name);
    let starts = |words: &[&str]| words.iter().any(|word| first.starts_with(word));
    if starts(&[
        "get",
        "list",
        "search",
        "read",
        "query",
        "find",
        "fetch",
        "logs",
        "traces",
        "issue",
        "pull",
        "pipeline",
        "step",
        "execution",
        "metrics",
        "services",
        "fields",
        "dashboards",
        "describe",
        "tables",
        "databases",
        "identity",
        "profiles",
        "cost",
        "projects",
        "deployment",
        "domains",
        "env",
        "comments",
        "console",
        "jobs",
        "plan",
        "target",
        "task",
        "ec2",
        "ecs",
    ]) {
        ToolIntent::Query
    } else {
        ToolIntent::Use
    }
}

/// A shell command by what it does: reading, searching, listing, testing, building, git.
pub(super) fn command(tool: &ToolCall) -> Kind {
    let typed = typed_command(tool);
    let (intent, subject) = typed
        .as_deref()
        .map_or((ToolIntent::Run, None), command_intent);
    Kind {
        family: ToolFamily::Shell,
        intent,
        subject,
    }
}

fn typed_command(tool: &ToolCall) -> Option<String> {
    let input = tool.raw_input.as_ref().and_then(Value::as_object);
    match input.and_then(|input| input.get("command")) {
        Some(Value::String(command)) => Some(command.clone()),
        Some(Value::Array(words)) => {
            let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
            match words.as_slice() {
                [shell, flag, script, ..]
                    if shell.ends_with("sh") && flag.starts_with('-') && flag.contains('c') =>
                {
                    Some((*script).to_owned())
                }
                words => Some(words.join(" ")),
            }
        }
        _ => Some(tool.title.clone()).filter(|title| !title.trim().is_empty()),
    }
}

/// What a command does, read from its first step that is not a `cd` or a setting, and what
/// it reads, searches for or lists.
pub(super) fn command_intent(command: &str) -> (ToolIntent, Option<String>) {
    let line = command.lines().next().unwrap_or_default();
    let step = line
        .split("&&")
        .flat_map(|part| part.split(';'))
        .flat_map(|part| part.split("||"))
        .map(|step| step.split('|').next().unwrap_or_default())
        .map(words)
        .map(|words| without_prefixes(&words))
        .find(|words| {
            words.first().is_some_and(|first| {
                !matches!(
                    first.as_str(),
                    "cd" | "export" | "set" | "source" | "." | "pushd"
                )
            })
        });
    let Some(step) = step else {
        return (ToolIntent::Run, None);
    };
    let program = step[0].rsplit('/').next().unwrap_or_default();
    let args = &step[1..];
    let positional: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let sub = positional.first().copied().unwrap_or_default();
    let file = |at_least: usize| {
        (positional.len() >= at_least)
            .then(|| positional.last().map(|path| basename(path).to_owned()))
            .flatten()
    };
    match program {
        "cat" | "head" | "tail" | "less" | "more" | "bat" | "nl" | "wc" | "xxd" | "hexdump"
        | "file" => (ToolIntent::Read, file(1)),
        "sed" | "awk" => (ToolIntent::Read, file(2)),
        "grep" | "rg" | "ag" | "ack" | "egrep" | "fgrep" => (
            ToolIntent::Search,
            positional.first().map(|pattern| (*pattern).to_owned()),
        ),
        "find" | "fd" => {
            let named = args
                .iter()
                .position(|arg| matches!(arg.as_str(), "-name" | "-iname" | "-path"))
                .and_then(|at| args.get(at + 1))
                .cloned();
            (
                ToolIntent::Search,
                named.or_else(|| positional.first().map(|path| (*path).to_owned())),
            )
        }
        "ls" | "tree" | "eza" | "exa" => (
            ToolIntent::List,
            Some(positional.last().map_or(".", |path| *path).to_owned()),
        ),
        "git" => match sub {
            "grep" => (
                ToolIntent::Search,
                positional.get(1).map(|pattern| (*pattern).to_owned()),
            ),
            "show" | "cat-file" => (ToolIntent::Read, None),
            "commit" => (ToolIntent::Commit, None),
            _ => (ToolIntent::Git, None),
        },
        "pytest" | "vitest" | "jest" | "mocha" | "playwright" | "nextest" => {
            (ToolIntent::Test, None)
        }
        "tsc" | "eslint" | "prettier" | "biome" | "clippy" | "swiftlint" | "rustfmt" | "ruff"
        | "mypy" | "make" | "ninja" | "gradle" | "xcodebuild" => {
            let testing = positional.iter().any(|arg| arg.contains("test"));
            (
                if testing {
                    ToolIntent::Test
                } else {
                    ToolIntent::Build
                },
                None,
            )
        }
        "cargo" | "go" | "swift" | "dotnet" | "mvn" | "zig" => match sub {
            "test" | "nextest" => (ToolIntent::Test, None),
            "build" | "check" | "clippy" | "fmt" | "vet" | "lint" | "doc" => {
                (ToolIntent::Build, None)
            }
            _ => (ToolIntent::Run, None),
        },
        "pnpm" | "npm" | "yarn" | "bun" | "npx" | "bunx" | "deno" => {
            let script = match sub {
                "run" | "exec" | "x" | "dlx" => positional.get(1).copied().unwrap_or_default(),
                script => script,
            };
            if script.starts_with("test") || matches!(script, "vitest" | "jest" | "playwright") {
                (ToolIntent::Test, None)
            } else if [
                "build",
                "check",
                "lint",
                "typecheck",
                "tsc",
                "format",
                "fmt",
            ]
            .iter()
            .any(|word| script.starts_with(word))
            {
                (ToolIntent::Build, None)
            } else {
                (ToolIntent::Run, None)
            }
        }
        _ => (ToolIntent::Run, None),
    }
}

/// A step's words, with quotes taken off.
fn words(step: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in step.chars() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// A step without what only sets it up: `env`, `time`, `sudo`, `VAR=value`, `timeout 60`.
fn without_prefixes(words: &[String]) -> Vec<String> {
    let mut at = 0;
    while let Some(word) = words.get(at) {
        let setting = word.contains('=') && !word.starts_with('-') && !word.starts_with('=');
        match word.as_str() {
            "env" | "time" | "sudo" | "command" | "exec" | "nice" | "nohup" => at += 1,
            "timeout" | "gtimeout" => at += 2,
            _ if setting => at += 1,
            _ => break,
        }
    }
    words[at.min(words.len())..].to_vec()
}

fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

fn is_picture(path: &str) -> bool {
    let path = path.split(':').next().unwrap_or(path);
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| PICTURES.contains(&ext.to_lowercase().as_str()))
}
