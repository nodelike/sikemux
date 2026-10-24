//! Starting a chat's agent again under the chat: on another account when its
//! own ran out of usage, or after a sign-in, with the prompt that failed sent
//! again once the agent is back.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::acp::account::{self, SignIn};
use crate::acp::SessionEnd;
use crate::protocol::{ChatAccount, ChatContext, ChatLaunch};

use super::super::Core;
use super::Chat;

/// How a connection ended: the chat's session is over, or its agent is to be
/// started again under it.
pub(super) enum Outcome {
    Ended(SessionEnd),
    Rebind(Box<Rebind>),
    /// The agent was let go of, still running, for a core that replaces
    /// this one to take up.
    Released,
}

/// A prompt to send again once the agent is back.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Replay {
    /// Clients have not been shown it yet.
    pub announce: bool,
    pub message_id: Option<String>,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
}

/// Starts the chat's agent again on the same session.
pub(super) struct Rebind {
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
pub(super) struct Carried {
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
pub(super) fn on_account(provider: &str, launch: &ChatLaunch, account: &ChatAccount) -> ChatLaunch {
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
pub(super) fn name_accounts(core: &Core, launch: &mut ChatLaunch) {
    if !launch.fallbacks.is_empty() {
        return;
    }
    let Some(book) = core.workspaces.accounts() else {
        return;
    };
    let folder = crate::launch::named_account(&launch.provider, &launch.env);
    let (current, others) = book.chat_accounts(&launch.provider, folder);
    launch.account = launch.account.take().or(current);
    launch.fallbacks = others;
}

/// Whether the person lets a chat of this agent move to another account
/// when its own runs out of usage.
pub(super) fn switches_at_limit(chat: &Chat) -> bool {
    chat.core
        .upgrade()
        .and_then(|core| core.workspaces.accounts())
        .is_some_and(|book| book.auto_switch.get(chat.provider()) == Some(&true))
}

/// The first account to move to that has usage left to try and is signed in.
pub(super) fn next_account<'a>(
    provider: &str,
    launch: &'a ChatLaunch,
    exhausted: &[String],
) -> Option<&'a ChatAccount> {
    launch.fallbacks.iter().find(|fallback| {
        !exhausted.contains(&fallback.id)
            && account::signed_in(provider, &on_account(provider, launch, fallback).env)
                != Some(SignIn::SignedOut)
    })
}

pub(super) fn switch_notice(launch: &ChatLaunch, to: &ChatAccount, reason: &str) -> Value {
    json!({
        "sessionUpdate": "account_switched",
        "account": to.id,
        "label": to.label,
        "from": launch.account.as_ref().map(|account| account.label.clone()),
        "reason": reason,
    })
}

pub(super) fn error_message(message: impl std::fmt::Display) -> Value {
    json!({ "message": message.to_string() })
}
