//! The account's Stash: what the person shared into Sikemux from another app, kept on the
//! accounts server for every device and host on the account. An item shared to one of this
//! host's chats lands in that chat's draft as soon as the host hears of it; any other waits
//! until the person attaches it from a composer.

use crate::server::changed::What;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::accounts::protocol::{
    AccountEvent, AccountEventType, StashItem, StashKind, StashLink, StashList, StashSeen,
    StashTaken, StashTarget, StashUpdate,
};

use super::account::api::{as_host, host_request, host_send, Upload};
use super::changed::Topic;
use super::connection::blocking;
use super::drafts::Delivery;
use super::{attachments, Core, CoreError, CoreResult};

mod deliver;
mod words;

pub(crate) use deliver::*;
use words::*;

/// The most a stash item can hold, as the accounts server takes it.
const MAX_ITEM_BYTES: u64 = 25 * 1024 * 1024;
/// The largest picture fetched only to be shown small.
const MAX_PICTURE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListParams {
    #[serde(default)]
    taken: Option<StashTaken>,
    #[serde(default)]
    cursor: Option<String>,
}

/// The stash as the apps draw it: none while this host is signed out.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownStash {
    pub signed_in: bool,
    pub items: Vec<ShownStashItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub seen_at: Option<String>,
}

/// An item as the server keeps it, with what apps draw a text or prompt by: its `words`, its
/// text with the links to pages taken out, and the state of each pull request or issue it
/// links to.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownStashItem {
    #[serde(flatten)]
    pub item: StashItem,
    /// What it is called: its page's title, its text's first line, or its file's name.
    pub title: String,
    /// It was stashed since the person last looked, on any device.
    pub fresh: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<ShownStashLink>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ShownStashLink {
    #[serde(flatten)]
    pub link: StashLink,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// One page of the account's stash, newest first; by default only what has not been used.
pub(crate) async fn list(core: &Core, params: ListParams) -> CoreResult<ShownStash> {
    if core.remote.signed_in_api().is_none() {
        return Ok(ShownStash::default());
    }
    let taken = params.taken.unwrap_or(StashTaken::False);
    let list = page(core, taken, params.cursor.as_deref()).await?;
    let references: Vec<&str> = (list.items.iter())
        .flat_map(|item| item.links.iter().flatten())
        .map(|link| link.url.as_str())
        .filter(|url| sikemux_markdown::reference(url).is_some())
        .collect();
    let states = super::codehost::known_states(core, &references);
    let seen = list.seen_at.as_deref().and_then(epoch_ms);
    let items = (list.items.into_iter())
        .map(|item| {
            let fresh = match (seen, epoch_ms(&item.created_at)) {
                (Some(seen), Some(at)) => at > seen,
                _ => seen.is_none(),
            };
            shown_item(item, fresh, &states)
        })
        .collect();
    Ok(ShownStash {
        signed_in: true,
        items,
        cursor: list.cursor,
        seen_at: list.seen_at,
    })
}

fn epoch_ms(at: &str) -> Option<i128> {
    time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|at| at.unix_timestamp_nanos())
}

/// What an item is called: its page's title, its text's first line, or its file's name.
pub fn title(item: &StashItem) -> String {
    fn text(said: &Option<String>) -> Option<&str> {
        said.as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }
    match item.kind {
        StashKind::Link => text(&item.url).map(|url| {
            (item.preview.as_ref().map(|preview| preview.title.trim()))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| {
                    url.trim_start_matches("https://")
                        .trim_start_matches("http://")
                })
                .to_owned()
        }),
        StashKind::Text | StashKind::Prompt => {
            text(&item.text).map(|text| text.lines().next().unwrap_or(text).to_owned())
        }
        _ => text(&item.name).map(str::to_owned),
    }
    .unwrap_or_else(|| "Untitled".to_owned())
}

