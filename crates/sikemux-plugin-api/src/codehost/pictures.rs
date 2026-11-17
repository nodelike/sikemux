//! A picture or recording read from disk for a code host to keep, as each host takes it.

use std::path::Path;

use base64::Engine;

use super::asks::PictureFile;

/// The largest file a host is handed for a description.
pub const MOST_BYTES: u64 = 10 << 20;

pub struct Picture {
    pub name: String,
    pub kind: &'static str,
    pub bytes: Vec<u8>,
}

fn kind_of(name: &str) -> Option<&'static str> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && !name.starts_with('.')
        && name
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || "-_.".contains(letter))
}

impl PictureFile {
    /// The file, refused unless it is a picture or recording of at most [`MOST_BYTES`].
    pub async fn read(&self) -> Result<Picture, String> {
        if !plain_name(&self.name) {
            return Err(format!("`{}` is not a plain file name", self.name));
        }
        let kind = kind_of(&self.name)
            .filter(|kind| Some(*kind) == kind_of(&self.path))
            .ok_or("only a png, jpeg, gif, webp, mp4, webm or mov file can be kept")?;
        let path = self.path.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            let size = std::fs::metadata(&path)
                .map_err(|error| error.to_string())?
                .len();
            if size > MOST_BYTES {
                return Err(format!(
                    "the file is {} MiB, more than the {} MiB a description takes",
                    size >> 20,
                    MOST_BYTES >> 20
                ));
            }
            std::fs::read(&path).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        Ok(Picture {
            name: self.name.clone(),
            kind,
            bytes,
        })
    }
}

impl Picture {
    pub fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }

    /// The picture as the one file of a form under `field`, with the content type to send it as.
    pub fn form(&self, field: &str) -> (String, Vec<u8>) {
        let boundary = format!(
            "sikemux-{:016x}",
            self.bytes.len() as u64 ^ 0x5eed_f00d_cafe
        );
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            self.name, self.kind
        )
        .into_bytes();
        body.extend_from_slice(&self.bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }
}
