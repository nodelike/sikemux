//! Context a message tells its agent alone, as it sits in the agent's own record of that message.

use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};

const CONTEXT_OPEN: &str = "<context ref=\"";
const CONTEXT_BODY: &str = "\">\n";
const CONTEXT_CLOSE: &str = "\n</context>";

/// Handed-over text, the way the agents' own clients write it after a message.
pub(crate) fn context_text(uri: &str, text: &str) -> String {
    format!("{CONTEXT_OPEN}{uri}{CONTEXT_BODY}{text}{CONTEXT_CLOSE}")
}

/// Sikemux's own block for the agent alone, named `name`. A line in `text` that would close the
/// block early is kept as text.
pub(crate) fn sikemux_context(name: &str, title: &str, text: &str) -> ChatContext {
    ChatContext {
        uri: format!("{FOR_AGENT_URI}{name}"),
        title: title.to_owned(),
        text: text.replace("</context", "&lt;/context"),
    }
}

/// What the person wrote of `text`, from an agent's own record of their message.
pub(crate) fn person_wrote(text: &str) -> String {
    without_for_agent(text, &mut Vec::new())
}

/// `text` without the context meant for the agent alone, which goes in `taken`, or the links to it.
pub(crate) fn without_for_agent(text: &str, taken: &mut Vec<ChatContext>) -> String {
    let opening = format!("{CONTEXT_OPEN}{FOR_AGENT_URI}");
    let mut left = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(&opening) {
        let inner = &rest[at + CONTEXT_OPEN.len()..];
        let Some((uri, body, after)) = inner.split_once(CONTEXT_BODY).and_then(|(uri, body)| {
            let (body, after) = body.split_once(CONTEXT_CLOSE)?;
            Some((uri, body, after))
        }) else {
            break;
        };
        left.push_str(&rest[..at]);
        taken.push(ChatContext {
            uri: uri.to_owned(),
            title: uri.to_owned(),
            text: body.to_owned(),
        });
        rest = after;
    }
    left.push_str(rest);
    let mut kept = String::new();
    let mut rest = left.as_str();
    while let Some(at) = rest.find(FOR_AGENT_URI) {
        kept.push_str(&rest[..at]);
        let link = &rest[at..];
        rest = &link[link.find(char::is_whitespace).unwrap_or(link.len())..];
    }
    kept.push_str(rest);
    kept.trim_end().to_owned()
}
