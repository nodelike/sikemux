//! Programs Sikemux did not start that drive its chats: an MCP client such as Claude Code or
//! Codex in the person's own terminal, or the `sikemux chat` command run outside every agent.
//! Each signs in once, the person allows it in the app with read-only or full access, and it
//! then calls with a token that lasts 30 days until the person removes it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::changed::Topic;
use sikemux_wire::protocol::DeviceAccess;
use tokio::sync::watch;

use super::changed::What;
use super::{Core, CoreError, CoreResult};

pub(crate) mod chats;
pub(crate) mod cli;
pub(crate) mod http;
mod oauth;

const FILE: &str = "outside-clients.json";
const SIGN_IN_LASTS: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How long the person has to answer, as a phone waiting to join gets.
pub(crate) const ANSWER_WITHIN: Duration = Duration::from_secs(120);
/// More waiting at once than one person starting sign-ins could mean; the rest are refused.
const MAX_WAITING: usize = 4;
const MAX_CLIENTS: usize = 64;
const MAX_REGISTRATIONS: usize = 64;
pub(crate) const NAME_LIMIT: usize = 64;
const TOKEN_PREFIX: &str = "skx_";

/// How an outside program reaches the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    /// The MCP endpoint, signed in through the browser.
    Mcp,
    /// `sikemux chat`, run outside every agent.
    Cli,
}

/// A program the person let in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideClient {
    pub id: String,
    /// What the program called itself. Nothing vouches for it.
    pub name: String,
    pub via: Via,
    /// Where its sign-in returned to, for one that signed in through the browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
    pub access: DeviceAccess,
    pub approved_at: u64,
    pub expires_at: u64,
    #[serde(default)]
    pub last_used: Option<u64>,
    #[serde(skip_serializing)]
    #[serde(default)]
    token_hash: String,
}

/// A program waiting for the person to let it in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideWaiting {
    pub id: String,
    pub name: String,
    pub via: Via,
    pub redirect: Option<String>,
    /// Four digits the sign-in shows where it started, to match against the app's question.
    pub check: String,
    pub expires_at: u64,
}

/// What the app shows of outside access.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideView {
    /// The MCP endpoint's address, once it listens.
    pub url: Option<String>,
    /// Why it does not listen.
    pub error: Option<String>,
    pub waiting: Vec<OutsideWaiting>,
    pub clients: Vec<OutsideClient>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    clients: Vec<OutsideClient>,
    registrations: Vec<oauth::Registration>,
}

/// The person's answer to a waiting program, once given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Waiting,
    Allowed(DeviceAccess),
    Declined,
    TimedOut,
}

struct Waiting {
    shown: OutsideWaiting,
    answer: Answer,
}

#[derive(Default)]
struct Inner {
    stored: Stored,
    waiting: Vec<Waiting>,
    codes: HashMap<String, oauth::Code>,
    /// What each MCP client waiting in the browser asked, by the id of its wait.
    authorizing: HashMap<String, oauth::Authorize>,
    url: Option<String>,
    error: Option<String>,
}

pub(crate) struct Outside {
    path: Option<PathBuf>,
    inner: Mutex<Inner>,
    /// Ticks each time an answer comes in, for sign-ins that wait on one.
    answered: watch::Sender<u64>,
}

/// Who is calling, by a token the person let in.
#[derive(Clone, Debug)]
pub(crate) struct Caller {
    pub name: String,
    pub access: DeviceAccess,
}

