//! A prompt checked and put into the content blocks agents take, with the
//! person's attachments and context.

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ResourceLink, TextResourceContents,
};
use serde_json::Value;
use sikemux_fold::{sent_spans, SentNames};
use sikemux_view::chat::SentChipKind;
use url::Url;

use crate::sessions::for_agent::without_for_agent;
use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};

const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;
pub use sikemux_calls::chat::MAX_ATTACHMENTS;
pub const MAX_CONTEXT_ITEMS: usize = 16;

/// Whether a prompt is a slash command rather than words for the model. One
/// only works as the whole prompt.
pub fn is_command(text: &str) -> bool {
    text.trim_start().starts_with('/')
}

pub(crate) fn command_parts(text: &str) -> Option<(&str, &str)> {
    let text = text.trim().strip_prefix('/')?.trim_start();
    let name = text.split_whitespace().next()?;
    let args = text.strip_prefix(name)?.trim();
    Some((name, args))
}

pub fn bounded_text(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max || value.contains(['\0', '\r', '\n']) {
        return Err(format!("{name} must be bounded non-blank text"));
    }
    Ok(())
}

/// Whether `paths` may go with the person's words to an agent, as a prompt's attachments may.
pub fn check_attachments(paths: &[String]) -> Result<(), String> {
    if paths.len() > MAX_ATTACHMENTS {
        return Err(format!(
            "at most {MAX_ATTACHMENTS} files go with one answer"
        ));
    }
    paths
        .iter()
        .try_for_each(|path| resource_link(path).map(drop))
}

fn resource_link(path: &str) -> Result<ContentBlock, String> {
    bounded_text("attachment path", path, 4_096)?;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("attachment paths must be absolute".into());
    }
    let uri = Url::from_file_path(&path)
        .map_err(|_| "attachment path cannot be represented as a file URL")?
        .to_string();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment")
        .to_owned();
    Ok(ContentBlock::ResourceLink(ResourceLink::new(name, uri)))
}

/// A context item written into the message itself, for an agent that cannot
/// take it as an embedded resource. The fence outgrows any run of backticks
/// in the text so the text cannot close it early.
fn context_section(item: &ChatContext) -> String {
    let longest = item
        .text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!(
        "### {}\n{}\n\n{fence}\n{}\n{fence}",
        item.title, item.uri, item.text
    )
}

fn context_resource(item: ChatContext) -> ContentBlock {
    ContentBlock::Resource(EmbeddedResource::new(
        EmbeddedResourceResource::TextResourceContents(
            TextResourceContents::new(item.text, item.uri).mime_type("text/markdown".to_string()),
        ),
    ))
}

/// The message cut after each chip that names one of `paths`, so the file can follow the words
/// it was put between, and the text after the last of them. A command keeps its words together.
fn split_at_attachments(text: &str, paths: &[String]) -> (Vec<(String, usize)>, String) {
    let command = is_command(text);
    let mut placed = vec![false; paths.len()];
    let mut pieces = Vec::new();
    let mut said = String::new();
    let names = SentNames {
        attachments: paths,
        ..SentNames::default()
    };
    let spans = sent_spans(text, names);
    if spans.is_empty() {
        said.push_str(text);
    }
    for span in spans {
        said.push_str(&span.text);
        let Some(chip) = span.chip else {
            continue;
        };
        if command && chip.kind != SentChipKind::Image {
            continue;
        }
        let index = (chip.carried).and_then(|carried| {
            (paths.iter().zip(&placed))
                .position(|(path, placed)| !placed && path.as_str() == &*carried)
        });
        if let Some((index, slot)) = index.and_then(|at| Some((at, placed.get_mut(at)?))) {
            *slot = true;
            pieces.push((std::mem::take(&mut said), index));
        }
    }
    (pieces, said)
}

/// A user message an ACP or Pi agent tells back, as when it loads a saved session, without what
/// Sikemux told the agent alone. Answers false when nothing the person wrote is left.
pub(crate) fn drop_for_agent(update: &mut Value) -> bool {
    if update["sessionUpdate"] != "user_message_chunk" {
        return true;
    }
    let content = &mut update["content"];
    let uri = content
        .get("resource")
        .and_then(|resource| resource.get("uri"));
    if uri
        .and_then(Value::as_str)
        .is_some_and(|uri| uri.starts_with(FOR_AGENT_URI))
    {
        return false;
    }
    match content["text"].as_str() {
        Some(text) if text.contains(FOR_AGENT_URI) => {
            let kept = without_for_agent(text, &mut Vec::new());
            let any = !kept.trim().is_empty();
            content["text"] = kept.into();
            any
        }
        _ => true,
    }
}

