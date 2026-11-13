//! Chats shared read-only with whoever opens their link. A share is a client
//! of the core that attaches to one chat as a phone would, seals what it
//! hears with the key in the link, and hands it to the accounts server, which
//! passes it to the signed-in people watching. Viewers never reach the core:
//! they may ask, sealed, for an older page of the chat, what one of its tool
//! calls changed and printed, or a picture pasted into it, and the core decides
//! what to answer. They may also write a message, which waits in the chat until
//! the person sends or dismisses it.

mod link;
mod seal;

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::share::{GuestMessageParams, StopShareParams};
pub use sikemux_calls::share::{ShareChatParams, ShareState, ShareView};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::GuestMessage;
use sikemux_view::share::{ShareAsk, ShareFrame, ShareReply, SharedChat, SharedFile};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use sikemux_wire::protocol::ChatAttachment;

use super::access::Peer;
use super::changed::Topic;
use super::connection::ClientConn;
use super::notify::Signal;
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
/// The longest name a viewer may give.
const MAX_GUEST_NAME_CHARS: usize = 40;
/// The longest message a viewer may write.
const MAX_GUEST_CHARS: usize = 10_000;

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

pub enum ToLink {
    Open(String),
    Close(String),
    /// A frame the chat sent a share's client, which counts against the client's backlog
    /// until it is sent or let go of.
    Heard(String, Arc<[u8]>, Taken),
    Send(crate::accounts::share::ShareClientMessage),
    /// The network came back: stop waiting out the backoff and connect now.
    Retry,
}

/// Takes a frame off its share client's backlog once dropped, so a share connection that
/// falls behind slows the chat's frames down instead of piling them up here.
pub struct Taken {
    client: Arc<ClientConn>,
    bytes: usize,
}

impl Drop for Taken {
    fn drop(&mut self) {
        self.client.took(self.bytes);
    }
}

#[derive(Default)]
pub struct Inner {
    running: HashMap<String, Running>,
    /// Shares stopped here that the server has not yet heard are stopped.
    closing: Vec<StoredShare>,
    link: Option<mpsc::UnboundedSender<ToLink>>,
}

#[derive(Default)]
pub struct Shares {
    pub inner: Mutex<Inner>,
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
    core.changed(What::Read(Topic::Shares));
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
    if let Err(error) = core.remote().keep_shares(shares) {
        eprintln!("sikemux core: could not save the shared chats: {error}");
    }
}

