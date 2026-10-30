//! The account's Stash: what the person shared into Sikemux from another app, kept on the
//! accounts server for every device and host on the account. An item shared to one of this
//! host's chats lands in that chat's draft as soon as the host hears of it; any other waits
//! until the person attaches it from a composer.

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
    AccountEvent, AccountEventType, StashItem, StashKind, StashList, StashSeen, StashTaken,
    StashTarget, StashUpdate,
};

use super::changed::Topic;
use super::connection::blocking;
use super::drafts::Delivery;
use super::hosts::account::{as_host, host_request, host_send, Upload};
use super::{attachments, Core, CoreError, CoreResult};

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
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Shown {
    signed_in: bool,
    /// Each item as the server keeps it, with what [`shown_item`] adds.
    items: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    seen_at: Option<String>,
}

/// One page of the account's stash, newest first; by default only what has not been used.
pub(crate) async fn list(core: &Core, params: ListParams) -> CoreResult<Shown> {
    if core.remote.signed_in_api().is_none() {
        return Ok(Shown {
            signed_in: false,
            items: Vec::new(),
            cursor: None,
            seen_at: None,
        });
    }
    let taken = params.taken.unwrap_or(StashTaken::False);
    let list = page(core, taken, params.cursor.as_deref()).await?;
    let references: Vec<&str> = (list.items.iter())
        .flat_map(|item| item.links.iter().flatten())
        .map(|link| link.url.as_str())
        .filter(|url| sikemux_markdown::reference(url).is_some())
        .collect();
    let states = super::codehost::known_states(core, &references);
    let mut items = Vec::with_capacity(list.items.len());
    for item in &list.items {
        let mut item = serde_json::to_value(item)?;
        shown_item(&mut item, &states);
        items.push(item);
    }
    Ok(Shown {
        signed_in: true,
        items,
        cursor: list.cursor,
        seen_at: list.seen_at,
    })
}

/// Adds to an item, as the server keeps it, what apps draw it by: a text's or prompt's `words`,
/// its text with the links to pages taken out, and the `state` of each pull request or issue it
/// links to that `states` holds, by its address.
fn shown_item(item: &mut Value, states: &HashMap<String, String>) {
    if !matches!(item["kind"].as_str(), Some("text" | "prompt")) {
        return;
    }
    let mut cut = Vec::new();
    for link in item["links"].as_array_mut().into_iter().flatten() {
        let Some(url) = link["url"].as_str().map(str::to_owned) else {
            continue;
        };
        match sikemux_markdown::reference(&url) {
            Some(_) => {
                if let Some(state) = states.get(&url) {
                    link["state"] = json!(state);
                }
            }
            None => cut.push(url),
        }
    }
    let words = words(item["text"].as_str().unwrap_or_default(), &mut cut);
    item["words"] = json!(words);
}

/// Where a link was taken out of a text, while what it leaves around it is tidied.
const CUT: char = '\u{1}';

/// `text` with the links in `cut` taken out, and what they leave dangling with them: an `and`,
/// `or` or comma before a link that ended the sentence, brackets around only links, doubled
/// spaces and a space before punctuation.
fn words(text: &str, cut: &mut [String]) -> String {
    cut.sort_by_key(|url| std::cmp::Reverse(url.len()));
    let mut marked = text.to_owned();
    for url in cut.iter() {
        marked = marked.replace(url.as_str(), &CUT.to_string());
    }
    let lines: Vec<String> = marked.lines().map(tidied).collect();
    let mut words = String::new();
    let mut blank = false;
    for line in lines {
        if line.is_empty() {
            blank = !words.is_empty();
            continue;
        }
        if !words.is_empty() {
            words.push_str(if blank { "\n\n" } else { "\n" });
        }
        words.push_str(&line);
        blank = false;
    }
    words
}

enum Token<'a> {
    Word(&'a str),
    /// A link taken out, with the punctuation written before and after it.
    Cut {
        open: &'a str,
        close: &'a str,
    },
}

