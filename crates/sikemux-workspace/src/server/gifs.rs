//! GIFs an agent asks for in its reply, found on KLIPY.

use crate::server::workspace_state::WorkspaceParts;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::Core;

const KLIPY: &str = "https://api.klipy.com/api/v1";
const KLIPY_KEY: Option<&str> = option_env!("KLIPY_API_KEY");
const TIMEOUT: Duration = Duration::from_secs(10);
/// The fewest results KLIPY returns; the best match among them that has a GIF to show is sent.
const RESULTS: &str = "8";
/// The medium GIF is sharp at the size a chat draws it; one larger than this falls back to the
/// small one, since every frame of it is held decoded while it shows.
const MAX_GIF_BYTES: u64 = 6 * 1024 * 1024;
/// KLIPY's best match for `query`, as a chat's GIF part holds it.
pub async fn find(core: &Core, query: &str) -> Result<Value, String> {
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
                let file = item.get("file")?.get(*size)?.get("gif")?.as_object()?;
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
        .ok_or_else(|| format!("No GIF for “{query}”"))?;
    Ok(json!({
        "id": item["slug"].as_str().unwrap_or_default(),
        "url": file["url"],
        "width": file.get("width").and_then(Value::as_u64).unwrap_or(0),
        "height": file.get("height").and_then(Value::as_u64).unwrap_or(0),
        "title": item["title"].as_str().unwrap_or_default(),
    }))
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
    (answer.pointer("/data/data").and_then(Value::as_array))
        .into_iter()
        .flatten()
}

/// The same person to the GIF service across searches, without saying who.
fn customer(core: &Core) -> String {
    let place = core
        .data_dir()
        .as_ref()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut customer = hex::encode(Sha256::digest(format!("klipy:{place}")));
    customer.truncate(32);
    customer
}
