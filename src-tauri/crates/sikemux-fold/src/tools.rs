//! What a tool call did, read once from what the agent handed it and handed
//! back, so the chat keeps the change, the output and the failure rather than
//! the payloads they came from.

use serde_json::{Map, Value};

use sikemux_wire::chat::{
    DiffLine, DiffSummary, OutputImage, OutputSummary, PageRef, ToolCall, ToolDetail, ToolDiff,
    ToolOutput,
};

/// Past this the transcript is the wrong place to read the change: the row
/// still names the file, and the diff pane shows the whole thing.
const MAX_DIFF_LINES: usize = 1200;
/// A finished call is kept for as long as the session is open, so what it
/// printed is kept only up to what a reader would scroll through under a row.
const MAX_OUTPUT_CHARS: usize = 16_000;
const MAX_OUTPUT_LINES: usize = 400;
const MAX_IMAGE_CHARS: usize = 1024 * 1024;
const MAX_FAILURE_CHARS: usize = 400;
/// What of an argument a row can name: its first line, this long.
const MAX_ARGUMENT_CHARS: usize = 240;
const MAX_ARGUMENTS: usize = 24;

fn text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

pub(super) fn running(status: Option<&str>) -> bool {
    !matches!(status, Some("completed" | "failed" | "cancelled"))
}

/// Our own tools, under whatever server name a host put in front of them.
pub(super) fn sikemux_tool_name(title: &str) -> &str {
    let named = title.find("sikemux-tools").and_then(|at| {
        let rest = &title[at + "sikemux-tools".len()..];
        let rest = match rest.strip_prefix("__") {
            Some(rest) => rest,
            None => rest.strip_prefix(['.', '/', ':'])?.trim_start(),
        };
        let end = rest
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        rest.starts_with(|c: char| c.is_ascii_lowercase())
            .then(|| &rest[..end])
    });
    named.unwrap_or_else(|| title.trim())
}

fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Where a single deleted line and the added line after it differ, so the
/// row shows the changed words rather than asking the reader to compare.
fn mark_pair(deleted: &mut DiffLine, added: &mut DiffLine) {
    let a: Vec<u16> = deleted.text.encode_utf16().collect();
    let b: Vec<u16> = added.text.encode_utf16().collect();
    if a.len() > 200 || b.len() > 200 {
        return;
    }
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let end = a[start..]
        .iter()
        .rev()
        .zip(b[start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    // Two lines that merely end in the same bracket are not an edit of one
    // another; a span is only worth marking when most of the line survived.
    let shared = start + end;
    if shared < 3 || (shared as f64) < a.len().min(b.len()) as f64 * 0.25 {
        return;
    }
    deleted.mark = Some(vec![start as u64, (a.len() - end) as u64]);
    added.mark = Some(vec![start as u64, (b.len() - end) as u64]);
}

fn mark_pairs(lines: &mut [DiffLine]) {
    for index in 0..lines.len().saturating_sub(1) {
        if lines[index].sign != "-" || lines[index + 1].sign != "+" {
            continue;
        }
        if index > 0 && lines[index - 1].sign == "-" {
            continue;
        }
        if lines.get(index + 2).is_some_and(|line| line.sign == "+") {
            continue;
        }
        let (left, right) = lines.split_at_mut(index + 1);
        mark_pair(&mut left[index], &mut right[0]);
    }
}

fn line(sign: &str, text: &str, old_line: Option<u64>, new_line: Option<u64>) -> DiffLine {
    DiffLine {
        sign: sign.to_owned(),
        text: text.to_owned(),
        old_line,
        new_line,
        mark: None,
    }
}

/// The lines of `old` and `new` with what was taken out and put in, or none
/// when there are too many to show.
pub(super) fn diff_lines(old: &str, new: &str) -> Option<Vec<DiffLine>> {
    let before = lines(old);
    let after = lines(new);
    if before.len() + after.len() > MAX_DIFF_LINES {
        return None;
    }
    let mut out = Vec::new();
    let (mut old_line, mut new_line) = (1u64, 1u64);
    let head = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let tail = before[head..]
        .iter()
        .rev()
        .zip(after[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    for text in &before[..head] {
        out.push(line(" ", text, Some(old_line), Some(new_line)));
        old_line += 1;
        new_line += 1;
    }
    let middle_before = &before[head..before.len() - tail];
    let middle_after = &after[head..after.len() - tail];
    let (rows, cols) = (middle_before.len(), middle_after.len());
    let mut grid = vec![0u32; (rows + 1) * (cols + 1)];
    let at = |i: usize, j: usize| i * (cols + 1) + j;
    for i in (0..rows).rev() {
        for j in (0..cols).rev() {
            grid[at(i, j)] = if middle_before[i] == middle_after[j] {
                grid[at(i + 1, j + 1)] + 1
            } else {
                grid[at(i + 1, j)].max(grid[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < rows && j < cols {
        if middle_before[i] == middle_after[j] {
            out.push(line(" ", middle_before[i], Some(old_line), Some(new_line)));
            old_line += 1;
            new_line += 1;
            i += 1;
            j += 1;
        } else if grid[at(i + 1, j)] >= grid[at(i, j + 1)] {
            out.push(line("-", middle_before[i], Some(old_line), None));
            old_line += 1;
            i += 1;
        } else {
            out.push(line("+", middle_after[j], None, Some(new_line)));
            new_line += 1;
            j += 1;
        }
    }
    for text in &middle_before[i..] {
        out.push(line("-", text, Some(old_line), None));
        old_line += 1;
    }
    for text in &middle_after[j..] {
        out.push(line("+", text, None, Some(new_line)));
        new_line += 1;
    }
    for text in &before[before.len() - tail..] {
        out.push(line(" ", text, Some(old_line), Some(new_line)));
        old_line += 1;
        new_line += 1;
    }
    mark_pairs(&mut out);
    Some(out)
}

struct Change<'a> {
    path: &'a str,
    old: &'a str,
    new: &'a str,
}

/// An adapter that follows the ACP spec hands over the change itself.
fn from_content(content: Option<&Value>) -> Option<Change<'_>> {
    for item in content?.as_array()? {
        let inner = item
            .get("content")
            .filter(|inner| inner.is_object())
            .unwrap_or(item);
        if text(inner.get("type")) != Some("diff") {
            continue;
        }
        let (Some(path), Some(new)) = (text(inner.get("path")), text(inner.get("newText"))) else {
            continue;
        };
        return Some(Change {
            path,
            old: text(inner.get("oldText")).unwrap_or_default(),
            new,
        });
    }
    None
}

/// One that does not still says what it was asked to write.
fn from_input(input: Option<&Value>) -> Option<Change<'_>> {
    let input = input?.as_object()?;
    let field = |names: &[&str]| names.iter().find_map(|name| text(input.get(*name)));
    Some(Change {
        path: field(&["file_path", "path", "filePath"])?,
        old: field(&["old_string", "oldText"]).unwrap_or_default(),
        new: field(&["new_string", "newText", "content"])?,
    })
}

/// A path the agent named relative to the folder it works in, made whole, so every
/// client opens the same file; a whole path, a home path or an address stays as it is.
pub(super) fn rooted(folder: Option<&std::path::Path>, path: String) -> String {
    match folder {
        Some(folder)
            if std::path::Path::new(&path).is_relative()
                && !path.is_empty()
                && !path.starts_with('~')
                && !path.contains("://") =>
        {
            folder.join(&path).to_string_lossy().into_owned()
        }
        _ => path,
    }
}

/// The call with the paths it names made whole against `folder`.
pub(super) fn root_call(folder: Option<&std::path::Path>, mut call: ToolCall) -> ToolCall {
    call.path = call.path.map(|path| rooted(folder, path));
    call.row.path = call.row.path.take().map(|path| rooted(folder, path));
    call
}

/// Gives the diff's lines the file's own numbers, counted from where its new text sits
/// in the file now. Lines counted within an edit's text alone would point at the wrong
/// place, so they go unnumbered when the text is not found.
pub(super) fn place(diff: &mut ToolDiff, raw: &Map<String, Value>, place: &super::Place) {
    let change = from_content(raw.get("content")).or_else(|| from_input(raw.get("rawInput")));
    let start = change.and_then(|change| place(change.path, change.new));
    let shift = |number: Option<u64>| Some(number? + start? - 1);
    for line in &mut diff.lines {
        line.old_line = shift(line.old_line);
        line.new_line = shift(line.new_line);
    }
}

pub(super) fn tool_diff(content: Option<&Value>, input: Option<&Value>) -> Option<ToolDiff> {
    let change = from_content(content).or_else(|| from_input(input))?;
    let lines = diff_lines(change.old, change.new)?;
    let adds = lines.iter().filter(|line| line.sign == "+").count() as u64;
    let dels = lines.iter().filter(|line| line.sign == "-").count() as u64;
    if adds == 0 && dels == 0 {
        return None;
    }
    Some(ToolDiff {
        path: change.path.to_owned(),
        adds,
        dels,
        lines,
    })
}

fn block_texts(blocks: &Value) -> Option<String> {
    let texts: Vec<&str> = blocks
        .as_array()?
        .iter()
        .filter_map(|block| text(block.get("text")))
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// Claude hands back the tool's own result, a string or a list of blocks.
/// Codex wraps a command's in `formatted_output`, and an MCP call's in `result`.
pub(super) fn output_text(raw: &Value) -> Option<String> {
    if let Some(text) = raw.as_str() {
        return Some(text.to_owned());
    }
    if let Some(blocks) = block_texts(raw) {
        return Some(blocks);
    }
    let record = raw.as_object()?;
    let direct = text(record.get("formatted_output"))
        .or_else(|| text(record.get("output")))
        .map(str::to_owned)
        .or_else(|| {
            record
                .get("result")
                .and_then(|result| result.get("content"))
                .and_then(block_texts)
        });
    if direct.is_some() {
        return direct;
    }
    let streams: Vec<&str> = [text(record.get("stdout")), text(record.get("stderr"))]
        .into_iter()
        .flatten()
        .filter(|stream| !stream.is_empty())
        .collect();
    if !streams.is_empty() {
        return Some(streams.join("\n"));
    }
    text(record.get("error"))
        .or_else(|| {
            record
                .get("error")
                .and_then(|error| text(error.get("message")))
        })
        .map(str::to_owned)
}

fn output_image(content: Option<&Value>) -> Option<OutputImage> {
    content?.as_array()?.iter().find_map(|entry| {
        let block = entry.get("content")?;
        let data = text(block.get("data"))?;
        let mime_type = text(block.get("mimeType"))?;
        (text(block.get("type")) == Some("image")
            && !data.is_empty()
            && data.len() <= MAX_IMAGE_CHARS
            && mime_type.starts_with("image/"))
        .then(|| OutputImage {
            data: data.to_owned(),
            mime_type: mime_type.to_owned(),
        })
    })
}

fn capped(text: &str) -> (String, bool) {
    let mut kept = text.trim_end();
    let mut cut = false;
    if let Some((at, _)) = kept.match_indices('\n').nth(MAX_OUTPUT_LINES - 1) {
        kept = &kept[..at];
        cut = true;
    }
    let mut units = 0;
    for (at, char) in kept.char_indices() {
        units += char.len_utf16();
        if units > MAX_OUTPUT_CHARS {
            kept = &kept[..at];
            cut = true;
            break;
        }
    }
    (kept.to_owned(), cut)
}

/// What a finished command or MCP call printed. Reads and edits already show
/// what they touched.
pub(super) fn tool_output(
    title: &str,
    kind: Option<&str>,
    raw_output: Option<&Value>,
    content: Option<&Value>,
) -> Option<ToolOutput> {
    if kind != Some("execute") && !title.starts_with("mcp__") {
        return None;
    }
    let raw = raw_output.and_then(output_text);
    let image = output_image(content);
    if raw.is_none() && image.is_none() {
        return None;
    }
    let (text, cut) = capped(raw.as_deref().unwrap_or_default());
    Some(ToolOutput {
        text,
        cut,
        exit_code: raw_output
            .and_then(|raw| raw.get("exit_code"))
            .and_then(Value::as_i64),
        image,
    })
}

pub(super) fn output_summary(output: &ToolOutput) -> OutputSummary {
    OutputSummary {
        lines: if output.text.is_empty() {
            0
        } else {
            output.text.split('\n').count() as u64
        },
        cut: output.cut,
        exit_code: output.exit_code,
        image: output.image.is_some(),
    }
}

/// What a failed call left behind, short enough to sit under it. Anything
/// longer belongs in the terminal the call came from.
pub(super) fn tool_failure(status: Option<&str>, raw_output: Option<&Value>) -> Option<String> {
    if status != Some("failed") {
        return None;
    }
    let raw = raw_output?;
    let said = raw.as_str().or_else(|| {
        let record = raw.as_object()?;
        text(record.get("output"))
            .or_else(|| text(record.get("stderr")))
            .or_else(|| text(record.get("error")))
    })?;
    let trimmed = said.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut chars = trimmed.char_indices();
    Some(match chars.nth(MAX_FAILURE_CHARS) {
        Some((at, _)) => format!("{}…", &trimmed[..at]),
        None => trimmed.to_owned(),
    })
}

/// The page a finished `page_show` call put in the reply, read from what it printed.
pub(super) fn tool_page(
    title: &str,
    status: Option<&str>,
    raw_output: Option<&Value>,
    content: Option<&Value>,
) -> Option<PageRef> {
    if sikemux_tool_name(title) != "page_show" || status != Some("completed") {
        return None;
    }
    let printed = raw_output.and_then(output_text).or_else(|| {
        let inner: Vec<Value> = content?
            .as_array()?
            .iter()
            .map(|entry| entry.get("content").cloned().unwrap_or(Value::Null))
            .collect();
        output_text(&Value::Array(inner))
    })?;
    let page = serde_json::from_str::<Value>(&printed)
        .ok()?
        .get("page")?
        .clone();
    let id = text(page.get("id"))?;
    let title = text(page.get("title"))?;
    if id.len() != 32
        || !id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    Some(PageRef {
        id: id.to_owned(),
        title: title.to_owned(),
        height: page
            .get("height")
            .and_then(Value::as_f64)
            .filter(|height| height.is_finite()),
    })
}

/// The line Claude writes beside a shell command to say what it is for.
fn description(kind: Option<&str>, input: Option<&Value>) -> Option<String> {
    if kind != Some("execute") {
        return None;
    }
    let said = text(input?.get("description"))?.trim();
    (!said.is_empty()).then(|| said.to_owned())
}

/// The first place a call reported touching, with its line when it gave one.
fn location(locations: Option<&Value>) -> Option<String> {
    let first = locations?.as_array()?.first()?;
    let path = text(first.get("path")).filter(|path| !path.is_empty())?;
    Some(match first.get("line").and_then(Value::as_u64) {
        Some(line) => format!("{path}:{line}"),
        None => path.to_owned(),
    })
}

fn first_line(text: &str) -> String {
    let line = text.split('\n').next().unwrap_or_default().trim();
    match line.char_indices().nth(MAX_ARGUMENT_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

/// An argument as short as a row can name it.
fn short(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => {
            let line = first_line(text);
            (!line.is_empty()).then(|| Value::String(line))
        }
        Value::Number(_) | Value::Bool(_) => Some(value.clone()),
        Value::Array(items) if items.iter().all(Value::is_string) => Some(Value::Array(
            items.iter().take(MAX_ARGUMENTS).filter_map(short).collect(),
        )),
        Value::Array(items) => Some(Value::from(items.len())),
        _ => None,
    }
}

/// The arguments a call was handed, each cut to what a row can name. Some
/// agents put the server and tool beside them, under `arguments`.
pub(super) fn short_input(input: Option<&Value>) -> Option<Value> {
    let input = input?.as_object()?;
    let mut kept = Map::new();
    for (key, value) in input.iter().take(MAX_ARGUMENTS) {
        let value = match (key.as_str(), value) {
            ("arguments", Value::Object(_)) => short_input(Some(value)),
            _ => short(value),
        };
        if let Some(value) = value {
            kept.insert(key.clone(), value);
        }
    }
    (!kept.is_empty()).then_some(Value::Object(kept))
}

/// A call as the transcript keeps it, from the latest of everything the agent
/// said of it.
pub(super) fn call(raw: &Map<String, Value>) -> ToolCall {
    let kind = text(raw.get("kind")).map(str::to_owned);
    ToolCall {
        tool_call_id: text(raw.get("toolCallId")).unwrap_or_default().to_owned(),
        title: text(raw.get("title")).unwrap_or_default().to_owned(),
        status: text(raw.get("status")).map(str::to_owned),
        raw_input: short_input(raw.get("rawInput")),
        path: location(raw.get("locations")),
        description: description(kind.as_deref(), raw.get("rawInput")),
        kind,
        row: Default::default(),
    }
    .with_row()
}

pub(super) trait WithRow {
    /// The call with its row worked out again from the rest of it.
    fn with_row(self) -> Self;
}

impl WithRow for ToolCall {
    fn with_row(mut self) -> Self {
        self.row = Box::new(super::row::row(&self));
        self
    }
}

/// What a finished call shows, read once.
pub(super) struct Settled {
    pub diff: Option<ToolDiff>,
    pub output: Option<ToolOutput>,
    pub page: Option<PageRef>,
    pub failure: Option<String>,
}

pub(super) fn settle(raw: &Map<String, Value>) -> Settled {
    let title = text(raw.get("title")).unwrap_or_default();
    let kind = text(raw.get("kind"));
    let status = text(raw.get("status"));
    let (content, raw_input, raw_output) = (
        raw.get("content"),
        raw.get("rawInput"),
        raw.get("rawOutput"),
    );
    let output = tool_output(title, kind, raw_output, content);
    let page = tool_page(title, status, raw_output, content);
    Settled {
        diff: tool_diff(content, raw_input),
        failure: output
            .is_none()
            .then(|| tool_failure(status, raw_output))
            .flatten(),
        output: page.is_none().then_some(output).flatten(),
        page,
    }
}

pub(super) fn summary(diff: &ToolDiff) -> DiffSummary {
    DiffSummary {
        path: diff.path.clone(),
        adds: diff.adds,
        dels: diff.dels,
    }
}

pub(super) fn detail(settled: &Settled) -> ToolDetail {
    ToolDetail {
        diff: settled.diff.clone(),
        output: settled.output.clone(),
    }
}
