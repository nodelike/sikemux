//! Chats shared read-only with whoever opens their link. A share is a client
//! of the core that attaches to one chat as a phone would, seals what it
//! hears with the key in the link, and hands it to the accounts server, which
//! passes it to the signed-in people watching. Viewers never reach the core:
//! the one thing they may do is ask, sealed, for an older page of the chat, what
//! one of its tool calls changed and printed, or a picture pasted into it, and
//! the core decides what to answer.

mod link;
mod seal;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::access::Peer;
use super::changed::Topic;
use super::connection::ClientConn;
use super::remote::StoredShare;
use super::{chat, Core, CoreError, CoreResult};
use seal::Seal;

/// The shortest and longest a share may stay open.
const MIN_LIFETIME_MS: u64 = 60_000;
const MAX_LIFETIME_MS: u64 = 7 * 24 * 60 * 60_000;
/// How many turns one older page holds.
const PAGE_TURNS: u32 = 20;
/// The largest pasted picture a viewer is sent.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// What the app shows of one share.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareView {
    pub id: String,
    pub agent_id: String,
    pub url: String,
    /// Milliseconds since the Unix epoch.
    pub expires_at: u64,
    pub viewers: u32,
    pub state: ShareState,
    /// Why the server would not open it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ShareState {
    /// Waiting for the accounts server, which is out of reach or has not
    /// answered yet.
    Connecting,
    Open,
    Refused,
}

/// One share the core runs.
struct Running {
    stored: StoredShare,
    seal: Arc<Seal>,
    client: Arc<ClientConn>,
    pump: JoinHandle<()>,
    viewers: u32,
    state: ShareState,
    problem: Option<String>,
    /// The last frame published, numbered from 1.
    seq: u64,
    /// The run of the chat's agent the last snapshot came from, to page back
    /// through.
    feed: Option<String>,
    /// The chat the last snapshot was asked of. A chat started again under
    /// the same agent is a new one, and gets a snapshot of its own.
    chat: Option<usize>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.pump.abort();
        self.client.close();
    }
}

/// What reaches the share connection.
pub(super) enum ToLink {
    Open(String),
    Close(String),
    /// A frame the chat sent a share's client.
    Heard(String, Arc<[u8]>),
    Send(crate::accounts::share::ShareClientMessage),
    /// The network came back: stop waiting out the backoff and connect now.
    Retry,
}

#[derive(Default)]
struct Inner {
    running: HashMap<String, Running>,
    /// Shares stopped here that the server has not yet heard are stopped.
    closing: Vec<StoredShare>,
    link: Option<mpsc::UnboundedSender<ToLink>>,
}

#[derive(Default)]
pub(crate) struct Shares {
    inner: Mutex<Inner>,
    next_request: AtomicU64,
}

/// Where a share's link opens: the web app, which hands it on to the Mac or
/// phone app when one is installed. The key stays after `#`, which browsers
/// never send to a server.
fn url_of(stored: &StoredShare) -> String {
    format!(
        "{}/s/{}#{}",
        crate::accounts::app_base(),
        stored.id,
        stored.key
    )
}

fn view_of(running: &Running) -> ShareView {
    ShareView {
        id: running.stored.id.clone(),
        agent_id: running.stored.agent_id.clone(),
        url: url_of(&running.stored),
        expires_at: running.stored.expires_at,
        viewers: running.viewers,
        state: running.state,
        problem: running.problem.clone(),
    }
}

/// Clients read the shares again: one began, ended, or its state, problem
/// or viewers moved.
fn changed(core: &Core) {
    core.changes.changed(Topic::Shares, None, None);
}

fn save(core: &Core, inner: &Inner) {
    changed(core);
    let mut shares: Vec<StoredShare> = inner
        .running
        .values()
        .map(|running| running.stored.clone())
        .collect();
    shares.extend(inner.closing.iter().map(|stored| StoredShare {
        agent_id: String::new(),
        ..stored.clone()
    }));
    shares.sort_by(|a, b| a.id.cmp(&b.id));
    if let Err(error) = core.remote.keep_shares(shares) {
        eprintln!("sikemux core: could not save the shared chats: {error}");
    }
}

