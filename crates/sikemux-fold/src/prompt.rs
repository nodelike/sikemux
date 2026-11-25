//! What a message sent to the agent holds, read the way the chat shows it:
//! letters from other agents, the context sent with it, and the notes an
//! agent's harness writes to itself, which the person never sees.

use std::collections::{BTreeMap, VecDeque};

use sikemux_markdown::paths::is_image_path;
use sikemux_view::chat::{
    tracked_title, AgentLetter, LetterKind, SentChip, SentChipKind, SentContext, SentSpan,
    TrackedKind, AGENT_URI, EARLIER_URI, WOKEN_URI,
};

/// Tags Claude Code writes to itself. A finished background agent's notice
/// carries its whole report, paragraphs and all, so these are recognised by
/// name rather than by looking like a single short block.
const HARNESS_TAGS: [&str; 8] = [
    "task-notification",
    "system-reminder",
    "local-command-caveat",
    "local-command-stdout",
    "local-command-stderr",
    "command-name",
    "command-message",
    "command-args",
];

const LETTER_TAGS: [&str; 2] = ["agent-message", "question-answer"];
/// The sender the core names on the prompts of a chat's triggers.
const TRIGGER: &str = "trigger";

pub fn is_agent_letter(text: &str) -> bool {
    let text = text.trim_start();
    LETTER_TAGS.iter().any(|tag| {
        text.strip_prefix('<')
            .and_then(|rest| rest.strip_prefix(tag))
            .is_some_and(|rest| rest.starts_with(' '))
    })
}

fn tag_name(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('<')?;
    if !rest.starts_with(|c: char| c.is_ascii_lowercase()) {
        return None;
    }
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    rest.get(..end)
}

fn is_harness_markup(text: &str) -> bool {
    let mut rest = text;
    while !rest.is_empty() {
        let Some(name) = tag_name(rest).filter(|name| HARNESS_TAGS.contains(name)) else {
            return false;
        };
        if rest.as_bytes().get(name.len() + 1) != Some(&b'>') {
            return false;
        }
        let close = format!("</{name}>");
        let Some((_, after)) = rest.split_once(close.as_str()) else {
            return false;
        };
        rest = after.trim_start();
    }
    true
}

/// The agent echoes the notifications its harness writes to itself, and those
/// are one block of tags with no prose around them. Markdown drops such a
/// block whole, so echoing it would leave a message with nothing in it.
pub(super) fn is_markup_only(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    if is_agent_letter(trimmed) {
        return false;
    }
    if is_harness_markup(trimmed) {
        return true;
    }
    if !trimmed.starts_with('<') || has_blank_line(trimmed) {
        return false;
    }
    let Some(name) = tag_name(trimmed).filter(|name| !name.is_empty()) else {
        return false;
    };
    let close = format!("</{}>", name.to_lowercase());
    trimmed.len() > close.len()
        && (trimmed.get(trimmed.len() - close.len()..))
            .is_some_and(|end| end.eq_ignore_ascii_case(&close))
        && (trimmed.get(1 + name.len()..)).is_some_and(|after| after.contains('>'))
}

fn has_blank_line(text: &str) -> bool {
    text.split('\n').skip(1).any(|line| line.trim().is_empty()) && text.trim_end().contains('\n')
}

/// Claude records a stop as a user message, so replaying a session would show
/// the person saying something they never typed.
pub(super) fn is_interrupt_marker(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed
        .strip_prefix("[Request interrupted by user")
        .and_then(|rest| rest.strip_suffix(']'))
        .is_some_and(|middle| !middle.contains(']'))
}

fn unescaped(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

pub(super) fn attributes(source: &str) -> Vec<(&str, String)> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some((before, after)) = rest.split_once("=\"") {
        let key_start = before
            .trim_end_matches(|c: char| c.is_ascii_lowercase() || c == '-')
            .len();
        let key = before.get(key_start..).unwrap_or_default();
        let Some((value, after_value)) = after.split_once('"') else {
            break;
        };
        if !key.is_empty() {
            found.push((key, unescaped(value)));
        }
        rest = after_value;
    }
    found
}

