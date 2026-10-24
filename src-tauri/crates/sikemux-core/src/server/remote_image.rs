//! Pictures a chat's markdown names by URL, fetched by the core for any client:
//! only from the public internet, only images, and kept a while so a long chat
//! scrolling past one picture fetches it once.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use url::Url;

use super::{CoreError, CoreResult};
use crate::http::{fetch_bounded, names_private_address, public_client, sniff};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
/// What the kept pictures may add up to, as their base64 text.
const KEPT_BYTES: usize = 48 * 1024 * 1024;
/// How long a picture that would not load is not asked for again.
const FAILED_FOR: Duration = Duration::from_secs(60);
const COULD_NOT: &str = "Couldn't load the picture";

#[derive(Deserialize)]
pub(crate) struct ImageParams {
    url: String,
}

#[derive(Clone)]
struct Image {
    mime: &'static str,
    /// The picture's bytes in base64.
    data: Arc<str>,
}

impl Image {
    fn answer(&self) -> Value {
        json!({ "mime": self.mime, "data": &*self.data })
    }
}

enum Kept {
    Loaded(Image),
    Failed(Instant),
}

/// The pictures fetched lately, the most recently used last.
#[derive(Default)]
pub(crate) struct RemoteImages {
    kept: Mutex<VecDeque<(String, Kept)>>,
}

impl RemoteImages {
    /// The kept answer for `url`, moved to the back as just used; a failure
    /// is forgotten once `FAILED_FOR` has passed.
    fn get(&self, url: &str) -> Option<CoreResult<Image>> {
        let mut kept = self.kept.locked();
        let place = kept.iter().position(|(kept_url, _)| kept_url == url)?;
        let entry = kept.remove(place)?;
        let answer = match &entry.1 {
            Kept::Loaded(image) => Ok(image.clone()),
            Kept::Failed(at) if at.elapsed() < FAILED_FOR => Err(CoreError::from(COULD_NOT)),
            Kept::Failed(_) => return None,
        };
        kept.push_back(entry);
        Some(answer)
    }

    fn keep(&self, url: String, entry: Kept) {
        let mut kept = self.kept.locked();
        kept.retain(|(kept_url, _)| *kept_url != url);
        kept.push_back((url, entry));
        let size = |entry: &Kept| match entry {
            Kept::Loaded(image) => image.data.len(),
            Kept::Failed(_) => 0,
        };
        let mut total: usize = kept.iter().map(|(_, entry)| size(entry)).sum();
        while total > KEPT_BYTES && kept.len() > 1 {
            if let Some((_, oldest)) = kept.pop_front() {
                total -= size(&oldest);
            }
        }
    }
}

/// What picture `bytes` hold, judged by the bytes alone: one `sniff` knows, or
/// an SVG, which README badges are.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    sniff(bytes)
        .filter(|mime| mime.starts_with("image/"))
        .or_else(|| is_svg(bytes).then_some("image/svg+xml"))
}

/// Whether the document's root is `<svg`, after any byte-order mark, white
/// space, XML declaration, comments and doctype.
fn is_svg(bytes: &[u8]) -> bool {
    let mut rest = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    loop {
        rest = rest.trim_ascii_start();
        let skipped = [(&b"<?"[..], &b"?>"[..]), (b"<!--", b"-->"), (b"<!", b">")]
            .into_iter()
            .find(|(open, _)| rest.starts_with(open))
            .and_then(|(open, close)| {
                let after = &rest[open.len()..];
                let end = after
                    .windows(close.len())
                    .position(|window| window == close)?;
                Some(&after[end + close.len()..])
            });
        match skipped {
            Some(after) => rest = after,
            None => break,
        }
    }
    rest.strip_prefix(b"<svg").is_some_and(|after| {
        matches!(
            after.first(),
            Some(b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/')
        )
    })
}

/// A web address the core may fetch: http or https, public, and carrying no
/// sign-in of its own.
fn allowed(url: &str) -> Option<Url> {
    let url = Url::parse(url).ok()?;
    let web = matches!(url.scheme(), "http" | "https");
    let signed = !url.username().is_empty() || url.password().is_some();
    (web && !signed && !names_private_address(&url)).then_some(url)
}

/// The picture at `url` as `{mime, data}`, its bytes in base64.
pub(crate) async fn fetch(images: &RemoteImages, params: ImageParams) -> CoreResult<Value> {
    if let Some(answer) = images.get(&params.url) {
        return answer.map(|image| image.answer());
    }
    let url = allowed(&params.url).ok_or_else(|| CoreError::from(COULD_NOT))?;
    let fetched = match public_client() {
        Ok(client) => fetch_bounded(client.get(url).timeout(TIMEOUT), MAX_BYTES).await,
        Err(_) => None,
    };
    let Some((mime, bytes)) = fetched.and_then(|bytes| Some((image_type(&bytes)?, bytes))) else {
        images.keep(params.url, Kept::Failed(Instant::now()));
        return Err(COULD_NOT.into());
    };
    let image = Image {
        mime,
        data: base64::engine::general_purpose::STANDARD
            .encode(bytes)
            .into(),
    };
    let answer = image.answer();
    images.keep(params.url, Kept::Loaded(image));
    Ok(answer)
}