/// Has a link waiting out a failure connect again now, if one runs.
pub(crate) fn network_back(core: &Core) {
    if let Some(link) = &core.shares.inner.locked().link {
        let _ = link.send(ToLink::Retry);
    }
}

/// Sends `message` to the share connection, starting it first if it is not
/// running.
fn tell(core: &Arc<Core>, inner: &mut Inner, message: ToLink) {
    let message = match &inner.link {
        Some(link) => match link.send(message) {
            Ok(()) => return,
            Err(mpsc::error::SendError(message)) => message,
        },
        None => message,
    };
    let Some(base) = core.remote.signed_in_api() else {
        return;
    };
    let (sender, receiver) = mpsc::unbounded_channel();
    let _ = sender.send(message);
    inner.link = Some(sender);
    tokio::spawn(link::run(Arc::downgrade(core), base, receiver));
}

/// Starts sharing `stored`: attaches its client and asks the server to open it.
fn begin(core: &Arc<Core>, inner: &mut Inner, stored: StoredShare) -> CoreResult<()> {
    let seal = Seal::new(&stored.id, &stored.key).ok_or("the share's key is not valid")?;
    let (client, mut frames) = ClientConn::inside(
        core,
        Peer::Share {
            id: stored.id.clone(),
        },
    );
    let id = stored.id.clone();
    let reader = client.clone();
    let weak = Arc::downgrade(core);
    let pump = tokio::spawn(async move {
        while let Some(frame) = frames.recv().await {
            reader.took(frame.len());
            let Some(core) = weak.upgrade() else {
                return;
            };
            let mut inner = core.shares.inner.locked();
            tell(&core, &mut inner, ToLink::Heard(id.clone(), frame));
        }
    });
    let id = stored.id.clone();
    inner.running.insert(
        id.clone(),
        Running {
            stored,
            seal: Arc::new(seal),
            client,
            pump,
            viewers: 0,
            state: ShareState::Connecting,
            problem: None,
            seq: 0,
            feed: None,
            chat: None,
        },
    );
    tell(core, inner, ToLink::Open(id));
    Ok(())
}

/// Shares the chat of `agent_id` for `lifetime_ms`, or answers with the share
/// it already has. A share whose chat the app renamed tells its viewers.
pub(crate) fn share_chat(
    core: &Arc<Core>,
    agent_id: &str,
    lifetime_ms: u64,
    title: Option<String>,
) -> CoreResult<ShareView> {
    let title = title.as_deref().and_then(chat::feed::title);
    if core.remote.signed_in_api().is_none() {
        return Err("Sign in to Sikemux to share chats.".into());
    }
    if core.chats.get(agent_id).is_none() {
        return Err("This chat is not running, so there is nothing to share yet.".into());
    }
    let mut inner = core.shares.inner.locked();
    if let Some(running) = inner
        .running
        .values_mut()
        .find(|running| running.stored.agent_id == agent_id && running.state != ShareState::Refused)
    {
        let view = view_of(running);
        if title.is_none() || running.stored.title == title {
            return Ok(view);
        }
        running.stored.title = title;
        let id = running.stored.id.clone();
        save(core, &inner);
        drop(inner);
        take_snapshot(core, &id);
        return Ok(view);
    }
    let random = || seal::random_text(16).ok_or_else(|| CoreError::from("no randomness"));
    let stored = StoredShare {
        id: random()?,
        key: seal::random_text(seal::KEY_LEN).ok_or("no randomness")?,
        agent_id: agent_id.to_owned(),
        expires_at: epoch_ms() + lifetime_ms.clamp(MIN_LIFETIME_MS, MAX_LIFETIME_MS),
        title,
    };
    let id = stored.id.clone();
    inner
        .running
        .retain(|_, running| running.stored.agent_id != agent_id);
    begin(core, &mut inner, stored)?;
    save(core, &inner);
    Ok(inner
        .running
        .get(&id)
        .map(view_of)
        .ok_or("the share did not start")?)
}

