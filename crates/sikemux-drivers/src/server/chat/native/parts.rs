//! A prompt in pieces, for the drivers that write it in their own agent's
//! shape rather than as ACP content blocks.

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{ContentBlock, EmbeddedResourceResource};
use url::Url;

use super::session::Prompt;

/// The file extensions of pictures an agent can be handed.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

pub enum Part {
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
pub fn parts(prompt: &Prompt) -> Result<Vec<Part>, String> {
    let blocks = crate::server::chat::prompt::prompt_blocks(
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

/// The picture at `uri` as its type, read from its first bytes, and its base64,
/// when it is one and holds at most `max_bytes`.
pub fn inline_picture(uri: &str, max_bytes: u64) -> Option<(&'static str, String)> {
    use base64::Engine as _;
    let path = picture(uri)?;
    if std::fs::metadata(&path).ok()?.len() > max_bytes {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let media_type = match bytes.as_slice() {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => return None,
    };
    Some((
        media_type,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

/// The file `uri` names, when it is a picture.
pub fn picture(uri: &str) -> Option<PathBuf> {
    let path = Url::parse(uri).ok()?.to_file_path().ok()?;
    let extension = path.extension()?.to_str()?.to_lowercase();
    IMAGE_EXTENSIONS
        .contains(&extension.as_str())
        .then_some(path)
}
