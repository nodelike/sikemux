//! Starting a chat's agent again under the chat: on another account when its
//! own ran out of usage, or after a sign-in, with the prompt that failed sent
//! again once the agent is back.

use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceParts;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::UpdateKind;

use crate::agent_accounts::sign_in::{self, SignIn};

use super::native::SessionEnd;
use sikemux_wire::protocol::{ChatAccount, ChatContext, ChatLaunch};

use super::super::Core;
use super::Chat;

/// How a connection ended: the chat's session is over, or its agent is to be
/// started again under it.
pub enum Outcome {
    Ended(SessionEnd),
    Rebind(Box<Rebind>),
    /// The agent was let go of, still running, for a core that replaces
    /// this one to take up.
    Released,
}

/// A prompt to send again once the agent is back.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Replay {
    /// Clients have not been shown it yet.
    pub announce: bool,
    pub message_id: Option<String>,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
    /// The person sent it, rather than another agent or Sikemux.
    #[serde(default)]
    pub person: bool,
    /// It answered a call to the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<super::Answered>,
    /// The fresh session the brief in its context was built for, the only one it goes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief_for: Option<String>,
}

impl Replay {
    pub fn whose(&self) -> super::Whose {
        super::Whose {
            person: self.person,
            answered: self.answered,
        }
    }
}

/// Starts the chat's agent again on the same session.
pub struct Rebind {
    pub launch: ChatLaunch,
    pub replay: Option<Replay>,
    /// Accounts that ran out of usage on the prompt being sent again.
    pub exhausted: Vec<String>,
    /// Set after a sign-in failure, so a second one is shown rather than retried.
    pub signed_in_again: bool,
    /// Said in the transcript once the agent is back.
    pub notice: Option<Value>,
}

/// What a connection carries over from the one before it.
pub struct Carried {
    pub replay: Option<Replay>,
    pub exhausted: Vec<String>,
    pub signed_in_again: bool,
    pub notice: Option<Value>,
    /// A session loaded again under a running chat replays history the chat
    /// already shows.
    pub quiet_load: bool,
}

impl Carried {
    pub fn from(rebind: Option<Box<Rebind>>) -> Self {
        match rebind {
            Some(rebind) => Self {
                replay: rebind.replay,
                exhausted: rebind.exhausted,
                signed_in_again: rebind.signed_in_again,
                notice: rebind.notice,
                quiet_load: true,
            },
            None => Self {
                replay: None,
                exhausted: Vec::new(),
                signed_in_again: false,
                notice: None,
                quiet_load: false,
            },
        }
    }
}

/// `launch` with `account` in place of the account it names.
pub fn on_account(provider: &str, launch: &ChatLaunch, account: &ChatAccount) -> ChatLaunch {
    let mut moved = launch.clone();
    if let Some(variable) = crate::launch::account_variable(provider) {
        moved.env.remove(variable);
    }
    moved.env.extend(account.env.clone());
    moved.fallbacks.retain(|fallback| fallback.id != account.id);
    if let Some(previous) = moved.account.replace(account.clone()) {
        if previous.id != account.id {
            moved.fallbacks.push(previous);
        }
    }
    moved
}

/// Names, from the accounts the core keeps, the account `launch` runs on and
/// the person's others it may move to, where the launch does not say.
pub fn name_accounts(core: &Core, launch: &mut ChatLaunch) {
    if !launch.fallbacks.is_empty() {
        return;
    }
    let book = core.workspaces().accounts().unwrap_or_default();
    let folder = crate::launch::named_account(&launch.provider, &launch.env);
    let current = match &launch.account {
        Some(account) => Some(account.id.clone()),
        None => book
            .account_in(&launch.provider, folder)
            .map(|profile| profile.id),
    };
    let takers = book.takers(&launch.provider, current.as_deref(), true);
    launch.account = launch.account.take().or(takers.current);
    launch.fallbacks = takers.others;
}

/// Whether the person lets a chat move to another of their accounts with its agent when its
/// own runs out of usage: while smart accounts is on, as it is unless they turned it off.
pub fn switches_at_limit(chat: &Chat) -> bool {
    chat.core
        .upgrade()
        .is_none_or(|core| core.workspaces().accounts().unwrap_or_default().smart)
}

/// The account to move to that has usage left to try and is signed in: of those, the one with
/// the most of its week left, so the chat can keep going, else the first.
pub fn next_account<'a>(
    chat: &Chat,
    provider: &str,
    launch: &'a ChatLaunch,
    exhausted: &[String],
) -> Option<&'a ChatAccount> {
    let book = chat
        .core
        .upgrade()
        .and_then(|core| core.workspaces().accounts());
    let open: Vec<&ChatAccount> = (launch.fallbacks.iter())
        .filter(|fallback| {
            !exhausted.contains(&fallback.id)
                && book
                    .as_ref()
                    .is_none_or(|book| book.allowed_in(&launch.cwd, &fallback.id))
                && sign_in::signed_in(provider, &on_account(provider, launch, fallback).env)
                    != Some(SignIn::SignedOut)
        })
        .collect();
    let placed = chat.core.upgrade().and_then(|core| {
        (core.chat_above().account_with_room)(&core, provider, launch, &|profile| {
            open.iter().any(|fallback| fallback.id == profile)
        })
    });
    let best = placed.and_then(|placed| open.iter().find(|fallback| fallback.id == placed));
    best.or(open.first()).copied()
}

pub fn switch_notice(launch: &ChatLaunch, to: &ChatAccount, reason: &str) -> Value {
    json!({
        "sessionUpdate": UpdateKind::AccountSwitched,
        "account": to.id,
        "label": to.label,
        "from": launch.account.as_ref().map(|account| account.label.clone()),
        "reason": reason,
    })
}

pub fn error_message(message: impl std::fmt::Display) -> Value {
    json!({ "message": message.to_string() })
}