/// Takes what a message told its agent alone out of the agent's own record of that message,
/// which holds it beside what the person wrote, for a chat read back from that record.
pub(crate) fn take_for_agent(content: &mut Value) -> Vec<ChatContext> {
    let mut taken = Vec::new();
    match content {
        Value::String(text) => *text = without_for_agent(text, &mut taken),
        Value::Array(blocks) => blocks.retain_mut(|block| {
            let Some(text) = block["text"].as_str() else {
                return true;
            };
            if !text.contains(FOR_AGENT_URI) {
                return true;
            }
            let kept = without_for_agent(text, &mut taken);
            let any = !kept.is_empty();
            block["text"] = kept.into();
            any
        }),
        _ => {}
    }
    taken
}

pub fn prompt_blocks(
    text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
    embedded_context: bool,
) -> Result<Vec<ContentBlock>, String> {
    if paths.len() > MAX_ATTACHMENTS {
        return Err(format!(
            "a prompt can include at most {MAX_ATTACHMENTS} attachments"
        ));
    }
    if context.iter().filter(|item| !item.for_agent_only()).count() > MAX_CONTEXT_ITEMS {
        return Err(format!(
            "a prompt can include at most {MAX_CONTEXT_ITEMS} context items"
        ));
    }
    for item in &context {
        bounded_text("context title", &item.title, 1_024)?;
        bounded_text("context uri", &item.uri, 4_096)?;
    }
    let context_bytes: usize = context.iter().map(|item| item.text.len()).sum();
    if text.len() + context_bytes > MAX_PROMPT_BYTES {
        return Err("prompt is too large".into());
    }
    let mut links = paths
        .iter()
        .map(|path| resource_link(path).map(Some))
        .collect::<Result<Vec<_>, _>>()?;
    let (pieces, mut rest) = split_at_attachments(&text, &paths);
    let mut resources = Vec::new();
    if embedded_context {
        resources.extend(context.into_iter().map(context_resource));
    } else {
        for item in &context {
            if !rest.trim().is_empty() || !pieces.is_empty() {
                rest.push_str("\n\n");
            }
            rest.push_str(&context_section(item));
        }
    }
    let mut blocks = Vec::with_capacity(paths.len() + resources.len() + pieces.len() + 1);
    for (said, index) in pieces {
        blocks.push(said.into());
        blocks.extend(links.get_mut(index).and_then(Option::take));
    }
    if !rest.trim().is_empty() {
        blocks.push(rest.into());
    }
    blocks.extend(resources);
    blocks.extend(links.into_iter().flatten());
    if blocks.is_empty() {
        return Err("prompt is empty".into());
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    const ATTACHED: &str = "/tmp/a.txt";
    #[cfg(windows)]
    const ATTACHED: &str = r"C:\tmp\a.txt";

    #[test]
    fn prompt_rejects_relative_attachment_paths() {
        let error = prompt_blocks(String::new(), vec!["relative.txt".into()], Vec::new(), true)
            .unwrap_err();
        assert_eq!(error, "attachment paths must be absolute");
    }

    fn issue_context() -> ChatContext {
        ChatContext {
            uri: "https://github.com/o/r/issues/12".into(),
            title: "#12 Login crashes".into(),
            text: "Issue #12: Login crashes\n\nIt crashes.".into(),
        }
    }

    #[test]
    fn prompt_embeds_context_when_the_agent_takes_it() {
        let blocks = prompt_blocks(
            "fix this".into(),
            vec![ATTACHED.into()],
            vec![issue_context()],
            true,
        )
        .unwrap();
        let blocks = serde_json::to_value(blocks).unwrap();
        assert_eq!(blocks[0], json!({ "type": "text", "text": "fix this" }));
        assert_eq!(
            blocks[1],
            json!({
                "type": "resource",
                "resource": {
                    "uri": "https://github.com/o/r/issues/12",
                    "mimeType": "text/markdown",
                    "text": "Issue #12: Login crashes\n\nIt crashes.",
                },
            })
        );
        assert_eq!(blocks[2]["type"], "resource_link");
    }

    #[test]
    fn prompt_writes_context_into_the_text_otherwise() {
        let blocks =
            prompt_blocks("fix this".into(), Vec::new(), vec![issue_context()], false).unwrap();
        let blocks = serde_json::to_value(blocks).unwrap();
        assert_eq!(blocks.as_array().unwrap().len(), 1);
        assert_eq!(
            blocks[0]["text"],
            "fix this\n\n### #12 Login crashes\nhttps://github.com/o/r/issues/12\n\n```\nIssue #12: Login crashes\n\nIt crashes.\n```"
        );
    }

    #[test]
    fn prompt_of_context_alone_is_not_empty() {
        let blocks =
            prompt_blocks(String::new(), Vec::new(), vec![issue_context()], false).unwrap();
        assert!(serde_json::to_value(blocks).unwrap()[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("### #12"));
        assert!(prompt_blocks(String::new(), Vec::new(), vec![issue_context()], true).is_ok());
    }

    #[test]
    fn context_fence_outgrows_backticks_in_the_text() {
        let item = ChatContext {
            text: "```rust\nfn main() {}\n```".into(),
            ..issue_context()
        };
        let section = context_section(&item);
        assert!(section.contains("\n````\n```rust"));
        assert!(section.ends_with("\n```\n````"));
    }
}
