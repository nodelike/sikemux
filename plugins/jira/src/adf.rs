// Jira keeps descriptions and comments as ADF, the Atlassian Document Format: a
// JSON tree of blocks and marked-up text. Agents and the issues view read and
// write markdown, so this turns one into the other, keeping what both can say
// and reducing the rest (panels, media, mentions) to plain text.

use serde_json::{json, Map, Value};

fn content(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn attr<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("attrs").and_then(|attrs| attrs.get(name))
}

fn kind(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or_default()
}

/// ADF to markdown. Anything that is not an ADF document is shown as it is.
pub fn to_markdown(document: &Value) -> String {
    match document {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        _ => blocks(content(document), "").trim_end().to_string(),
    }
}

fn blocks(nodes: &[Value], indent: &str) -> String {
    nodes
        .iter()
        .map(|node| block(node, indent))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block(node: &Value, indent: &str) -> String {
    match kind(node) {
        "paragraph" => format!("{indent}{}", inlines(content(node))),
        "heading" => {
            let level = attr(node, "level")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            format!("{indent}{} {}", "#".repeat(level), inlines(content(node)))
        }
        "bulletList" => list(content(node), indent, |_| "- ".to_string()),
        "orderedList" => {
            let start = attr(node, "order").and_then(Value::as_u64).unwrap_or(1);
            list(content(node), indent, |index| {
                format!("{}. ", start + index as u64)
            })
        }
        "taskList" => content(node)
            .iter()
            .map(|item| {
                let done = attr(item, "state").and_then(Value::as_str) == Some("DONE");
                format!(
                    "{indent}- [{}] {}",
                    if done { "x" } else { " " },
                    inlines(content(item))
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "codeBlock" => {
            let language = attr(node, "language")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let code = content(node)
                .iter()
                .filter_map(|text| text.get("text").and_then(Value::as_str))
                .collect::<String>();
            format!("{indent}```{language}\n{code}\n{indent}```")
        }
        "blockquote" | "panel" => blocks(content(node), "")
            .lines()
            .map(|line| format!("{indent}> {line}").trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        "rule" => format!("{indent}---"),
        "table" => table(node),
        "mediaSingle" | "mediaGroup" | "media" => format!("{indent}[attachment]"),
        _ if !content(node).is_empty() => blocks(content(node), indent),
        _ => inline(node),
    }
}

fn list(items: &[Value], indent: &str, marker: impl Fn(usize) -> String) -> String {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let marker = marker(index);
            let nested = format!("{indent}{}", " ".repeat(marker.len()));
            let mut parts = content(item).iter();
            let first = parts
                .next()
                .map(|first| block(first, ""))
                .unwrap_or_default();
            let rest = parts
                .map(|part| block(part, &nested))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>();
            let mut text = format!("{indent}{marker}{first}");
            for part in rest {
                text.push('\n');
                text.push_str(&part);
            }
            text
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn table(node: &Value) -> String {
    let rows: Vec<Vec<String>> = content(node)
        .iter()
        .map(|row| {
            content(row)
                .iter()
                .map(|cell| {
                    blocks(content(cell), "")
                        .replace('\n', " ")
                        .replace('|', "\\|")
                })
                .collect()
        })
        .collect();
    let Some(first) = rows.first() else {
        return String::new();
    };
    let mut lines = vec![
        format!("| {} |", first.join(" | ")),
        format!("|{}|", vec![" --- "; first.len()].join("|")),
    ];
    lines.extend(
        rows.iter()
            .skip(1)
            .map(|row| format!("| {} |", row.join(" | "))),
    );
    lines.join("\n")
}

fn inlines(nodes: &[Value]) -> String {
    nodes.iter().map(inline).collect()
}

fn inline(node: &Value) -> String {
    match kind(node) {
        "text" => {
            let text = node.get("text").and_then(Value::as_str).unwrap_or_default();
            marked(
                text,
                node.get("marks")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            )
        }
        "hardBreak" => "\n".into(),
        "mention" => {
            let name = attr(node, "text")
                .and_then(Value::as_str)
                .unwrap_or("someone");
            if name.starts_with('@') {
                name.to_string()
            } else {
                format!("@{name}")
            }
        }
        "emoji" => attr(node, "text")
            .or_else(|| attr(node, "shortName"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "inlineCard" | "blockCard" => attr(node, "url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "status" => attr(node, "text")
            .and_then(Value::as_str)
            .map(|text| format!("[{text}]"))
            .unwrap_or_default(),
        "date" => attr(node, "timestamp")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => inlines(content(node)),
    }
}

fn marked(text: &str, marks: &[Value]) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = text.to_string();
    let has = |name: &str| marks.iter().any(|mark| kind(mark) == name);
    if has("code") {
        out = format!("`{out}`");
    } else {
        if has("strong") {
            out = format!("**{out}**");
        }
        if has("em") {
            out = format!("*{out}*");
        }
        if has("strike") {
            out = format!("~~{out}~~");
        }
    }
    if let Some(href) = marks
        .iter()
        .find(|mark| kind(mark) == "link")
        .and_then(|mark| attr(mark, "href"))
        .and_then(Value::as_str)
    {
        out = format!("[{out}]({href})");
    }
    out
}

/// Markdown to an ADF document: headings, paragraphs, lists, task lists,
/// quotes, fenced code, rules, and bold, italic, strike, code and links inline.
pub fn from_markdown(markdown: &str) -> Value {
    let lines: Vec<&str> = markdown.lines().collect();
    json!({ "type": "doc", "version": 1, "content": parse_blocks(&lines) })
}

fn parse_blocks(lines: &[&str]) -> Vec<Value> {
    let mut nodes = Vec::new();
    let mut rest = lines;
    while let Some((&line, after)) = rest.split_first() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            rest = after;
        } else if let Some(language) = trimmed.strip_prefix("```") {
            let end = after
                .iter()
                .position(|candidate| candidate.trim().starts_with("```"))
                .unwrap_or(after.len());
            let (code, tail) = after.split_at(end);
            let mut block = Map::new();
            block.insert("type".into(), json!("codeBlock"));
            if !language.trim().is_empty() {
                block.insert("attrs".into(), json!({ "language": language.trim() }));
            }
            let body = if code.is_empty() {
                json!([])
            } else {
                json!([{ "type": "text", "text": code.join("\n") }])
            };
            block.insert("content".into(), body);
            nodes.push(Value::Object(block));
            rest = tail.split_first().map(|(_, tail)| tail).unwrap_or_default();
        } else if let Some((level, text)) = heading(trimmed) {
            nodes.push(json!({ "type": "heading", "attrs": { "level": level }, "content": parse_inline(text) }));
            rest = after;
        } else if matches!(trimmed, "---" | "***" | "___") {
            nodes.push(json!({ "type": "rule" }));
            rest = after;
        } else if trimmed.starts_with('>') {
            let count = rest
                .iter()
                .take_while(|candidate| candidate.trim().starts_with('>'))
                .count();
            let (quoted, tail) = rest.split_at(count);
            let inner: Vec<&str> = quoted
                .iter()
                .map(|candidate| {
                    let text = candidate.trim().trim_start_matches('>');
                    text.strip_prefix(' ').unwrap_or(text)
                })
                .collect();
            nodes.push(json!({ "type": "blockquote", "content": parse_blocks(&inner) }));
            rest = tail;
        } else if list_marker(line).is_some() {
            let count = rest
                .iter()
                .take_while(|candidate| {
                    list_marker(candidate).is_some()
                        || (indent_of(candidate) > 0 && !candidate.trim().is_empty())
                })
                .count();
            let (items, tail) = rest.split_at(count);
            nodes.push(parse_list(items));
            rest = tail;
        } else {
            let count = rest
                .iter()
                .take_while(|candidate| {
                    let text = candidate.trim();
                    !text.is_empty()
                        && !text.starts_with("```")
                        && heading(text).is_none()
                        && !text.starts_with('>')
                        && list_marker(candidate).is_none()
                })
                .count()
                .max(1);
            let (paragraph, tail) = rest.split_at(count);
            let mut inline = Vec::new();
            for (index, text) in paragraph.iter().enumerate() {
                if index > 0 {
                    inline.push(json!({ "type": "hardBreak" }));
                }
                inline.extend(parse_inline(text.trim()));
            }
            nodes.push(json!({ "type": "paragraph", "content": inline }));
            rest = tail;
        }
    }
    nodes
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    let text = line.get(level..)?.strip_prefix(' ')?;
    (1..=6).contains(&level).then_some((level, text.trim()))
}

fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|character| *character == ' ')
        .count()
}

enum Marker {
    Bullet,
    Ordered(u64),
    Task(bool),
}

/// The list marker a line starts with, if any, and the text after it.
fn list_marker(line: &str) -> Option<(Marker, &str)> {
    let text = line.trim_start();
    for bullet in ["- ", "* ", "+ "] {
        if let Some(after) = text.strip_prefix(bullet) {
            for (box_, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
                if let Some(task) = after.strip_prefix(box_) {
                    return Some((Marker::Task(done), task));
                }
            }
            return Some((Marker::Bullet, after));
        }
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let number = text.get(..digits)?.parse().ok()?;
    let after = text.get(digits..)?;
    let after = after
        .strip_prefix(". ")
        .or_else(|| after.strip_prefix(") "))?;
    Some((Marker::Ordered(number), after))
}

fn parse_list(lines: &[&str]) -> Value {
    let base = lines
        .first()
        .map(|line| indent_of(line))
        .unwrap_or_default();
    let first = lines
        .first()
        .and_then(|line| list_marker(line))
        .map(|(marker, _)| marker);
    let mut items: Vec<(String, Vec<&str>, Option<bool>)> = Vec::new();
    for line in lines {
        match list_marker(line) {
            Some((marker, text)) if indent_of(line) <= base => {
                let done = match marker {
                    Marker::Task(done) => Some(done),
                    _ => None,
                };
                items.push((text.to_string(), Vec::new(), done));
            }
            _ => {
                if let Some((_, nested, _)) = items.last_mut() {
                    nested.push(line.get(base.min(indent_of(line))..).unwrap_or(line));
                }
            }
        }
    }
    if let Some(Marker::Task(_)) = first {
        let tasks: Vec<Value> = items
            .iter()
            .enumerate()
            .map(|(index, (text, _, done))| {
                json!({ "type": "taskItem", "attrs": { "localId": format!("task-{index}"), "state": if done.unwrap_or(false) { "DONE" } else { "TODO" } }, "content": parse_inline(text) })
            })
            .collect();
        return json!({ "type": "taskList", "attrs": { "localId": "tasks" }, "content": tasks });
    }
    let list_items: Vec<Value> = items
        .iter()
        .map(|(text, nested, _)| {
            let mut body = vec![json!({ "type": "paragraph", "content": parse_inline(text) })];
            let nested_lines: Vec<&str> = nested.iter().map(|line| line.trim_start()).collect();
            if !nested_lines.is_empty() {
                body.extend(parse_blocks(&nested_lines));
            }
            json!({ "type": "listItem", "content": body })
        })
        .collect();
    match first {
        Some(Marker::Ordered(start)) if start != 1 => {
            json!({ "type": "orderedList", "attrs": { "order": start }, "content": list_items })
        }
        Some(Marker::Ordered(_)) => json!({ "type": "orderedList", "content": list_items }),
        _ => json!({ "type": "bulletList", "content": list_items }),
    }
}

/// Inline markdown to ADF text nodes with marks.
fn parse_inline(text: &str) -> Vec<Value> {
    let mut nodes = Vec::new();
    inline_into(text, &[], &mut nodes);
    nodes
}

fn push_text(nodes: &mut Vec<Value>, text: &str, marks: &[Value]) {
    if text.is_empty() {
        return;
    }
    let mut node = Map::new();
    node.insert("type".into(), json!("text"));
    node.insert("text".into(), json!(text));
    if !marks.is_empty() {
        node.insert("marks".into(), Value::Array(marks.to_vec()));
    }
    nodes.push(Value::Object(node));
}

fn with(marks: &[Value], mark: Value) -> Vec<Value> {
    let mut all = marks.to_vec();
    all.push(mark);
    all
}

fn inline_into(text: &str, marks: &[Value], nodes: &mut Vec<Value>) {
    let mut plain = String::new();
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        let inside_word = plain.chars().last().is_some_and(char::is_alphanumeric);
        let delimited: Option<(&str, &str, Value)> = [
            ("**", "strong"),
            ("__", "strong"),
            ("~~", "strike"),
            ("*", "em"),
            ("_", "em"),
        ]
        .iter()
        .find_map(|(delimiter, mark)| {
            // An underscore inside a word, as in snake_case, is not emphasis.
            if delimiter.starts_with('_') && inside_word {
                return None;
            }
            let after = rest.strip_prefix(delimiter)?;
            let end = after.find(delimiter)?;
            let inner = after.get(..end)?;
            if inner.is_empty() || inner.starts_with(' ') {
                return None;
            }
            Some((
                inner,
                after.get(end + delimiter.len()..)?,
                json!({ "type": mark }),
            ))
        });
        if let Some((inner, after, mark)) = delimited {
            push_text(nodes, &std::mem::take(&mut plain), marks);
            inline_into(inner, &with(marks, mark), nodes);
            rest = after;
            continue;
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                push_text(nodes, &std::mem::take(&mut plain), marks);
                push_text(
                    nodes,
                    after.get(..end).unwrap_or_default(),
                    &[json!({ "type": "code" })],
                );
                rest = after.get(end + 1..).unwrap_or_default();
                continue;
            }
        }
        if let Some((label, href, after)) = link(rest) {
            push_text(nodes, &std::mem::take(&mut plain), marks);
            inline_into(
                label,
                &with(marks, json!({ "type": "link", "attrs": { "href": href } })),
                nodes,
            );
            rest = after;
            continue;
        }
        if rest.starts_with("https://") || rest.starts_with("http://") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let url = rest
                .get(..end)
                .unwrap_or_default()
                .trim_end_matches(['.', ',', ')', ';']);
            push_text(nodes, &std::mem::take(&mut plain), marks);
            push_text(
                nodes,
                url,
                &with(marks, json!({ "type": "link", "attrs": { "href": url } })),
            );
            rest = rest.get(url.len()..).unwrap_or_default();
            continue;
        }
        plain.push(character);
        rest = rest.get(character.len_utf8()..).unwrap_or_default();
    }
    push_text(nodes, &plain, marks);
}

/// `[label](href)` at the start of `text`: the label, the address, and what follows.
fn link(text: &str) -> Option<(&str, &str, &str)> {
    let after = text.strip_prefix('[')?;
    let close = after.find("](")?;
    let label = after.get(..close)?;
    let tail = after.get(close + 2..)?;
    let end = tail.find(')')?;
    Some((label, tail.get(..end)?, tail.get(end + 1..)?))
}

/// Letters and digits only, lowercased, so a task's words can be compared however they were marked up.
fn words(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn plain_text(node: &Value) -> String {
    match kind(node) {
        "text" => node
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "mention" | "emoji" | "date" | "status" => attr(node, "text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "inlineCard" => attr(node, "url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => content(node).iter().map(plain_text).collect(),
    }
}

/// Where the `wanted`th task sits, as child positions from the top of the document.
fn task_path(node: &Value, wanted: usize, seen: &mut usize, path: &mut Vec<usize>) -> bool {
    if kind(node) == "taskItem" {
        if *seen == wanted {
            return true;
        }
        *seen += 1;
    }
    for (position, child) in content(node).iter().enumerate() {
        path.push(position);
        if task_path(child, wanted, seen, path) {
            return true;
        }
        path.pop();
    }
    false
}

/// Ticks or clears the `index`th task in the document, counting in reading order as the
/// markdown shows them. The task's words must still be `text`, so a description edited
/// meanwhile is not changed in the wrong place. Returns whether the task was found.
pub fn set_task(document: &mut Value, index: usize, text: &str, done: bool) -> bool {
    let mut path = Vec::new();
    if !task_path(document, index, &mut 0, &mut path) {
        return false;
    }
    let Some(task) = path.into_iter().try_fold(document, |node, position| {
        node.get_mut("content")?.as_array_mut()?.get_mut(position)
    }) else {
        return false;
    };
    if words(&plain_text(task)) != words(text) {
        return false;
    }
    let Some(task) = task.as_object_mut() else {
        return false;
    };
    let attrs = task.entry("attrs").or_insert_with(|| json!({}));
    let Some(attrs) = attrs.as_object_mut() else {
        return false;
    };
    attrs.insert("state".into(), json!(if done { "DONE" } else { "TODO" }));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checklist() -> Value {
        json!({
            "type": "doc",
            "content": [
                { "type": "paragraph", "content": [text("Acceptance:")] },
                { "type": "taskList", "attrs": { "localId": "a" }, "content": [
                    { "type": "taskItem", "attrs": { "localId": "1", "state": "TODO" }, "content": [
                        text("A video whose finding is over "), { "type": "text", "text": "500", "marks": [{ "type": "strong" }] }, text(" characters sends")
                    ] },
                    { "type": "taskItem", "attrs": { "localId": "2", "state": "DONE" }, "content": [
                        { "type": "mention", "attrs": { "id": "x", "text": "@Irwan" } }, text(" confirms the wording")
                    ] }
                ] },
                { "type": "panel", "content": [
                    { "type": "taskList", "content": [ { "type": "taskItem", "attrs": { "state": "TODO" }, "content": [text("Inside a panel")] } ] }
                ] }
            ]
        })
    }

    fn states(document: &Value) -> Vec<String> {
        let mut found = Vec::new();
        fn walk(node: &Value, found: &mut Vec<String>) {
            if kind(node) == "taskItem" {
                found.push(
                    attr(node, "state")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
            }
            content(node).iter().for_each(|child| walk(child, found));
        }
        walk(document, &mut found);
        found
    }

    #[test]
    fn ticks_and_clears_one_task_and_leaves_the_rest_as_it_was() {
        let mut document = checklist();
        assert!(set_task(
            &mut document,
            0,
            "A video whose finding is over 500 characters sends",
            true
        ));
        assert!(set_task(
            &mut document,
            1,
            "@Irwan confirms the wording",
            false
        ));
        assert!(set_task(&mut document, 2, "Inside a panel", true));
        assert_eq!(states(&document), ["DONE", "TODO", "DONE"]);
        assert_eq!(
            document["content"][1]["content"][0]["attrs"]["localId"],
            "1"
        );
        assert_eq!(document["content"][0], checklist()["content"][0]);
    }

    #[test]
    fn refuses_a_task_whose_words_changed_or_that_is_not_there() {
        let mut document = checklist();
        assert!(!set_task(&mut document, 0, "Something else entirely", true));
        assert!(!set_task(&mut document, 7, "Inside a panel", true));
        assert_eq!(states(&document), ["TODO", "DONE", "TODO"]);
    }

    #[test]
    fn task_order_matches_the_markdown_the_pane_shows() {
        let markdown = to_markdown(&checklist());
        let boxes: Vec<&str> = markdown
            .lines()
            .filter(|line| line.contains("- [ ] ") || line.contains("- [x] "))
            .collect();
        assert_eq!(boxes.len(), 3, "{markdown}");
        assert!(
            boxes[0].contains("500") && boxes[1].contains("@Irwan") && boxes[2].contains("panel")
        );
    }

    fn text(value: &str) -> Value {
        json!({ "type": "text", "text": value })
    }

    #[test]
    fn a_jira_description_reads_as_markdown() {
        let document = json!({ "type": "doc", "version": 1, "content": [
            { "type": "heading", "attrs": { "level": 2 }, "content": [text("Steps")] },
            { "type": "paragraph", "content": [
                text("Open "),
                { "type": "text", "text": "Settings", "marks": [{ "type": "strong" }] },
                text(", see "),
                { "type": "text", "text": "docs", "marks": [{ "type": "link", "attrs": { "href": "https://x.dev" } }] },
                { "type": "hardBreak" },
                { "type": "mention", "attrs": { "text": "@Ana" } },
                text(" run "),
                { "type": "text", "text": "make", "marks": [{ "type": "code" }] }
            ] },
            { "type": "orderedList", "content": [
                { "type": "listItem", "content": [ { "type": "paragraph", "content": [text("one")] },
                    { "type": "bulletList", "content": [ { "type": "listItem", "content": [ { "type": "paragraph", "content": [text("inner")] } ] } ] } ] },
                { "type": "listItem", "content": [ { "type": "paragraph", "content": [text("two")] } ] }
            ] },
            { "type": "codeBlock", "attrs": { "language": "rust" }, "content": [text("fn main() {}")] },
            { "type": "panel", "content": [ { "type": "paragraph", "content": [text("Careful")] } ] },
            { "type": "rule" },
            { "type": "taskList", "content": [ { "type": "taskItem", "attrs": { "state": "DONE" }, "content": [text("shipped")] } ] }
        ] });
        assert_eq!(
            to_markdown(&document),
            "## Steps\n\nOpen **Settings**, see [docs](https://x.dev)\n@Ana run `make`\n\n1. one\n   - inner\n2. two\n\n```rust\nfn main() {}\n```\n\n> Careful\n\n---\n\n- [x] shipped"
        );
    }

    #[test]
    fn a_table_reads_as_a_markdown_table() {
        let cell = |value: &str| json!({ "type": "tableCell", "content": [ { "type": "paragraph", "content": [text(value)] } ] });
        let document = json!({ "type": "doc", "content": [ { "type": "table", "content": [
            { "type": "tableRow", "content": [cell("Key"), cell("State")] },
            { "type": "tableRow", "content": [cell("ABC-1"), cell("Done")] }
        ] } ] });
        assert_eq!(
            to_markdown(&document),
            "| Key | State |\n| --- | --- |\n| ABC-1 | Done |"
        );
    }

    #[test]
    fn markdown_is_written_back_as_adf() {
        let document = from_markdown(
            "# Summary\nFixed **the race** in `attach`, see [PR](https://x.dev/1).\nNext line\n\n- first\n- second *really*\n\n1. one\n2. two\n\n```sh\nmake test\n```\n\n> quoted\n\n---\n\n- [x] tested\n- [ ] released",
        );
        let blocks = document["content"].as_array().cloned().unwrap_or_default();
        let kinds: Vec<&str> = blocks
            .iter()
            .map(|block| block["type"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            kinds,
            [
                "heading",
                "paragraph",
                "bulletList",
                "orderedList",
                "codeBlock",
                "blockquote",
                "rule",
                "taskList"
            ]
        );
        assert_eq!(
            blocks.get(1),
            Some(&json!({ "type": "paragraph", "content": [
                text("Fixed "),
                { "type": "text", "text": "the race", "marks": [{ "type": "strong" }] },
                text(" in "),
                { "type": "text", "text": "attach", "marks": [{ "type": "code" }] },
                text(", see "),
                { "type": "text", "text": "PR", "marks": [{ "type": "link", "attrs": { "href": "https://x.dev/1" } }] },
                text("."),
                { "type": "hardBreak" },
                text("Next line")
            ] }))
        );
        assert_eq!(
            blocks.get(4).and_then(|block| block.get("attrs")),
            Some(&json!({ "language": "sh" }))
        );
        assert_eq!(
            blocks
                .get(7)
                .and_then(|block| block.pointer("/content/0/attrs/state")),
            Some(&json!("DONE"))
        );
    }

    #[test]
    fn markdown_survives_the_round_trip() {
        let markdown = "## Plan\n\nShip **it** with `make`, see [docs](https://x.dev)\n\n- one\n- two\n\n```js\nrun()\n```\n\n> note";
        assert_eq!(to_markdown(&from_markdown(markdown)), markdown);
    }

    #[test]
    fn a_bare_link_becomes_a_link_and_odd_input_is_kept_as_text() {
        let document = from_markdown("see https://jira.example/browse/ABC-1. thanks");
        assert_eq!(
            document.pointer("/content/0/content/1"),
            Some(
                &json!({ "type": "text", "text": "https://jira.example/browse/ABC-1", "marks": [{ "type": "link", "attrs": { "href": "https://jira.example/browse/ABC-1" } }] })
            )
        );
        assert_eq!(
            to_markdown(&from_markdown("2 * 3 = 6 and a_b_c")),
            "2 * 3 = 6 and a_b_c"
        );
        assert_eq!(to_markdown(&json!("plain")), "plain");
        assert_eq!(to_markdown(&Value::Null), "");
    }
}