impl Outside {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let path = data_dir.map(|dir| dir.join(FILE));
        let stored = path
            .as_deref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(Inner {
                stored,
                ..Inner::default()
            }),
            answered: watch::channel(0).0,
        }
    }

    fn save(&self, stored: &Stored) {
        let Some(path) = &self.path else {
            return;
        };
        match serde_json::to_vec_pretty(stored) {
            Ok(bytes) => {
                if let Err(error) = sikemux_process::files::write_atomic(path, &bytes, true) {
                    eprintln!("sikemux core: could not save outside clients: {error}");
                }
            }
            Err(error) => eprintln!("sikemux core: could not save outside clients: {error}"),
        }
    }

    pub(crate) fn view(&self) -> OutsideView {
        let now = epoch_ms();
        let inner = self.inner.locked();
        OutsideView {
            url: inner.url.clone(),
            error: inner.error.clone(),
            waiting: inner
                .waiting
                .iter()
                .filter(|waiting| waiting.answer == Answer::Waiting)
                .filter(|waiting| waiting.shown.expires_at > now)
                .map(|waiting| waiting.shown.clone())
                .collect(),
            clients: inner
                .stored
                .clients
                .iter()
                .filter(|client| client.expires_at > now)
                .cloned()
                .collect(),
        }
    }

    pub(crate) fn set_listening(&self, url: Option<String>, error: Option<String>) {
        let mut inner = self.inner.locked();
        inner.url = url;
        inner.error = error;
    }

    /// Puts `name` before the person, and answers with the id its answer comes under.
    pub(crate) fn ask(&self, name: &str, via: Via, redirect: Option<String>) -> CoreResult<String> {
        let now = epoch_ms();
        let mut inner = self.inner.locked();
        inner.waiting.retain(|waiting| {
            waiting.answer != Answer::Waiting || waiting.shown.expires_at + 60_000 > now
        });
        let open = inner
            .waiting
            .iter()
            .filter(|waiting| waiting.answer == Answer::Waiting && waiting.shown.expires_at > now)
            .count();
        if open >= MAX_WAITING {
            return Err(CoreError::from(
                "too many programs are waiting to sign in to Sikemux; try again in two minutes",
            ));
        }
        let id = random_text(16)?;
        inner.waiting.push(Waiting {
            shown: OutsideWaiting {
                id: id.clone(),
                name: clean_name(name),
                via,
                redirect,
                check: check_digits()?,
                expires_at: now + ANSWER_WITHIN.as_millis() as u64,
            },
            answer: Answer::Waiting,
        });
        Ok(id)
    }

    /// The program waiting under `id`, while the person has not answered it.
    pub(crate) fn waiting_shown(&self, id: &str) -> Option<OutsideWaiting> {
        let now = epoch_ms();
        let inner = self.inner.locked();
        inner
            .waiting
            .iter()
            .find(|waiting| waiting.shown.id == id && waiting.answer == Answer::Waiting)
            .filter(|waiting| waiting.shown.expires_at > now)
            .map(|waiting| waiting.shown.clone())
    }

    /// What the person answered `id`, or that they still have not.
    pub(crate) fn answer_of(&self, id: &str) -> Answer {
        let now = epoch_ms();
        let inner = self.inner.locked();
        match inner.waiting.iter().find(|waiting| waiting.shown.id == id) {
            Some(waiting)
                if waiting.answer == Answer::Waiting && waiting.shown.expires_at <= now =>
            {
                Answer::TimedOut
            }
            Some(waiting) => waiting.answer,
            None => Answer::TimedOut,
        }
    }

    /// Waits up to `limit` for the person to answer `id`.
    pub(crate) async fn wait_for_answer(&self, id: &str, limit: Duration) -> Answer {
        let mut answered = self.answered.subscribe();
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let answer = self.answer_of(id);
            if answer != Answer::Waiting {
                return answer;
            }
            let woke = tokio::time::timeout_at(deadline, answered.changed()).await;
            if !matches!(woke, Ok(Ok(()))) {
                return self.answer_of(id);
            }
        }
    }

    /// The person's answer to `id`, from the app.
    pub(crate) fn answer(&self, id: &str, access: Option<DeviceAccess>) -> CoreResult<()> {
        let now = epoch_ms();
        {
            let mut inner = self.inner.locked();
            let waiting = inner
                .waiting
                .iter_mut()
                .find(|waiting| waiting.shown.id == id && waiting.answer == Answer::Waiting)
                .filter(|waiting| waiting.shown.expires_at > now)
                .ok_or_else(|| CoreError::from("that program is no longer waiting"))?;
            waiting.answer = access.map_or(Answer::Declined, Answer::Allowed);
        }
        self.answered.send_modify(|count| *count += 1);
        Ok(())
    }

    /// Takes the answer to `id` once it allowed it, so it is used once.
    fn take_allowed(&self, id: &str) -> Option<(OutsideWaiting, DeviceAccess)> {
        let mut inner = self.inner.locked();
        let index = inner
            .waiting
            .iter()
            .position(|waiting| waiting.shown.id == id)?;
        let Answer::Allowed(access) = inner.waiting[index].answer else {
            return None;
        };
        Some((inner.waiting.remove(index).shown, access))
    }

    /// Lets in the program the person allowed under `id`, and answers with its token.
    pub(crate) fn admit(&self, id: &str) -> CoreResult<(String, OutsideClient)> {
        let (shown, access) = self
            .take_allowed(id)
            .ok_or_else(|| CoreError::from("the person has not allowed this sign-in"))?;
        self.add(shown.name, shown.via, shown.redirect, access)
    }

    fn add(
        &self,
        name: String,
        via: Via,
        redirect: Option<String>,
        access: DeviceAccess,
    ) -> CoreResult<(String, OutsideClient)> {
        let token = format!("{TOKEN_PREFIX}{}", random_text(32)?);
        let now = epoch_ms();
        let client = OutsideClient {
            id: random_text(12)?,
            name,
            via,
            redirect,
            access,
            approved_at: now,
            expires_at: now + SIGN_IN_LASTS.as_millis() as u64,
            last_used: None,
            token_hash: digest(&token),
        };
        let mut inner = self.inner.locked();
        let stored = &mut inner.stored;
        stored.clients.retain(|kept| kept.expires_at > now);
        if stored.clients.len() >= MAX_CLIENTS {
            stored.clients.remove(0);
        }
        stored.clients.push(client.clone());
        self.save(stored);
        Ok((token, client))
    }

    /// The program `token` lets in, if the person still lets it.
    pub(crate) fn caller(&self, token: &str) -> Option<Caller> {
        let hash = digest(token);
        let now = epoch_ms();
        let mut inner = self.inner.locked();
        let client = inner.stored.clients.iter_mut().find(|client| {
            crate::cli::auth::same_secret(&client.token_hash, &hash) && client.expires_at > now
        })?;
        let save = client
            .last_used
            .is_none_or(|used| now.saturating_sub(used) > 60_000);
        client.last_used = Some(now);
        let caller = Caller {
            name: client.name.clone(),
            access: client.access,
        };
        if save {
            self.save(&inner.stored);
        }
        Some(caller)
    }

    pub(crate) fn remove(&self, id: &str) -> CoreResult<()> {
        let mut inner = self.inner.locked();
        let before = inner.stored.clients.len();
        inner.stored.clients.retain(|client| client.id != id);
        if inner.stored.clients.len() == before {
            return Err(CoreError::from("no program with that id may use Sikemux"));
        }
        self.save(&inner.stored);
        Ok(())
    }

    pub(crate) fn set_access(&self, id: &str, access: DeviceAccess) -> CoreResult<()> {
        let mut inner = self.inner.locked();
        let client = inner
            .stored
            .clients
            .iter_mut()
            .find(|client| client.id == id)
            .ok_or_else(|| CoreError::from("no program with that id may use Sikemux"))?;
        client.access = access;
        self.save(&inner.stored);
        Ok(())
    }
}