/// Adds to an item what apps draw a text or prompt by: its words, and the `state` of each pull
/// request or issue it links to that `states` holds, by its address.
fn shown_item(
    mut item: StashItem,
    fresh: bool,
    states: &HashMap<String, String>,
) -> ShownStashItem {
    let title = title(&item);
    if !matches!(item.kind, StashKind::Text | StashKind::Prompt) {
        return ShownStashItem {
            item,
            title,
            fresh,
            links: None,
            words: None,
        };
    }
    let mut cut = Vec::new();
    let links = item.links.take().map(|links| {
        (links.into_iter())
            .map(|link| {
                let state = match sikemux_markdown::reference(&link.url) {
                    Some(_) => states.get(&link.url).cloned(),
                    None => {
                        cut.push(link.url.clone());
                        None
                    }
                };
                ShownStashLink { link, state }
            })
            .collect()
    });
    let words = words(item.text.as_deref().unwrap_or_default(), &mut cut);
    ShownStashItem {
        item,
        title,
        fresh,
        links,
        words: Some(words),
    }
}

async fn page(core: &Core, taken: StashTaken, cursor: Option<&str>) -> CoreResult<StashList> {
    let taken = serde_json::to_value(taken)?;
    let mut path = format!("/v1/stash?taken={}", taken.as_str().unwrap_or("false"));
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.extend(url::form_urlencoded::byte_serialize(cursor.as_bytes()));
    }
    as_host(core, Method::GET, &path, None).await
}

/// An item of the stash by its id, as the server writes it in a path.
fn item_path(id: &str) -> CoreResult<String> {
    let usable = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_'));
    if !usable {
        return Err(CoreError::from("that is not an item of the stash"));
    }
    Ok(format!("/v1/stash/{id}"))
}

async fn item(core: &Core, id: &str) -> CoreResult<StashItem> {
    as_host(core, Method::GET, &item_path(id)?, None).await
}

