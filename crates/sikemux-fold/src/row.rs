//! What a tool call's row shows, worked out once here so every client draws
//! the same row: what the call did, to what, and the file it opens. Clients
//! keep only their own words, icons and colours for each action.

use serde_json::{Map, Value};
use sikemux_wire::chat::{ToolAction, ToolCall, ToolRow};

use super::tool_rows::SPECS;
use super::tools::sikemux_tool_name;

/// How a row of one of our tools reads, declared beside the tool's schema in
/// browser/tools.json or its plugin's manifest. The first template whose every
/// `{argument}` the call was given is filled in.
pub(super) struct Spec {
    pub verb: &'static str,
    pub kind: &'static str,
    pub icon: &'static str,
    pub target: &'static [&'static str],
    pub detail: &'static [&'static str],
    pub plugin: Option<&'static str>,
}

/// A running call's own words are kept only while they fit the line that shows them.
const MAX_ACTIVITY_CHARS: usize = 40;
const MAX_DETAIL_CHARS: usize = 120;
const MAX_VERB_CHARS: usize = 12;

pub(super) fn row(tool: &ToolCall) -> ToolRow {
    let args = arguments(tool.raw_input.as_ref());
    match spec(&tool.title) {
        Some(spec) => ours(spec, &args),
        None => theirs(tool, &args),
    }
}

fn spec(title: &str) -> Option<&'static Spec> {
    let name = sikemux_tool_name(title);
    SPECS
        .iter()
        .find(|(named, _)| *named == name)
        .map(|(_, spec)| spec)
}

fn ours(spec: &Spec, args: &Map<String, Value>) -> ToolRow {
    let target = fill(spec.target, args).unwrap_or_default();
    let detail = fill(spec.detail, args).filter(|detail| !detail.is_empty() && *detail != target);
    let said = format!("{} {target}", spec.verb).trim().to_owned();
    ToolRow {
        action: action(spec.kind),
        activity: Some(if fits(&said, MAX_ACTIVITY_CHARS) {
            said
        } else {
            spec.verb.to_owned()
        }),
        target,
        detail,
        plugin: spec.plugin.map(str::to_owned),
        verb: Some(spec.verb.to_owned()),
        icon: Some(spec.icon.to_owned()),
        ..ToolRow::default()
    }
}

fn theirs(tool: &ToolCall, args: &Map<String, Value>) -> ToolRow {
    let (server, name) = label(&tool.title);
    let line = name.split('\n').next().unwrap_or_default().trim();
    let named = tool
        .kind
        .as_deref()
        .map(action)
        .filter(|action| *action != ToolAction::Other);
    let action = match (named, server) {
        (_, Some(_)) => ToolAction::Mcp,
        (Some(action), None) => action,
        (None, None) => ToolAction::Other,
    };
    let command = action == ToolAction::Execute;
    let touches_file = matches!(
        action,
        ToolAction::Read | ToolAction::Edit | ToolAction::Delete | ToolAction::Move
    );
    let patched = match (action, &tool.path) {
        (ToolAction::Edit, None) => args
            .get("input")
            .and_then(Value::as_str)
            .map(patch_files)
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let file = tool
        .path
        .as_deref()
        .map(without_line)
        .filter(|path| !command && (touches_file || names_file(line, path)))
        .or(patched.first().copied());
    let searched = args
        .get("pattern")
        .and_then(Value::as_str)
        .filter(|pattern| {
            action == ToolAction::Search && !pattern.is_empty() && !line.contains(*pattern)
        });
    let folders = searched
        .and(args.get("path").and_then(Value::as_str))
        .map(|path| {
            path.split(';')
                .map(str::trim)
                .filter(|folder| !matches!(*folder, "" | "." | "./"))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|folders| !folders.is_empty());
    let ran = args
        .get("command")
        .and_then(Value::as_str)
        .map(|ran| ran.split('\n').next().unwrap_or_default().trim())
        .filter(|ran| command && !ran.is_empty());
    let aside = match (file, ran, searched) {
        (_, _, Some(_)) => folders.clone().or_else(|| Some(line.to_owned())),
        (Some(path), _, _) => {
            (!line.is_empty() && !names_file(line, path)).then(|| line.to_owned())
        }
        (None, Some(ran), _) => {
            (!line.is_empty() && line.trim_start_matches("$ ") != ran).then(|| line.to_owned())
        }
        (None, None, None) => None,
    }
    .filter(|aside| !aside.is_empty());
    let target = match (server, file, ran, searched) {
        (Some(_), ..) => line.replace('_', " "),
        (None, _, _, Some(pattern)) => pattern.to_owned(),
        (None, Some(path), _, _) => match patched.len() {
            0 | 1 => basename(path).to_owned(),
            files => format!("{} and {} more", basename(path), files - 1),
        },
        (None, None, Some(ran), _) => ran.to_owned(),
        (None, None, None, None) if !command && is_path(line) => basename(line).to_owned(),
        (None, None, None, None) => line.to_owned(),
    };
    let verb = (action == ToolAction::Other).then(|| {
        let word = name
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or_default();
        word.chars()
            .take(MAX_VERB_CHARS)
            .collect::<String>()
            .to_lowercase()
    });
    let says = matches!(action, ToolAction::Mcp | ToolAction::Other);
    ToolRow {
        action,
        detail: server.and_then(|_| first_short_argument(args)).or(aside),
        server: server.map(str::to_owned),
        path: (!command)
            .then(|| {
                tool.path
                    .clone()
                    .or_else(|| patched.first().map(|path| (*path).to_owned()))
                    .or_else(|| is_path(line).then(|| line.to_owned()))
            })
            .flatten()
            .filter(|path| !path.contains(';')),
        icon: (verb.as_deref() == Some("toolsearch")).then(|| "wrench".to_owned()),
        activity: (says && !target.is_empty() && fits(&target, MAX_ACTIVITY_CHARS))
            .then(|| target.clone()),
        verb,
        target,
        plugin: None,
    }
}

fn action(kind: &str) -> ToolAction {
    match kind {
        "read" => ToolAction::Read,
        "edit" => ToolAction::Edit,
        "delete" => ToolAction::Delete,
        "move" => ToolAction::Move,
        "search" => ToolAction::Search,
        "execute" => ToolAction::Execute,
        "think" => ToolAction::Think,
        "fetch" => ToolAction::Fetch,
        "switch_mode" => ToolAction::SwitchMode,
        _ => ToolAction::Other,
    }
}

/// An MCP call's server and tool, from `mcp__server__tool`.
fn label(title: &str) -> (Option<&str>, &str) {
    let mut segments = title.splitn(3, "__");
    match (segments.next(), segments.next(), segments.next()) {
        (Some("mcp"), Some(server), Some(name)) => (Some(server), name),
        _ => (None, title),
    }
}

/// The files a patch names, in order: `*** Update File: path` as Codex and OMP write
/// them, or `+++ b/path` as a unified diff does.
pub(super) fn patch_files(patch: &str) -> Vec<&str> {
    let mut files: Vec<&str> = Vec::new();
    for line in patch.lines() {
        let named = [
            "*** Update File: ",
            "*** Add File: ",
            "*** Delete File: ",
            "+++ b/",
            "+++ ",
        ]
        .into_iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != "/dev/null");
        if let Some(path) = named.filter(|path| !files.contains(path)) {
            files.push(path);
        }
    }
    files
}

/// `path` without the `:line` or `:first-last` lines a call's location adds.
fn without_line(path: &str) -> &str {
    match path.rsplit_once(':') {
        Some((file, lines))
            if lines.starts_with(|c: char| c.is_ascii_digit())
                && lines
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-') =>
        {
            file
        }
        _ => path,
    }
}

/// Whether a title names the file at `path`: the path alone, or a word such as `Read`
/// before it, whole or relative to the folder the agent works in.
fn names_file(line: &str, path: &str) -> bool {
    if line == path {
        return true;
    }
    let named = line
        .split_once(char::is_whitespace)
        .map_or(line, |(_, rest)| rest.trim());
    !named.is_empty() && (named == path || path.ends_with(&format!("/{named}")))
}

/// A title names a file when it is one word with a slash in it, and not a page.
fn is_path(line: &str) -> bool {
    line.contains('/') && !line.contains(char::is_whitespace) && !is_web_url(line)
}

fn is_web_url(text: &str) -> bool {
    let Some(rest) = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty() && !host.contains('@') && !rest.contains(char::is_whitespace)
}

fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rsplit(['/', '\\']).next() {
        Some(name) if !name.is_empty() => name,
        _ => path,
    }
}