/// Has a link waiting out a failure connect again now, if one runs.
pub fn network_back(core: &Core) {
    if let Some(link) = &core.shares().inner.locked().link {
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
    let Some(base) = core.remote().signed_in_api() else {
        return;
    };
    let (sender, receiver) = mpsc::unbounded_channel();
    let _ = sender.send(message);
    inner.link = Some(sender);
    tokio::spawn(link::run(Arc::downgrade(core), base, receiver));
}

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
            let taken = Taken {
                client: reader.clone(),
                bytes: frame.len(),
            };
            let Some(core) = weak.upgrade() else {
                return;
            };
            let mut inner = core.shares().inner.locked();
            tell(&core, &mut inner, ToLink::Heard(id.clone(), frame, taken));
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
pub fn share_chat(
    core: &Arc<Core>,
    agent_id: &str,
    lifetime_ms: u64,
    title: Option<String>,
) -> CoreResult<ShareView> {
    let title = title.as_deref().and_then(chat::feed::title);
    if core.remote().signed_in_api().is_none() {
        return Err("Sign in to Sikemux to share chats.".into());
    }
    if core.chats().get(agent_id).is_none() {
        return Err("This chat is not running, so there is nothing to share yet.".into());
    }
    let mut inner = core.shares().inner.locked();
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
pub fn stop(core: &Arc<Core>, id: &str) -> CoreResult<()> {
    let mut inner = core.shares().inner.locked();
    let running = inner
        .running
        .remove(id)
        .ok_or("that share has already ended")?;
    chat::detach(core, running.client.id, &running.stored.agent_id);
    let agent_id = &running.stored.agent_id;
    let still_shared = inner
        .running
        .values()
        .any(|other| &other.stored.agent_id == agent_id);
    if !still_shared {
        if let Some(feed) = core.chats().heard(agent_id) {
            guests_gone(core, agent_id, feed.clear_guests());
        }
    }
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

pub fn list(core: &Core) -> Vec<ShareView> {
    let inner = core.shares().inner.locked();
    let mut views: Vec<ShareView> = inner.running.values().map(view_of).collect();
    views.sort_by(|a, b| a.id.cmp(&b.id));
    views
}

/// Takes up the shares a core before this one ran.
pub fn restore(core: &Arc<Core>) {
    let now = epoch_ms();
    let mut inner = core.shares().inner.locked();
    for stored in core.remote().shares() {
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

/// What a chat's head says that a share does not carry: the pages its turns changed, which nobody
/// viewing it can open, and the names and work of a lead's workers.
const NOT_SHARED: [&str; 2] = ["shown", "results"];

/// `payload` without what a share does not carry, read and written again only when it holds any
/// of it.
fn unshown(payload: &[u8]) -> Cow<'_, [u8]> {
    let holds = |name: &str| {
        let quoted = format!("\"{name}\"");
        payload
            .windows(quoted.len())
            .any(|window| window == quoted.as_bytes())
    };
    if !(NOT_SHARED.iter().chain(&["ruleChange", "chips"])).any(|name| holds(name)) {
        return Cow::Borrowed(payload);
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(payload) else {
        return Cow::Borrowed(payload);
    };
    strip_unshared(&mut value);
    serde_json::to_vec(&value).map_or(Cow::Borrowed(payload), Cow::Owned)
}

/// Takes out of `value` what a share does not carry: from every chat head, a view's head and the
/// fields a head change sets; a rule change, which holds the person's rule in their words, so its
/// row is a plain titled notice; and where each `why` chip opens, which becomes the unknown kind.
/// A chip's address still names what it points at.
fn strip_unshared(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let head_change = fields.get("op").and_then(Value::as_str) == Some("head");
            let head = match head_change {
                true => fields.get_mut("set"),
                false => fields.get_mut("head"),
            };
            if let Some(Value::Object(head)) = head {
                for name in NOT_SHARED {
                    head.remove(name);
                }
            }
            fields.remove("ruleChange");
            if let Some(Value::Array(chips)) = fields.get_mut("chips") {
                for chip in chips.iter_mut().filter_map(Value::as_object_mut) {
                    if chip.contains_key("href") && chip.contains_key("kind") {
                        chip.insert("open".into(), serde_json::json!({ "kind": "unknown" }));
                    }
                }
            }
            fields.values_mut().for_each(strip_unshared);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_unshared),
        _ => {}
    }
}

/// `reply` as a viewer is sent it, without what a share does not carry.
fn shared_reply(reply: &ShareReply) -> Option<Vec<u8>> {
    let mut value = serde_json::to_value(reply).ok()?;
    strip_unshared(&mut value);
    serde_json::to_vec(&value).ok()
}

/// A snapshot: the chat as it stands, from its last turns, without what a share does not carry.
fn snapshot(
    core: &Core,
    agent_id: &str,
    title: Option<String>,
    mut attachment: ChatAttachment,
) -> ShareFrame {
    match &mut attachment {
        ChatAttachment::Live { view, .. } => strip_unshared(view),
        ChatAttachment::Resumed { changes, .. } => strip_unshared(changes),
        ChatAttachment::Missing => {}
    }
    let info = core.chats().get(agent_id).map(|chat| chat.info());
    ShareFrame::Snapshot {
        chat: SharedChat {
            agent_id: agent_id.to_owned(),
            provider: info.as_ref().map(|info| info.provider.clone()),
            title: title.or_else(|| info.as_ref().and_then(|info| info.title.clone())),
            model: info.as_ref().and_then(|info| info.model.clone()),
            cwd: info.as_ref().map(|info| info.cwd.clone()),
            home: std::env::var_os("HOME").map(PathBuf::from),
            agents: crate::server::catalog::view(),
        },
        attachment,
    }
}

/// How every chat event the core sends its clients begins. Such a frame is
/// sealed as it came, with nothing read or written again.
const CHAT_EVENT: &[u8] = br#"{"type":"event","event":{"kind":"chat","#;

/// What a share publishes for a frame its client heard, if anything, and
/// whether it is a snapshot. A chat event goes out as the core wrote it; the
/// answer to the share's attach becomes a snapshot.
fn published<'a>(core: &Core, id: &str, frame: &'a [u8]) -> Option<(Cow<'a, [u8]>, bool)> {
    use sikemux_wire::protocol::{Event, FrameKind, Response, ServerMessage};
    let (kind, payload) = frame.get(4..)?.split_first()?;
    if *kind != FrameKind::Control as u8 {
        return None;
    }
    if payload.starts_with(CHAT_EVENT) {
        return Some((unshown(payload), false));
    }
    match serde_json::from_slice::<ServerMessage>(payload).ok()? {
        ServerMessage::Response {
            response: Response::ChatAttached { attachment },
            ..
        } => {
            let mut inner = core.shares().inner.locked();
            let running = inner.running.get_mut(id)?;
            running.feed = match &attachment {
                ChatAttachment::Live { mark, .. } | ChatAttachment::Resumed { mark, .. } => {
                    Some(mark.feed.clone())
                }
                ChatAttachment::Missing => None,
            };
            let agent_id = running.stored.agent_id.clone();
            let title = running.stored.title.clone();
            drop(inner);
            let frame = snapshot(core, &agent_id, title, attachment);
            Some((Cow::Owned(serde_json::to_vec(&frame).ok()?), true))
        }
        ServerMessage::Event {
            event: Event::Chat { .. },
        } => Some((unshown(payload), false)),
        _ => None,
    }
}

/// Asks the chat for everything a viewer starting now needs, which comes back
/// through the share's client as a snapshot.
fn take_snapshot(core: &Arc<Core>, id: &str) {
    let Some((client, agent_id)) =
        core.shares()
            .inner
            .locked()
            .running
            .get_mut(id)
            .map(|running| {
                running.chat = core
                    .chats()
                    .get(&running.stored.agent_id)
                    .map(|chat| Arc::as_ptr(&chat) as usize);
                (running.client.clone(), running.stored.agent_id.clone())
            })
    else {
        return;
    };
    let request_id = core.shares().next_request.fetch_add(1, Ordering::Relaxed);
    let core = core.clone();
    tokio::spawn(async move {
        chat::attach(&core, &client, request_id, &agent_id, None).await;
    });
}

/// A viewer's sealed question and the core's sealed answer, or nothing for a
/// question it cannot read.
async fn answer(core: Arc<Core>, id: String, viewer: String, sealed: String) -> Option<ToLink> {
    let (seal, agent_id) = {
        let inner = core.shares().inner.locked();
        let running = inner.running.get(&id)?;
        (running.seal.clone(), running.stored.agent_id.clone())
    };
    let ask: ShareAsk = serde_json::from_slice(&seal.open(&sealed)?).ok()?;
    let mut reply = ShareReply {
        id: ask.id(),
        ..ShareReply::default()
    };
    let failed = match ask {
        ShareAsk::Page { feed, before, .. } => {
            match chat::history(&core, &agent_id, &feed, before, PAGE_TURNS) {
                Ok(page) => {
                    reply.page = Some(page);
                    None
                }
                Err(error) => Some(error.to_string()),
            }
        }
        ShareAsk::ToolDetail { tool_call_id, .. } => {
            match chat::tool_detail(&core, &agent_id, &tool_call_id) {
                Ok(detail) => {
                    reply.tool_detail = Some(detail);
                    None
                }
                Err(error) => Some(error.to_string()),
            }
        }
        ShareAsk::File { path, .. } => match pasted_picture(&core, path).await {
            Ok(data) => {
                reply.file = Some(SharedFile { data });
                None
            }
            Err(error) => Some(error.to_string()),
        },
        ShareAsk::Message { text, name, .. } => guest_wrote(&core, &agent_id, name, text).err(),
    };
    reply.error = failed;
    let sealed = seal.seal(&shared_reply(&reply)?)?;
    Some(ToLink::Send(
        crate::accounts::share::ShareClientMessage::Answer(crate::accounts::share::ShareAnswer {
            share: id,
            viewer,
            sealed,
        }),
    ))
}

/// Keeps a viewer's message in the chat for the person to send or dismiss, and tells them.
fn guest_wrote(
    core: &Core,
    agent_id: &str,
    name: Option<String>,
    text: String,
) -> Result<(), String> {
    let name = name
        .map(|name| name.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|name| !name.is_empty());
    if name
        .as_ref()
        .is_some_and(|name| name.chars().count() > MAX_GUEST_NAME_CHARS)
    {
        return Err(format!(
            "a name holds up to {MAX_GUEST_NAME_CHARS} characters"
        ));
    }
    let text = text.trim();
    if text.is_empty() {
        return Err("the message is empty".into());
    }
    if text.chars().count() > MAX_GUEST_CHARS {
        return Err(format!(
            "a message holds up to {MAX_GUEST_CHARS} characters"
        ));
    }
    let feed = core
        .chats()
        .heard(agent_id)
        .ok_or("this chat is not running on its host right now")?;
    let guest = feed
        .add_guest(name, text.to_owned())
        .ok_or("too many messages wait for the sharer already")?;
    core.notify().send(Signal::Guest {
        agent_id: agent_id.to_owned(),
        guest,
    });
    core.changed(What::Agents);
    Ok(())
}

/// The person dealt with these viewers' messages, so what told them of each goes.
fn guests_gone(core: &Core, agent_id: &str, gone: impl IntoIterator<Item = GuestMessage>) {
    for guest in gone {
        core.notify().send(Signal::GuestCleared {
            agent_id: agent_id.to_owned(),
            id: guest.id,
        });
    }
    core.changed(What::Agents);
}

/// Sends a viewer's waiting message to the agent as the person's own.
pub async fn send_guest(core: &Arc<Core>, params: GuestMessageParams) -> CoreResult<()> {
    let guest = core
        .chats()
        .heard(&params.agent_id)
        .and_then(|feed| feed.take_guest(&params.id))
        .ok_or("that message is no longer waiting")?;
    let text = guest.text.clone();
    guests_gone(core, &params.agent_id, [guest]);
    chat::calls::prompt_chat(
        core,
        chat::calls::PromptChatParams::new(params.agent_id, text),
    )
    .await
}

pub fn dismiss_guest(core: &Core, params: GuestMessageParams) {
    let gone = core
        .chats()
        .heard(&params.agent_id)
        .and_then(|feed| feed.take_guest(&params.id));
    guests_gone(core, &params.agent_id, gone);
}

/// A picture pasted into a chat, in base64. Viewers may see only those, never
/// another file on this computer.
async fn pasted_picture(core: &Core, path: PathBuf) -> CoreResult<String> {
    use base64::Engine;
    let dir = core
        .attachment_dir()
        .clone()
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

pub fn share_chat_for(core: &Arc<Core>, params: ShareChatParams) -> CoreResult<ShareView> {
    share_chat(core, &params.agent_id, params.lifetime_ms, params.title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_view::chat::{
        AgentNotice, ChatMessage, ChatPage, ChatPart, NoticeSeverity, RuleChange, WhyChip, WhyKind,
        WhyOpen,
    };

    fn answer() -> ChatMessage {
        let notice = AgentNotice {
            severity: NoticeSeverity::Info,
            title: "Rule changed".into(),
            description: None,
            goal: None,
            rule: None,
            overruled: None,
            week: None,
            rule_change: Some(RuleChange {
                project: "storefront".into(),
                rule: 7,
                text: "never ship on fridays".into(),
                commit: None,
                was: Some("avoid shipping late in the week".into()),
                by: "Rohan".into(),
                restored: false,
                back_by: None,
            }),
            decided: None,
        };
        let chip = WhyChip {
            href: "sikemux://why/commit/c0ffee1234".into(),
            kind: WhyKind::Change,
            at: Some(1_700_000_000_000),
            landed: true,
            open: WhyOpen::Commit {
                sha: "c0ffee1234".into(),
            },
        };
        let part = ChatPart::AgentNotice {
            id: "p1".into(),
            notice,
        };
        serde_json::from_value(serde_json::json!({
            "id": "m1", "role": "assistant", "parts": [part], "chips": [chip],
        }))
        .unwrap()
    }

    /// What a share sent holds no rule words, its chips open nothing, and what the page draws stays.
    fn check(shared: &[u8]) {
        let row: Value = serde_json::from_slice(shared).unwrap();
        let text = row.to_string();
        for private in ["never ship on fridays", "late in the week", "ruleChange"] {
            assert!(
                !text.contains(private),
                "{private} reached the share: {text}"
            );
        }
        let chips = text.matches("\"open\":{\"kind\":\"unknown\"}").count();
        assert_eq!(chips, 1, "a chip still opens something: {text}");
        for drawn in [
            "Rule changed",
            "\"kind\":\"change\"",
            "\"landed\":true",
            "1700000000000",
        ] {
            assert!(text.contains(drawn), "{drawn} left the share: {text}");
        }
    }

    #[test]
    fn a_share_carries_no_rule_words_and_its_chips_open_nothing() {
        let event = serde_json::json!({"type": "event", "event": {"kind": "chat",
            "rows": [{"op": "put", "row": answer()}]}});
        check(&unshown(&serde_json::to_vec(&event).unwrap()));
        let reply = ShareReply {
            id: 1,
            page: Some(ChatPage {
                rows: vec![answer()],
                ..ChatPage::default()
            }),
            ..ShareReply::default()
        };
        check(&shared_reply(&reply).unwrap());
    }
}