fn tidied(line: &str) -> String {
    let tokens: Vec<Token> = line
        .split_whitespace()
        .flat_map(|word| {
            let Some(start) = word.find(CUT) else {
                return vec![Token::Word(word)];
            };
            let end = word.rfind(CUT).map_or(start, |end| end + CUT.len_utf8());
            let (before, close) = (&word[..start], &word[end..]);
            match before.chars().any(char::is_alphanumeric) {
                true => vec![Token::Word(before), Token::Cut { open: "", close }],
                false => vec![Token::Cut {
                    open: before,
                    close,
                }],
            }
        })
        .collect();
    let joins = |word: &str| matches!(word, "and" | "or" | "&" | ",");
    let mut kept: Vec<String> = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        let Token::Cut { open, .. } = tokens[at] else {
            if let Token::Word(word) = tokens[at] {
                kept.push(word.to_owned());
            }
            at += 1;
            continue;
        };
        let mut last = at;
        let mut next = at + 1;
        while next < tokens.len() {
            match tokens[next] {
                Token::Cut { .. } => last = next,
                Token::Word(word)
                    if joins(word) && matches!(tokens.get(next + 1), Some(Token::Cut { .. })) => {}
                Token::Word(_) => break,
            }
            next += 1;
        }
        let close = match tokens[last] {
            Token::Cut { close, .. } => close,
            Token::Word(_) => "",
        };
        let ends = next == tokens.len() || close.contains(['.', '!', '?', ';', ':']);
        let closing = |char: char| !open.is_empty() && matches!(char, ')' | ']' | '}');
        let mut after: String = close.chars().filter(|char| !closing(*char)).collect();
        if ends {
            after.retain(|char| char != ',');
            if kept.last().is_some_and(|word| joins(word)) {
                kept.pop();
            }
            if let Some(word) = kept.last_mut() {
                if word.ends_with(',') {
                    word.pop();
                }
            }
        }
        match kept.last_mut() {
            Some(word) => word.push_str(&after),
            None if !after.chars().all(|char| char.is_ascii_punctuation()) => kept.push(after),
            None => {}
        }
        at = last + 1;
    }
    kept.join(" ")
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

#[derive(Deserialize)]
pub struct ItemParams {
    id: String,
}

/// Takes an item out of the account's stash for good, on every device.
pub(crate) async fn remove(core: &Core, params: ItemParams) -> CoreResult<()> {
    host_request(core, Method::DELETE, &item_path(&params.id)?, None).await?;
    core.changes.changed(Topic::Stash, None, None);
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
    core.changes.changed(Topic::Stash, None, None);
    Ok(seen)
}

#[derive(Serialize)]
pub(crate) struct Kept {
    path: PathBuf,
}

/// The item's file, kept on this host for the person to open.
pub(crate) async fn keep(core: &Core, params: ItemParams) -> CoreResult<Kept> {
    let item = item(core, &params.id).await?;
    if !has_file(&item) {
        return Err(CoreError::from("that item has no file"));
    }
    Ok(Kept {
        path: download(core, &item).await?,
    })
}

/// A picture in the stash, in base64, for an app to show small.
pub(crate) async fn picture(core: &Core, params: ItemParams) -> CoreResult<Value> {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachParams {
    id: String,
    agent_id: String,
    /// False when the app keeps its own draft: the item's file is only kept here, and the app
    /// adds it to its message and marks the item used once it is sent.
    #[serde(default)]
    draft: Option<bool>,
}

/// What attaching an item put in the chat's draft.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attached {
    agent_id: String,
    /// Where the item's file was kept on this host, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    /// Where each of the item's files was kept: its own, or a prompt's.
    paths: Vec<PathBuf>,
}

impl Attached {
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
pub(crate) async fn attach(core: &Core, params: AttachParams) -> CoreResult<Attached> {
    if core.workspaces.agent(&params.agent_id).is_none() {
        return Err(CoreError::from("that chat is no longer on this host"));
    }
    let item = item(core, &params.id).await?;
    if params.draft == Some(false) {
        let paths = files(core, &item).await?;
        return Ok(Attached::new(params.agent_id, paths));
    }
    let _delivering = Delivering::start(&item.id)
        .ok_or_else(|| CoreError::from("that item is already on its way to a chat"))?;
    let paths = into_draft(core, &item, &params.agent_id).await?;
    take(core, &item.id).await?;
    core.changes.changed(Topic::Stash, None, None);
    Ok(Attached::new(params.agent_id, paths))
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    agent_id: String,
}

#[derive(Serialize)]
pub struct Stashed {
    id: String,
}

/// The most a prompt's text can be, as the accounts server takes it.
const MAX_PROMPT_CHARS: usize = 16_000;
/// The most files a prompt can carry.
const MAX_PROMPT_FILES: usize = 20;

/// Keeps what is written in `agent_id`'s draft, its files and the links it holds as one prompt
/// in the Stash, and empties the draft.
pub(crate) async fn stash_prompt(core: &Core, params: PromptParams) -> CoreResult<Stashed> {
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
    core.changes.changed(Topic::Stash, None, None);
    Ok(Stashed { id: prompt.id })
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
                "uri": format!("sikemux://stash/{}", item.id),
                "title": title.chars().take(80).collect::<String>(),
                "text": text,
            }))
        }
        _ => None,
    }
}

