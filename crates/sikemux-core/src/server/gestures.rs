//! Reactions and GIFs: what an agent does in a chat without a row of its own.
//! A chat reads what each call answered and shows only that, an emoji on the
//! message it answers or a GIF where it sent it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::Core;
use crate::cli::protocol::HarnessRequest;

/// What a chat agent with Sikemux's tools is told about its gestures, in its system prompt,
/// where it weighs them as part of who it is rather than as one tool among many.
pub(crate) const PERSONALITY: &str = "You have a personality in Sikemux: react to the person's messages with react, and send GIFs with gif wherever they fit, as many as the reply calls for, even between its parts. Never mention either.";

const KLIPY: &str = "https://api.klipy.com/api/v1";
const KLIPY_KEY: Option<&str> = option_env!("KLIPY_API_KEY");
const TIMEOUT: Duration = Duration::from_secs(10);
/// The fewest results KLIPY returns; the best match among them that has a GIF to show is sent.
const RESULTS: &str = "8";
/// The medium GIF is sharp at the size a chat draws it; one larger than this falls back to the
/// small one, since every frame of it is held decoded while it shows.
const MAX_GIF_BYTES: u64 = 6 * 1024 * 1024;
/// Joined emoji such as a family or a flag run to several characters; anything
/// longer is words.
const MAX_EMOJI_CHARS: usize = 12;

pub(super) fn react(request: &HarnessRequest) -> Result<Value, String> {
    let emoji = request
        .params
        .get("emoji")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !is_emoji(emoji) {
        return Err("emoji must be one emoji, such as 👍".into());
    }
    Ok(json!({
        "reacted": emoji,
        "next": "A GIF can go with it: call gif with a few words. Never mention either.",
    }))
}

fn is_emoji(text: &str) -> bool {
    !text.is_empty()
        && text.chars().count() <= MAX_EMOJI_CHARS
        && text.chars().any(|char| u32::from(char) >= 0xA9)
        && !text
            .chars()
            .any(|char| char.is_whitespace() || char.is_alphabetic())
}

/// Sends KLIPY's best match for `query` under the agent's reply.
pub(super) async fn gif(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let query = text(&request.params, "query")?;
    let answer = klipy(
        "gifs/search",
        &[
            ("q", query),
            ("per_page", RESULTS),
            ("customer_id", &customer(core)),
            ("content_filter", "medium"),
            ("format_filter", "gif"),
        ],
    )
    .await?;
    let (item, file) = items(&answer)
        .find_map(|item| {
            let file = ["md", "sm", "xs"].iter().find_map(|size| {
                let file = item["file"][size]["gif"].as_object()?;
                let shown = file
                    .get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|url| url.starts_with("https://"));
                let small = file
                    .get("size")
                    .and_then(Value::as_u64)
                    .is_some_and(|size| size <= MAX_GIF_BYTES);
                (shown && small).then_some(file)
            })?;
            Some((item, file))
        })
        .ok_or_else(|| format!("No GIF for “{query}”; try other words"))?;
    Ok(json!({
        "gif": {
            "id": item["slug"].as_str().unwrap_or_default(),
            "url": file["url"],
            "width": file.get("width").and_then(Value::as_u64).unwrap_or(0),
            "height": file.get("height").and_then(Value::as_u64).unwrap_or(0),
            "title": item["title"].as_str().unwrap_or_default(),
        },
        "shown": "The person sees the GIF here in your reply. Do not mention it.",
    }))
}

fn text<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} must be nonempty text"))
}

async fn klipy(path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let key = KLIPY_KEY
        .filter(|key| !key.is_empty())
        .ok_or("GIFs are not set up in this build of Sikemux")?;
    let client = crate::http::client().map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{KLIPY}/{key}/{path}"))
        .query(query)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|_| "Couldn't reach the GIF service".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "The GIF service answered {}",
            response.status().as_u16()
        ));
    }
    response
        .json()
        .await
        .map_err(|_| "The GIF service's answer couldn't be read".to_owned())
}

fn items(answer: &Value) -> impl Iterator<Item = &Value> {
    answer["data"]["data"].as_array().into_iter().flatten()
}

/// The same person to the GIF service across searches, without saying who.
fn customer(core: &Core) -> String {
    let place = core
        .harness
        .data_dir
        .as_ref()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default();
    hex::encode(Sha256::digest(format!("klipy:{place}")))[..32].to_owned()
}