async fn take(core: &Core, id: &str) -> CoreResult<()> {
    let path = item_path(id)?;
    let update = StashUpdate {
        note: None,
        taken: Some(true),
    };
    let body = Some(serde_json::to_value(update)?);
    let _: StashItem = as_host(core, Method::PATCH, &path, body).await?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct StashItemParams {
    pub id: String,
}

/// Takes an item out of the account's stash for good, on every device.
pub(crate) async fn remove(core: &Core, params: StashItemParams) -> CoreResult<()> {
    host_request(core, Method::DELETE, &item_path(&params.id)?, None).await?;
    core.changed(What::Read(Topic::Stash));
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct SeeParams {
    at: String,
}

/// The person has seen the stash as it stood at `at`, the newest item they saw, on every device.
pub(crate) async fn see(core: &Core, params: SeeParams) -> CoreResult<StashSeen> {
    let body = serde_json::to_value(StashSeen { at: params.at })?;
    let seen = as_host(core, Method::PUT, "/v1/stash/seen", Some(body)).await?;
    core.changed(What::Read(Topic::Stash));
    Ok(seen)
}

#[derive(Serialize)]
pub(crate) struct Kept {
    path: PathBuf,
}

/// The item's file, kept on this host for the person to open.
pub(crate) async fn keep(core: &Core, params: StashItemParams) -> CoreResult<Kept> {
    let item = item(core, &params.id).await?;
    if !has_file(&item) {
        return Err(CoreError::from("that item has no file"));
    }
    Ok(Kept {
        path: download(core, &item).await?,
    })
}

/// A picture in the stash, in base64, for an app to show small.
pub(crate) async fn picture(core: &Core, params: StashItemParams) -> CoreResult<Value> {
    let item = item(core, &params.id).await?;
    let mime = item.mime.clone().unwrap_or_default();
    if item.kind != StashKind::Image || !mime.starts_with("image/") {
        return Err(CoreError::from("that item is not a picture"));
    }
    if item
        .size
        .is_some_and(|size| size as u64 > MAX_PICTURE_BYTES)
    {
        return Err(CoreError::from("that picture is too large to show small"));
    }
    let bytes = file(core, &item, MAX_PICTURE_BYTES).await?;
    Ok(json!({ "mime": mime, "data": STANDARD.encode(bytes) }))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachStashParams {
    pub id: String,
    pub agent_id: String,
    /// False when the app keeps its own draft: the item's file is only kept here, and the app
    /// adds it to its message and marks the item used once it is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachedStash {
    pub agent_id: String,
    /// Where the item's file was kept on this host, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Where each of the item's files was kept: its own, or a prompt's.
    pub paths: Vec<PathBuf>,
}

impl AttachedStash {
    fn new(agent_id: String, paths: Vec<PathBuf>) -> Self {
        Self {
            agent_id,
            path: paths.first().cloned(),
            paths,
        }
    }
}

/// Puts a stash item in the draft of one of this host's chats, unsent, and marks it used; or
/// with `draft` false only keeps its file where the chat's agent can read it.
pub(crate) async fn attach(core: &Core, params: AttachStashParams) -> CoreResult<AttachedStash> {
    if core.workspaces.agent(&params.agent_id).is_none() {
        return Err(CoreError::from("that chat is no longer on this host"));
    }
    let item = item(core, &params.id).await?;
    if params.draft == Some(false) {
        let paths = files(core, &item).await?;
        return Ok(AttachedStash::new(params.agent_id, paths));
    }
    let _delivering = Delivering::start(&item.id)
        .ok_or_else(|| CoreError::from("that item is already on its way to a chat"))?;
    let paths = into_draft(core, &item, &params.agent_id).await?;
    take(core, &item.id).await?;
    core.changed(What::Read(Topic::Stash));
    Ok(AttachedStash::new(params.agent_id, paths))
}

/// Downloads the item's file, when it has one, and adds it with the item's words to the
/// draft of `agent_id`'s chat.
async fn into_draft(core: &Core, item: &StashItem, agent_id: &str) -> CoreResult<Vec<PathBuf>> {
    let paths = files(core, item).await?;
    let key = super::drafts::chat_key(agent_id);
    let said = match item.kind {
        StashKind::Prompt => item.text.as_deref(),
        _ => None,
    };
    let words = [said, item.note.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|words| !words.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let delivery = Delivery {
        paths: paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        context: context(item),
        words,
    };
    core.drafts.deliver(&key, delivery)?;
    super::drafts::changed(core, vec![key]);
    Ok(paths)
}

/// The item's files kept on this host: its own, or each of a prompt's.
async fn files(core: &Core, item: &StashItem) -> CoreResult<Vec<PathBuf>> {
    let mut paths = Vec::new();
    match item.kind {
        StashKind::Prompt => {
            for file in item.attachments.iter().flatten() {
                paths.push(download(core, file).await?);
            }
        }
        _ if has_file(item) => paths.push(download(core, item).await?),
        _ => {}
    }
    Ok(paths)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashPromptParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct StashedPrompt {
    pub id: String,
}

/// The most a prompt's text can be, as the accounts server takes it.
const MAX_PROMPT_CHARS: usize = 16_000;
const MAX_PROMPT_FILES: usize = 20;

/// Keeps what is written in `agent_id`'s draft, its files and the links it holds as one prompt
/// in the Stash, and empties the draft.
pub(crate) async fn stash_prompt(
    core: &Core,
    params: StashPromptParams,
) -> CoreResult<StashedPrompt> {
    let key = super::drafts::chat_key(&params.agent_id);
    let draft = core
        .drafts
        .chat_draft(&params.agent_id)
        .ok_or_else(|| CoreError::from("there is nothing written to stash"))?;
    let extra = draft.extra.unwrap_or_default();
    let files: Vec<PathBuf> = extra["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|path| path.as_str().map(PathBuf::from))
        .collect();
    let links = extra["context"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["uri"].as_str())
        .filter(|uri| uri.starts_with("https://") || uri.starts_with("http://"));
    let text = [draft.text.trim()]
        .into_iter()
        .chain(links)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() && files.is_empty() {
        return Err(CoreError::from("there is nothing written to stash"));
    }
    if text.chars().count() > MAX_PROMPT_CHARS {
        return Err(CoreError::from(
            "a stashed prompt can be at most 16,000 characters",
        ));
    }
    if files.len() > MAX_PROMPT_FILES {
        return Err(CoreError::from(
            "a stashed prompt can carry at most 20 files",
        ));
    }
    let body = json!({ "kind": "prompt", "text": text, "attachmentCount": files.len() });
    let prompt: StashItem = as_host(core, Method::POST, "/v1/stash", Some(body)).await?;
    for file in files {
        upload(core, &prompt.id, file).await?;
    }
    core.drafts.set(&key, String::new(), None)?;
    super::drafts::changed(core, vec![key]);
    core.changed(What::Read(Topic::Stash));
    Ok(StashedPrompt { id: prompt.id })
}

/// Sends one of a prompt's files to the Stash, as part of prompt `id`.
async fn upload(core: &Core, id: &str, file: PathBuf) -> CoreResult<()> {
    let read = file.clone();
    let (bytes, mime) = blocking(move || {
        let size = std::fs::metadata(&read)
            .map_err(|error| CoreError::from(format!("{}: {error}", read.display())))?
            .len();
        if size > MAX_ITEM_BYTES {
            return Err(CoreError::from(format!(
                "{} is larger than the 25 MB a stash item can be",
                read.display()
            )));
        }
        let bytes = std::fs::read(&read)
            .map_err(|error| CoreError::from(format!("{}: {error}", read.display())))?;
        let mime = crate::http::mime_of(&read).unwrap_or("application/octet-stream");
        Ok((bytes, mime.to_owned()))
    })
    .await?;
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let upload = Upload {
        bytes,
        content_type: mime,
        headers: vec![
            (
                "x-stash-name",
                url::form_urlencoded::byte_serialize(name.as_bytes()).collect(),
            ),
            ("x-stash-prompt", id.to_owned()),
        ],
    };
    host_send(core, Method::POST, "/v1/stash", Some(upload)).await?;
    Ok(())
}

fn has_file(item: &StashItem) -> bool {
    matches!(
        item.kind,
        StashKind::Image | StashKind::Video | StashKind::File
    )
}

/// A link or text as a chip in the draft, which the message hands the agent with its words: a
/// link's title, address and description, or the text itself.
fn context(item: &StashItem) -> Option<Value> {
    match item.kind {
        StashKind::Link => {
            let url = item.url.as_deref()?;
            let preview = item.preview.as_ref();
            let title = preview
                .map(|preview| preview.title.trim())
                .filter(|title| !title.is_empty())
                .unwrap_or(url);
            let about = preview
                .map(|preview| preview.description.trim())
                .unwrap_or_default();
            let text = [title, url, about]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            Some(json!({ "uri": url, "title": title, "text": text }))
        }
        StashKind::Text => {
            let text = item.text.as_deref()?.trim();
            let title = text.lines().next().unwrap_or(text);
            Some(json!({
                "uri": format!("{}{}", sikemux_view::chat::STASH_URI, item.id),
                "title": title.chars().take(80).collect::<String>(),
                "text": text,
            }))
        }
        _ => None,
    }
}

async fn download(core: &Core, item: &StashItem) -> CoreResult<PathBuf> {
    let dir = core
        .config
        .attachment_dir
        .clone()
        .ok_or_else(|| CoreError::from("this host has nowhere to keep files"))?;
    let bytes = file(core, item, MAX_ITEM_BYTES).await?;
    let name = item.name.clone().unwrap_or_default();
    let mime = item.mime.clone().unwrap_or_default();
    blocking(move || attachments::save_bytes(&dir, &name, &mime, &bytes)).await
}

/// The item's file, refused once it runs past `most` bytes.
async fn file(core: &Core, item: &StashItem, most: u64) -> CoreResult<Vec<u8>> {
    let too_large = || CoreError::from("that item is larger than it may be here");
    if item.size.is_some_and(|size| size as u64 > most) {
        return Err(too_large());
    }
    let path = format!("{}/file", item_path(&item.id)?);
    let mut response = host_request(core, Method::GET, &path, None).await?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| CoreError::from(error.to_string()))?
    {
        if (bytes.len() + chunk.len()) as u64 > most {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