async fn download(core: &Core, item: &StashItem) -> CoreResult<PathBuf> {
    let dir = core
        .listening
        .get()
        .and_then(|listening| listening.config.attachment_dir.clone())
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

/// The items being put in a chat's draft right now, so an event and a catch-up that both
/// hear of one deliver it once.
struct Delivering(String);

impl Delivering {
    fn held() -> &'static Mutex<HashSet<String>> {
        static HELD: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        HELD.get_or_init(Default::default)
    }

    fn start(id: &str) -> Option<Self> {
        Self::held()
            .locked()
            .insert(id.to_owned())
            .then(|| Self(id.to_owned()))
    }
}

impl Drop for Delivering {
    fn drop(&mut self) {
        Self::held().locked().remove(&self.0);
    }
}

/// What the live connection heard about the stash: every app reads it again, and an item
/// shared to one of this host's chats is delivered there.
pub(crate) fn heard(core: &Arc<Core>, events: &[AccountEvent]) {
    let ids: Vec<String> = events
        .iter()
        .filter(|event| {
            matches!(
                event.r#type,
                AccountEventType::StashAdded | AccountEventType::StashChanged
            )
        })
        .filter_map(|event| event.stash_id.clone())
        .collect();
    let stash_moved = events.iter().any(|event| {
        matches!(
            event.r#type,
            AccountEventType::StashAdded
                | AccountEventType::StashChanged
                | AccountEventType::StashRemoved
                | AccountEventType::StashSeen
        )
    });
    if stash_moved {
        core.changes.changed(Topic::Stash, None, None);
    }
    if ids.is_empty() {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        for id in ids {
            match item(&core, &id).await {
                Ok(item) => deliver(&core, item).await,
                Err(error) => eprintln!("sikemux core: could not read stash item {id}: {error}"),
            }
        }
    });
}

/// Delivers every item shared to this host's chats while it was away, once its live
/// connection is up again.
pub(crate) fn catch_up(core: &Arc<Core>) {
    let core = core.clone();
    tokio::spawn(async move {
        let mut cursor = None;
        loop {
            let list = match page(&core, StashTaken::False, cursor.as_deref()).await {
                Ok(list) => list,
                Err(error) => {
                    eprintln!("sikemux core: could not read the stash: {error}");
                    return;
                }
            };
            for item in list.items {
                deliver(&core, item).await;
            }
            match list.cursor {
                Some(next) => cursor = Some(next),
                None => return,
            }
        }
    });
}

/// Puts `item` in the chat it was shared to, when that chat is on this host, and marks it
/// used. One shared to a chat since closed, or a project no longer open here, stays in the
/// stash for the person to attach.
async fn deliver(core: &Arc<Core>, item: StashItem) {
    if item.taken_at.is_some() {
        return;
    }
    let Some(target) = item.target.as_ref() else {
        return;
    };
    if core.remote.core_id().as_deref() != Some(target.host_key.as_str()) {
        return;
    }
    let Some(_delivering) = Delivering::start(&item.id) else {
        return;
    };
    if let Err(error) = deliver_to(core, &item, target).await {
        eprintln!(
            "sikemux core: could not put stash item {} in its chat: {error}",
            item.id
        );
    }
}

async fn deliver_to(core: &Arc<Core>, item: &StashItem, target: &StashTarget) -> CoreResult<()> {
    let agent_id = match (&target.chat_id, &target.project_path) {
        (Some(chat), _) if core.workspaces.agent(chat).is_some() => chat.clone(),
        (Some(_), _) => return Ok(()),
        (None, Some(project)) if target.new_chat == Some(true) => {
            match new_chat(core, project).await? {
                Some(agent_id) => agent_id,
                None => return Ok(()),
            }
        }
        _ => return Ok(()),
    };
    into_draft(core, item, &agent_id).await?;
    take(core, &item.id).await?;
    core.changes.changed(Topic::Stash, None, None);
    Ok(())
}

/// Opens a new chat in the project at `folder`, as a new chat there opens: it waits for the
/// person, so its agent starts only once they write or send. None when that project is not
/// open on this host or no agent here can chat.
async fn new_chat(core: &Arc<Core>, folder: &str) -> CoreResult<Option<String>> {
    let view = core.workspace_view();
    let Some(project) = view
        .projects
        .iter()
        .find(|project| project.path.to_string_lossy() == folder)
    else {
        return Ok(None);
    };
    let Some(launcher) = project.launcher.clone() else {
        return Ok(None);
    };
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4().simple());
    let chosen = super::launching::Launch {
        agent_id: agent_id.clone(),
        launcher,
        cwd: project.path.clone(),
        permission_mode: None,
        model: None,
        model_provider: None,
        effort: None,
        resume_id: None,
        account: None,
        fallbacks: Vec::new(),
        program: None,
        waits_for_person: true,
    };
    super::launching::start(core, None, chosen).await?;
    let tab = json!({ "project": project.id, "kind": "agent", "target": agent_id });
    if let Ok(tab) = serde_json::from_value(tab) {
        let _ = super::layout::open_tab(core, tab);
    }
    Ok(Some(agent_id))
}