/// The messages from other agents and answers a text is made of, or none when
/// it holds anything else.
pub(super) fn agent_letters(text: &str) -> Option<Vec<AgentLetter>> {
    if !is_agent_letter(text) {
        return None;
    }
    let mut letters = Vec::new();
    let mut rest = text;
    let mut outside = String::new();
    loop {
        let found = LETTER_TAGS
            .iter()
            .filter_map(|tag| rest.find(&format!("<{tag} ")).map(|at| (at, *tag)))
            .min();
        let Some((at, tag)) = found else {
            outside.push_str(rest);
            break;
        };
        let Some((before, from_tag)) = rest.split_at_checked(at) else {
            outside.push_str(rest);
            break;
        };
        outside.push_str(before);
        let open = from_tag.split_once(">\n");
        let Some((open, after_open)) = open.filter(|(open, _)| !open.contains('>')) else {
            outside.push_str(from_tag);
            break;
        };
        let source = open.get(tag.len() + 2..).unwrap_or_default();
        let close = format!("\n</{tag}>");
        let Some((body, after_close)) = after_open.split_once(close.as_str()) else {
            outside.push_str(from_tag);
            break;
        };
        let body = body.replace(&format!("&lt;/{tag}"), &format!("</{tag}"));
        let found = attributes(source);
        let attribute = |key: &str| {
            found
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        letters.push(if tag == "question-answer" {
            let (text, attachments) = split_attached(&body);
            AgentLetter {
                kind: LetterKind::Answer,
                name: attribute("asker").trim_start_matches('@').to_owned(),
                agent_id: attribute("asker-id"),
                cli: attribute("cli"),
                question: attribute("question"),
                text,
                secret: attribute("secret") == "true",
                attachments,
                tree: None,
                trigger: None,
            }
        } else {
            let agent_id = attribute("agent-id");
            let trigger = (agent_id == TRIGGER)
                .then(|| super::trigger::parse(&body))
                .flatten();
            let text = trigger.as_ref().map_or(body, |fired| fired.prompt.clone());
            AgentLetter {
                kind: match trigger {
                    Some(_) => LetterKind::Trigger,
                    None => LetterKind::Message,
                },
                name: attribute("from").trim_start_matches('@').to_owned(),
                agent_id,
                cli: attribute("cli"),
                question: String::new(),
                tree: Some(super::send::read(&text)),
                text,
                secret: false,
                attachments: Vec::new(),
                trigger,
            }
        });
        rest = after_close;
    }
    (!letters.is_empty() && outside.trim().is_empty()).then_some(letters)
}

/// `text` without the `[@name](file://path)` links a saved session keeps the files sent with a
/// message as, and their paths: a picture's where the `[Image #n]` before its link numbers it,
/// the others in the order of the text.
pub(super) fn split_file_links(text: &str) -> (String, Vec<String>) {
    let mut said = String::new();
    let mut pictures = BTreeMap::new();
    let mut others = VecDeque::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("[@") {
        said.push_str(before);
        let Some((path, after_link)) = file_link(after) else {
            said.push_str("[@");
            rest = after;
            continue;
        };
        match image_number(&said).filter(|number| !pictures.contains_key(number)) {
            Some(number) => {
                pictures.insert(number, path);
            }
            None => others.push_back(path),
        }
        rest = after_link;
    }
    said.push_str(rest);
    let mut paths = Vec::new();
    while let Some((number, path)) = pictures.pop_first() {
        while paths.len() + 1 < number {
            let Some(other) = others.pop_front() else {
                break;
            };
            paths.push(other);
        }
        paths.push(path);
    }
    paths.extend(others);
    (said, paths)
}

/// The path of the link that starts `text`, after its `[@`, and what follows the link. Its words
/// are the file's name, which tells its closing bracket from one in the path.
fn file_link(text: &str) -> Option<(String, &str)> {
    let line = text.split('\n').next()?;
    let (label, linked) = line.split_once("](file://")?;
    linked.match_indices(')').find_map(|(end, _)| {
        let path = percent_decoded(linked.get(..end)?)?;
        let path = match path.as_bytes() {
            [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => path.get(1..)?.to_owned(),
            _ => path,
        };
        let name = path.trim_end_matches('/').rsplit('/').next()?;
        let after = label.len() + "](file://".len() + end + 1;
        (name == label).then_some((path.clone(), text.get(after..)?))
    })
}

fn percent_decoded(uri: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(uri.len());
    let mut rest = uri;
    while let Some((before, after)) = rest.split_once('%') {
        bytes.extend_from_slice(before.as_bytes());
        let (hex, after) = after.split_at_checked(2)?;
        bytes.push(u8::from_str_radix(hex, 16).ok()?);
        rest = after;
    }
    bytes.extend_from_slice(rest.as_bytes());
    String::from_utf8(bytes).ok()
}

/// The number of the `[Image #n]` that ends `text`.
fn image_number(text: &str) -> Option<usize> {
    let (_, number) = text.trim_end().strip_suffix(']')?.rsplit_once("[Image #")?;
    number.parse().ok()
}

/// An answer's typed text and the files listed under the `Attached:` the core wrote after it,
/// with the `[Image #n]` tags that stood for those files taken out of the text.
fn split_attached(answer: &str) -> (String, Vec<String>) {
    const HEAD: &str = "Attached:\n";
    let split = match answer.strip_prefix(HEAD) {
        Some(listed) => Some(("", listed)),
        None => (answer.find(&format!("\n\n{HEAD}")))
            .and_then(|at| answer.split_at_checked(at + 2))
            .and_then(|(before, rest)| Some((before, rest.strip_prefix(HEAD)?))),
    };
    let Some((before, listed)) = split else {
        return (answer.to_owned(), Vec::new());
    };
    let end = listed.find("\n\n").unwrap_or(listed.len());
    let (list, after) = listed.split_at_checked(end).unwrap_or((listed, ""));
    let paths: Vec<String> = (list.lines())
        .filter_map(|line| line.strip_prefix("- "))
        .map(str::to_owned)
        .collect();
    let text = [before, after].concat();
    (without_image_tags(&text, paths.len()), paths)
}

/// `text` without the `[Image #n]` tags that stood for its first `count` files.
pub(super) fn without_image_tags(text: &str, count: usize) -> String {
    let mut text = text.to_owned();
    for number in 1..=count {
        text = text.replace(&format!("[Image #{number}]"), "");
    }
    text.trim().to_owned()
}

/// The context items a message carries in its text, read back out of it: the
/// `<context>` blocks the app sends, and the headed fences an agent that
/// cannot take context embedded was sent instead.
pub(super) fn split_sent_context(text: &str) -> (String, Vec<SentContext>) {
    let mut context = Vec::new();
    let mut rest = String::new();
    let mut remaining = text;
    const OPEN: &str = "<context ref=\"";
    while let Some((before, after_open)) = remaining.split_once(OPEN) {
        let parsed = after_open.split_once("\">\n").and_then(|(uri, after_uri)| {
            let (body, after_body) = after_uri.split_once("\n</context>")?;
            (!uri.is_empty() && !uri.contains('"')).then_some((uri, body, after_body))
        });
        let Some((uri, body, after_body)) = parsed else {
            rest.push_str(before);
            rest.push_str(OPEN);
            remaining = after_open;
            continue;
        };
        rest.push_str(before.strip_suffix('\n').unwrap_or(before));
        context.push(
            SentContext::new(
                uri.to_owned(),
                body.split('\n').next().unwrap_or_default().to_owned(),
            )
            .with_text(body),
        );
        remaining = after_body;
    }
    rest.push_str(remaining);
    for item in &context {
        rest = rest.replacen(&item.uri, "", 1);
    }
    let rest = headed_fences(&rest, &mut context);
    if context.is_empty() {
        return (text.to_owned(), context);
    }
    (rest.trim().to_owned(), context)
}

/// `### title`, a line with the uri, a blank line, then a fence of three or
/// more backticks closed by the same fence, at the start or after a blank line.
fn headed_fences(text: &str, context: &mut Vec<SentContext>) -> String {
    let mut out = String::new();
    let mut rest = text;
    let mut at_start = true;
    while !rest.is_empty() {
        let starts = if at_start && rest.starts_with("### ") {
            Some((0, 0))
        } else {
            (rest.find("\n\n### ")).map(|cut| (cut, cut + 2))
        };
        at_start = false;
        let Some((cut, head)) = starts else {
            break;
        };
        match headed_fence(rest.get(head..).unwrap_or_default()) {
            Some((sent, len)) => {
                out.push_str(rest.get(..cut).unwrap_or_default());
                context.push(sent);
                rest = rest.get(head + len..).unwrap_or_default();
            }
            None => {
                let (kept, after) = rest
                    .split_at_checked(head.max(cut + 1))
                    .unwrap_or((rest, ""));
                out.push_str(kept);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn headed_fence(text: &str) -> Option<(SentContext, usize)> {
    let rest = text.strip_prefix("### ")?;
    let (title, rest) = rest.split_once('\n')?;
    let (uri, rest) = rest.split_once('\n')?;
    if title.is_empty() || uri.is_empty() || uri.contains(char::is_whitespace) {
        return None;
    }
    let rest = rest.strip_prefix('\n')?;
    let ticks = rest.chars().take_while(|c| *c == '`').count();
    if ticks < 3 || rest.as_bytes().get(ticks) != Some(&b'\n') {
        return None;
    }
    let (fence, after_fence) = rest.split_at_checked(ticks)?;
    let body = after_fence.get(1..)?;
    let close = format!("\n{fence}");
    let mut search = 0;
    while let Some(found) = body.get(search..)?.find(&close) {
        let end = search + found + close.len();
        let after = body.get(end..)?;
        if after.is_empty() || after.starts_with("\n\n###") {
            let consumed = text.len() - after.len();
            let sent = SentContext::new(uri.to_owned(), title.to_owned())
                .with_text(body.get(..search + found)?);
            return Some((sent, consumed));
        }
        search = search + found + 1;
    }
    None
}

pub(super) fn is_mention(uri: &str) -> bool {
    uri.starts_with(AGENT_URI)
}

/// What a draft wrote into a text, as far as the text itself says.
#[derive(Default)]
pub struct SentNames<'a> {
    pub attachments: &'a [String],
    /// Which of `attachments` are folders.
    pub folders: &'a [String],
    pub context: &'a [SentContext],
    /// Agents the text may name by `@name`, as `(name, id)`, when no context item says so.
    pub agents: &'a [(String, String)],
    /// Commands the text may name by `/name`.
    pub commands: &'a [String],
}

/// `text` read in order, its words and the chips its draft wrote into it; empty when it holds
/// no chip. The core reads this once, so each client draws the same chips, whether or not the
/// agent a chip names is still open under that name.
pub fn sent_spans(text: &str, names: SentNames<'_>) -> Vec<SentSpan> {
    let found = sent_chips(text, &names);
    let mut spans = Vec::new();
    let mut rest = text;
    loop {
        let next = (found.iter())
            .filter_map(|(written, chip)| Some((rest.find(written.as_str())?, written, chip)))
            .min_by_key(|(at, written, _)| (*at, std::cmp::Reverse(written.len())));
        let Some((at, written, chip)) = next else {
            break;
        };
        let Some((before, from)) = rest.split_at_checked(at) else {
            break;
        };
        if !before.is_empty() {
            spans.push(SentSpan {
                text: before.to_owned(),
                chip: None,
            });
        }
        spans.push(SentSpan {
            text: written.clone(),
            chip: Some(chip.clone()),
        });
        rest = from.get(written.len()..).unwrap_or_default();
    }
    if spans.is_empty() {
        return spans;
    }
    if !rest.is_empty() {
        spans.push(SentSpan {
            text: rest.to_owned(),
            chip: None,
        });
    }
    spans
}

fn sent_chips(text: &str, names: &SentNames<'_>) -> Vec<(String, SentChip)> {
    let chip = |kind, label: &str, carried: Option<&str>| SentChip {
        kind,
        label: label.to_owned(),
        carried: carried.map(str::to_owned),
    };
    let mut found: Vec<(String, SentChip)> = Vec::new();
    for (name, id) in names.agents {
        let written = format!("@{name}");
        let carried = format!("{AGENT_URI}{id}");
        let agent = chip(SentChipKind::Agent, &written, Some(&carried));
        found.push((written, agent));
    }
    for name in names.commands {
        let written = format!("/{name}");
        let command = chip(SentChipKind::Command, &written, None);
        found.push((written, command));
    }
    for (index, path) in names.attachments.iter().enumerate() {
        if is_image_path(path) {
            let number = index + 1;
            let label = format!("Image #{number}");
            let tag = format!("[Image #{number}]");
            if text.contains(&tag) {
                found.push((tag, chip(SentChipKind::Image, &label, Some(path))));
                continue;
            }
        }
        let parts: Vec<&str> = path
            .split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .collect();
        let tag = (0..parts.len())
            .filter_map(|from| parts.get(from..))
            .map(|tail| format!("+{}", tail.join("/")))
            .find(|tag| text.contains(tag.as_str()));
        if let Some(tag) = tag {
            let kind = match names.folders.contains(path) {
                true => SentChipKind::Folder,
                false => SentChipKind::File,
            };
            let file = chip(kind, &tag, Some(path));
            found.push((tag, file));
        }
    }
    for item in names.context {
        if item.uri.starts_with(AGENT_URI) || item.uri.starts_with(WOKEN_URI) {
            let written = item.title.split(' ').next().unwrap_or_default();
            if written.starts_with('@') {
                let agent = chip(SentChipKind::Agent, written, Some(&item.uri));
                found.push((written.to_owned(), agent));
            }
        } else if item.uri.starts_with(EARLIER_URI) && item.title.starts_with('@') {
            let earlier = chip(SentChipKind::Chat, &item.title, Some(&item.uri));
            found.push((item.title.clone(), earlier));
        } else if let Some(tracked) = &item.tracked {
            let (sign, kind) = match tracked.kind {
                TrackedKind::Pull => ('!', SentChipKind::Pull),
                TrackedKind::Issue => ('#', SentChipKind::Issue),
            };
            let number = &tracked.number;
            let label = format!("#{number} {}", tracked_title(&item.title, number));
            let named = chip(kind, &label, Some(&item.uri));
            found.push((format!("{sign}{number}"), named));
        }
    }
    found.retain(|(written, _)| text.contains(written.as_str()));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_resumed_answer_whose_image_follows_it_as_a_link() {
        let saved = "<question-answer asker=\"Zouken\" asker-id=\"agent-1\" cli=\"claude\" question=\"Keep it?\" note=\"\">\nKeep the App line\n</question-answer>[@shot.png](file:///Users/me/shot.png)";
        let (lettered, linked) = split_file_links(saved);
        let letters = agent_letters(&lettered).expect("an answer");
        assert_eq!(letters[0].text, "Keep the App line");
        assert_eq!(linked, ["/Users/me/shot.png"]);
    }
}
