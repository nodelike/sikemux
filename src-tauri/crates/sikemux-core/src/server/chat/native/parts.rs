//! A prompt in pieces, for the drivers that write it in their own agent's
//! shape rather than as ACP content blocks.

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{ContentBlock, EmbeddedResourceResource};
use url::Url;

use super::session::Prompt;

/// The pictures an agent can be handed, by file extension.
const IMAGE_TYPES: [(&str, &str); 5] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

pub(super) enum Part {
    Text(String),
    /// A file the person attached.
    File {
        name: String,
        uri: String,
    },
    /// Text handed over with the message, such as an issue, and where it is from.
    Context {
        uri: String,
        text: String,
    },
}

/// `prompt` checked and split into its parts.
pub(super) fn parts(prompt: &Prompt) -> Result<Vec<Part>, String> {
    let blocks = crate::acp::prompt_blocks(
        prompt.text.clone(),
        prompt.paths.clone(),
        prompt.context.clone(),
        true,
    )?;
    Ok(blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(Part::Text(text.text)),
            ContentBlock::ResourceLink(link) => Some(Part::File {
                name: link.name,
                uri: link.uri,
            }),
            ContentBlock::Resource(resource) => match resource.resource {
                EmbeddedResourceResource::TextResourceContents(text) => Some(Part::Context {
                    uri: text.uri,
                    text: text.text,
                }),
                _ => None,
            },
            _ => None,
        })
        .collect())
}

/// The file `uri` names and its picture type, when it is a picture.
pub(super) fn picture(uri: &str) -> Option<(PathBuf, &'static str)> {
    let path = Url::parse(uri).ok()?.to_file_path().ok()?;
    let extension = path.extension()?.to_str()?.to_lowercase();
    let (_, media_type) = IMAGE_TYPES.iter().find(|(known, _)| *known == extension)?;
    Some((path, media_type))
}

/// Handed-over text, the way the agents' own clients write it after a message.
pub(super) fn context_text(uri: &str, text: &str) -> String {
    format!("<context ref=\"{uri}\">\n{text}\n</context>")
}