fn fits(text: &str, most: usize) -> bool {
    text.encode_utf16().count() <= most
}

/// Some agents hand over the server and tool beside the arguments rather than the arguments alone.
fn arguments(input: Option<&Value>) -> Map<String, Value> {
    let Some(input) = input.and_then(Value::as_object) else {
        return Map::new();
    };
    input
        .get("arguments")
        .and_then(Value::as_object)
        .unwrap_or(input)
        .clone()
}

fn first_short_argument(args: &Map<String, Value>) -> Option<String> {
    let first = args
        .values()
        .filter_map(Value::as_str)
        .find(|text| !text.trim().is_empty() && !text.contains('\n'))?;
    fits(first, MAX_DETAIL_CHARS).then(|| first.trim().to_owned())
}

/// The first template whose every argument the call was given, filled in.
fn fill(templates: &[&str], args: &Map<String, Value>) -> Option<String> {
    templates.iter().find_map(|template| {
        let mut filled = String::new();
        let mut rest = *template;
        while let Some(open) = rest.find('{') {
            let close = rest[open..].find('}').map(|at| open + at)?;
            let key = &rest[open + 1..close];
            filled.push_str(&rest[..open]);
            filled.push_str(&value_text(key, args.get(key)?)?);
            rest = &rest[close + 1..];
        }
        filled.push_str(rest);
        Some(filled)
    })
}

/// An argument in a row's words: a path by its name, a wait as a duration, a list joined or counted.
fn value_text(key: &str, value: &Value) -> Option<String> {
    let shorten = |text: &str| {
        if key.to_lowercase().contains("path") {
            basename(text).to_owned()
        } else {
            text.to_owned()
        }
    };
    match value {
        Value::String(text) => {
            let line = text.split('\n').next().unwrap_or_default().trim();
            (!line.is_empty()).then(|| shorten(line))
        }
        Value::Number(number) if key == "ms" || key.ends_with("Ms") => {
            number.as_f64().map(duration)
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Array(items) if items.is_empty() => None,
        Value::Array(items) => match items.iter().map(Value::as_str).collect::<Option<Vec<_>>>() {
            Some(texts) => Some(
                texts
                    .into_iter()
                    .map(shorten)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            None => Some(items.len().to_string()),
        },
        _ => None,
    }
}

/// How long something took, as the app says it: milliseconds under a second, then seconds, minutes and hours.
fn duration(ms: f64) -> String {
    let ms = ms.max(0.);
    if ms < 1000. {
        return format!("{}ms", ms.round());
    }
    let seconds = (ms / 1000.).round() as u64;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}
