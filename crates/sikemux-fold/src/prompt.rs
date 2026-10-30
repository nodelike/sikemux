//! What a message sent to the agent holds, read the way the chat shows it:
//! letters from other agents, the context sent with it, and the notes an
//! agent's harness writes to itself, which the person never sees.

use sikemux_wire::chat::{AgentLetter, LetterKind, SentContext};

/// Where an `@` mention of another agent points; the message's own text
/// already shows the mention.
const MENTION_URI: &str = "sikemux://agent/";

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

pub(super) fn is_agent_letter(text: &str) -> bool {
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
    Some(&rest[..end])
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
        let Some(end) = rest.find(&close) else {
            return false;
        };
        rest = rest[end + close.len()..].trim_start();
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
        && trimmed[trimmed.len() - close.len()..].eq_ignore_ascii_case(&close)
        && trimmed[1 + name.len()..].contains('>')
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
    while let Some(eq) = rest.find("=\"") {
        let key_start = rest[..eq]
            .rfind(|c: char| !(c.is_ascii_lowercase() || c == '-'))
            .map_or(0, |at| at + 1);
        let key = &rest[key_start..eq];
        let value_start = eq + 2;
        let Some(len) = rest[value_start..].find('"') else {
            break;
        };
        if !key.is_empty() {
            found.push((key, unescaped(&rest[value_start..value_start + len])));
        }
        rest = &rest[value_start + len + 1..];
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
        outside.push_str(&rest[..at]);
        let open_end = rest[at..].find(">\n").map(|end| at + end);
        let Some(open_end) = open_end.filter(|end| !rest[at..*end].contains('>')) else {
            outside.push_str(&rest[at..]);
            break;
        };
        let source = &rest[at + tag.len() + 2..open_end];
        let body_start = open_end + 2;
        let close = format!("\n</{tag}>");
        let Some(body_len) = rest[body_start..].find(&close) else {
            outside.push_str(&rest[at..]);
            break;
        };
        let body = rest[body_start..body_start + body_len]
            .replace(&format!("&lt;/{tag}"), &format!("</{tag}"));
        let found = attributes(source);
        let attribute = |key: &str| {
            found
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        letters.push(if tag == "question-answer" {
            AgentLetter {
                kind: LetterKind::Answer,
                name: attribute("asker"),
                agent_id: attribute("asker-id"),
                cli: attribute("cli"),
                question: attribute("question"),
                text: body,
                secret: attribute("secret") == "true",
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
                name: attribute("from"),
                agent_id,
                cli: attribute("cli"),
                question: String::new(),
                tree: Some(super::send::read(&text)),
                text,
                secret: false,
                trigger,
            }
        });
        rest = &rest[body_start + body_len + close.len()..];
    }
    (!letters.is_empty() && outside.trim().is_empty()).then_some(letters)
}

/// The context items a message carries in its text, read back out of it: the
/// `<context>` blocks the app sends, and the headed fences an agent that
/// cannot take context embedded was sent instead.
pub(super) fn split_sent_context(text: &str) -> (String, Vec<SentContext>) {
    let mut context = Vec::new();
    let mut rest = String::new();
    let mut remaining = text;
    const OPEN: &str = "<context ref=\"";
    while let Some(at) = remaining.find(OPEN) {
        let uri_start = at + OPEN.len();
        let parsed = remaining[uri_start..].find("\">\n").and_then(|uri_len| {
            let uri = &remaining[uri_start..uri_start + uri_len];
            let body_start = uri_start + uri_len + 3;
            let body_len = remaining[body_start..].find("\n</context>")?;
            (!uri.is_empty() && !uri.contains('"')).then_some((uri, body_start, body_len))
        });
        let Some((uri, body_start, body_len)) = parsed else {
            rest.push_str(&remaining[..uri_start]);
            remaining = &remaining[uri_start..];
            continue;
        };
        let before = &remaining[..at];
        rest.push_str(before.strip_suffix('\n').unwrap_or(before));
        let body = &remaining[body_start..body_start + body_len];
        context.push(SentContext {
            uri: uri.to_owned(),
            title: body.split('\n').next().unwrap_or_default().to_owned(),
        });
        remaining = &remaining[body_start + body_len + "\n</context>".len()..];
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
    let mut at = 0;
    while at < text.len() {
        let starts = if at == 0 && text.starts_with("### ") {
            Some((0, 0))
        } else {
            text[at..]
                .find("\n\n### ")
                .map(|offset| (at + offset, at + offset + 2))
        };
        let Some((cut, head)) = starts else {
            break;
        };
        match headed_fence(&text[head..]) {
            Some((uri, title, len)) => {
                out.push_str(&text[at..cut]);
                context.push(SentContext { uri, title });
                at = head + len;
            }
            None => {
                out.push_str(&text[at..head.max(cut + 1)]);
                at = head.max(cut + 1);
            }
        }
    }
    out.push_str(&text[at.min(text.len())..]);
    out
}

fn headed_fence(text: &str) -> Option<(String, String, usize)> {
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
    let fence = &rest[..ticks];
    let body = &rest[ticks + 1..];
    let close = format!("\n{fence}");
    let mut search = 0;
    while let Some(found) = body[search..].find(&close) {
        let end = search + found + close.len();
        let after = &body[end..];
        if after.is_empty() || after.starts_with("\n\n###") {
            let consumed = text.len() - after.len();
            return Some((uri.to_owned(), title.to_owned(), consumed));
        }
        search = search + found + 1;
    }
    None
}

pub(super) fn is_mention(uri: &str) -> bool {
    uri.starts_with(MENTION_URI)
}