/// Stops a share for good. Its viewers are told it was stopped.
pub(crate) fn stop(core: &Arc<Core>, id: &str) -> CoreResult<()> {
    let mut inner = core.shares.inner.locked();
    let running = inner
        .running
        .remove(id)
        .ok_or("that share has already ended")?;
    chat::detach(core, running.client.id, &running.stored.agent_id);
    let refused = running.state == ShareState::Refused;
    let stored = running.stored.clone();
    drop(running);
    if !refused {
        inner.closing.push(stored);
        tell(core, &mut inner, ToLink::Close(id.to_owned()));
    }
    save(core, &inner);
    Ok(())
}

pub(crate) fn list(core: &Core) -> Vec<ShareView> {
    let inner = core.shares.inner.locked();
    let mut views: Vec<ShareView> = inner.running.values().map(view_of).collect();
    views.sort_by(|a, b| a.id.cmp(&b.id));
    views
}

/// Takes up the shares a core before this one ran.
pub(crate) fn restore(core: &Arc<Core>) {
    let now = epoch_ms();
    let mut inner = core.shares.inner.locked();
    for stored in core.remote.shares() {
        if stored.expires_at <= now {
            continue;
        }
        if stored.agent_id.is_empty() {
            let id = stored.id.clone();
            inner.closing.push(stored);
            tell(core, &mut inner, ToLink::Close(id));
        } else if let Err(error) = begin(core, &mut inner, stored) {
            eprintln!("sikemux core: a shared chat did not carry on: {error}");
        }
    }
    save(core, &inner);
}

/// The plain text of a snapshot: the chat as it stands, from its last turns.
fn snapshot_text(core: &Core, agent_id: &str, title: Option<String>, attachment: &Value) -> Value {
    let info = core.chats.get(agent_id).map(|chat| chat.info());
    json!({
        "type": "snapshot",
        "chat": {
            "agentId": agent_id,
            "provider": info.as_ref().map(|info| info.provider.clone()),
            "title": title.or_else(|| info.as_ref().and_then(|info| info.title.clone())),
            "model": info.as_ref().and_then(|info| info.model.clone()),
            "cwd": info.as_ref().map(|info| info.cwd.clone()),
            "home": std::env::var_os("HOME").map(PathBuf::from),
        },
        "attachment": attachment,
    })
}

/// How every chat event the core sends its clients begins. Such a frame is
/// sealed as it came, with nothing read or written again.
const CHAT_EVENT: &[u8] = br#"{"type":"event","event":{"kind":"chat","#;

/// What a share publishes for a frame its client heard, if anything, and
/// whether it is a snapshot. A chat event goes out as the core wrote it; the
/// answer to the share's attach becomes a snapshot.
fn published<'a>(core: &Core, id: &str, frame: &'a [u8]) -> Option<(Cow<'a, [u8]>, bool)> {
    use crate::protocol::{Event, FrameKind, Response, ServerMessage};
    let (kind, payload) = frame.get(4..)?.split_first()?;
    if *kind != FrameKind::Control as u8 {
        return None;
    }
    if payload.starts_with(CHAT_EVENT) {
        return Some((Cow::Borrowed(payload), false));
    }
    match serde_json::from_slice::<ServerMessage>(payload).ok()? {
        ServerMessage::Response {
            response: Response::ChatAttached { attachment },
            ..
        } => {
            let attachment = serde_json::to_value(&attachment).ok()?;
            let mut inner = core.shares.inner.locked();
            let running = inner.running.get_mut(id)?;
            running.feed = attachment
                .pointer("/mark/feed")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let agent_id = running.stored.agent_id.clone();
            let title = running.stored.title.clone();
            drop(inner);
            let text = snapshot_text(core, &agent_id, title, &attachment);
            Some((Cow::Owned(serde_json::to_vec(&text).ok()?), true))
        }
        ServerMessage::Event {
            event: Event::Chat { .. },
        } => Some((Cow::Borrowed(payload), false)),
        _ => None,
    }
}