impl super::part::Part for Outside {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![tokio::spawn(http::serve(Arc::downgrade(core)))]
    }
}

/// Tells the app what changed in outside access.
pub(crate) fn changed(core: &Core) {
    core.changed(What::Read(Topic::Outside));
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideAnswer {
    pub id: String,
    pub allow: bool,
    pub access: DeviceAccess,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideChange {
    pub id: String,
    /// None removes the program.
    #[serde(default)]
    pub access: Option<DeviceAccess>,
}

pub(crate) fn answer(core: &Core, answer: OutsideAnswer) -> CoreResult<OutsideView> {
    core.outside
        .answer(&answer.id, answer.allow.then_some(answer.access))?;
    changed(core);
    Ok(core.outside.view())
}

pub(crate) fn change(core: &Core, change: OutsideChange) -> CoreResult<OutsideView> {
    match change.access {
        Some(access) => core.outside.set_access(&change.id, access)?,
        None => core.outside.remove(&change.id)?,
    }
    changed(core);
    Ok(core.outside.view())
}

pub(crate) fn random_text(bytes: usize) -> CoreResult<String> {
    use base64::Engine;
    let mut random = vec![0u8; bytes];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| CoreError::from("could not make a random token"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random))
}

fn check_digits() -> CoreResult<String> {
    let mut random = [0u8; 4];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| CoreError::from("could not make a random token"))?;
    Ok(format!("{:04}", u32::from_le_bytes(random) % 10_000))
}

pub(crate) fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

pub(crate) fn clean_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|ch| !ch.is_control())
        .take(NAME_LIMIT)
        .collect();
    match name.trim() {
        "" => "An app".to_owned(),
        name => name.to_owned(),
    }
}
