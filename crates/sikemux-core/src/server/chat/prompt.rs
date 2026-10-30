//! A prompt checked and put into the content blocks agents take, with the
//! person's attachments and context.

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ResourceLink, TextResourceContents,
};
use url::Url;

use crate::protocol::ChatContext;

const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ATTACHMENTS: usize = 32;
const MAX_CONTEXT_ITEMS: usize = 16;

pub fn bounded_text(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max || value.contains(['\0', '\r', '\n']) {
        return Err(format!("{name} must be bounded non-blank text"));
    }
    Ok(())
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

/// The message cut after each `[Image #N]` it writes, so attachment N can
/// follow the words it was put between, and the text after the last of them.
fn split_at_images(text: &str, attachments: usize) -> (Vec<(&str, usize)>, &str) {
    const OPEN: &str = "[Image #";
    let mut pieces = Vec::new();
    let mut placed = vec![false; attachments];
    let mut from = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find(OPEN) {
        let digits = search + found + OPEN.len();
        let close = digits
            + text[digits..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
        search = digits;
        if !text[close..].starts_with(']') {
            continue;
        }
        let Some(index) = text[digits..close]
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .filter(|&index| index < attachments && !placed[index])
        else {
            continue;
        };
        placed[index] = true;
        pieces.push((&text[from..=close], index));
        from = close + 1;
        search = from;
    }
    (pieces, &text[from..])
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
    if context.len() > MAX_CONTEXT_ITEMS {
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
    let (pieces, rest) = split_at_images(&text, links.len());
    let mut rest = rest.to_owned();
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
        blocks.push(said.to_owned().into());
        blocks.extend(links[index].take());
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
            vec!["/tmp/a.txt".into()],
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