/// Asks the chat for everything a viewer starting now needs, which comes back
/// through the share's client as a snapshot.
fn take_snapshot(core: &Arc<Core>, id: &str) {
    let Some((client, agent_id)) = core
        .shares
        .inner
        .locked()
        .running
        .get_mut(id)
        .map(|running| {
            running.chat = core
                .chats
                .get(&running.stored.agent_id)
                .map(|chat| Arc::as_ptr(&chat) as usize);
            (running.client.clone(), running.stored.agent_id.clone())
        })
    else {
        return;
    };
    let request_id = core.shares.next_request.fetch_add(1, Ordering::Relaxed);
    let core = core.clone();
    tokio::spawn(async move {
        chat::attach(&core, &client, request_id, &agent_id, None).await;
    });
}

/// A viewer's sealed question and the core's sealed answer, or nothing for a
/// question it cannot read.
async fn answer(core: Arc<Core>, id: String, viewer: String, sealed: String) -> Option<ToLink> {
    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum Ask {
        Page {
            feed: String,
            before: u64,
        },
        #[serde(rename_all = "camelCase")]
        ToolDetail {
            tool_call_id: String,
        },
        File {
            path: PathBuf,
        },
    }
    #[derive(Deserialize)]
    struct Question {
        id: u64,
        #[serde(flatten)]
        ask: Ask,
    }

    let (seal, agent_id) = {
        let inner = core.shares.inner.locked();
        let running = inner.running.get(&id)?;
        (running.seal.clone(), running.stored.agent_id.clone())
    };
    let question: Question = serde_json::from_slice(&seal.open(&sealed)?).ok()?;
    let reply = match question.ask {
        Ask::Page { feed, before } => {
            match chat::history(&core, &agent_id, &feed, before, PAGE_TURNS) {
                Ok(crate::protocol::Response::ChatHistory { page }) => json!({ "page": page }),
                Ok(_) => json!({ "error": "the host answered something else" }),
                Err(error) => json!({ "error": error.to_string() }),
            }
        }
        Ask::ToolDetail { tool_call_id } => {
            match chat::tool_detail(&core, &agent_id, &tool_call_id) {
                Ok(detail) => json!({ "toolDetail": detail }),
                Err(error) => json!({ "error": error.to_string() }),
            }
        }
        Ask::File { path } => match pasted_picture(&core, path).await {
            Ok(data) => json!({ "file": { "data": data } }),
            Err(error) => json!({ "error": error.to_string() }),
        },
    };
    let mut reply = reply;
    reply["id"] = json!(question.id);
    let sealed = seal.seal(&serde_json::to_vec(&reply).ok()?)?;
    Some(ToLink::Send(
        crate::accounts::share::ShareClientMessage::Answer(crate::accounts::share::ShareAnswer {
            share: id,
            viewer,
            sealed,
        }),
    ))
}

/// A picture pasted into a chat, in base64. Viewers may see only those, never
/// another file on this computer.
async fn pasted_picture(core: &Core, path: PathBuf) -> CoreResult<String> {
    use base64::Engine;
    let dir = core
        .listening
        .get()
        .and_then(|listening| listening.config.attachment_dir.clone())
        .ok_or("this host keeps no pasted pictures")?;
    super::connection::blocking(move || {
        let kept = match (std::fs::canonicalize(&dir), std::fs::canonicalize(&path)) {
            (Ok(dir), Ok(path)) => path.starts_with(dir),
            _ => false,
        };
        if !kept {
            return Err("only pictures pasted into the chat are shared".into());
        }
        let size = std::fs::metadata(&path)
            .map_err(|_| CoreError::from("this host no longer has that picture"))?
            .len();
        if size > MAX_FILE_BYTES {
            return Err("that picture is too large to share".into());
        }
        let bytes = std::fs::read(&path)
            .map_err(|_| CoreError::from("this host no longer has that picture"))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    })
    .await
}
