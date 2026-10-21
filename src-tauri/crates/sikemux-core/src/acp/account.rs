//! Which account a chat agent signs in as, and what an agent says when a
//! turn fails because of that account.
//!
//! Claude Code and Codex each keep one account per directory, named by an
//! environment variable. The account is read from the files their own CLIs
//! write, so a sign-in made in a terminal is noticed before the next turn.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};

use crate::agent_catalog::AgentKind;
use crate::launch::account_variable;
use crate::sessions::roots::{account_root, environment_path};

fn home() -> PathBuf {
    sikemux_process::paths::home().unwrap_or_default()
}

/// Who the provider is signed in as: an account id, or `SignedOut`. `None`
/// when that cannot be told, such as for a provider without account files or
/// a file caught halfway through being written.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SignIn {
    Account(String),
    SignedOut,
}

pub fn signed_in(provider: &str, env: &BTreeMap<String, String>) -> Option<SignIn> {
    let kind = AgentKind::from_name(provider)?;
    let variable = account_variable(provider)?;
    let directory = account_root(kind, None, env)?;
    match kind {
        AgentKind::Claude => {
            // The default account keeps this file beside its directory, not in it.
            let named = env.contains_key(variable) || environment_path(variable).is_some();
            let file = if named {
                directory.join(".claude.json")
            } else {
                home().join(".claude.json")
            };
            claude_sign_in(&read_json(&file)?)
        }
        AgentKind::Codex => codex_sign_in(&read_json(&directory.join("auth.json"))?),
        _ => None,
    }
}

/// A missing file is a known answer, signed out. An unreadable one is not.
fn read_json(path: &std::path::Path) -> Option<Value> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).ok(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Value::Null),
        Err(_) => None,
    }
}

fn claude_sign_in(config: &Value) -> Option<SignIn> {
    Some(
        match config
            .pointer("/oauthAccount/accountUuid")
            .and_then(Value::as_str)
        {
            Some(account) if !account.is_empty() => SignIn::Account(account.to_owned()),
            _ => SignIn::SignedOut,
        },
    )
}

fn codex_sign_in(auth: &Value) -> Option<SignIn> {
    if let Some(account) = auth
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .filter(|account| !account.is_empty())
    {
        return Some(SignIn::Account(account.to_owned()));
    }
    let has_key = auth
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .is_some_and(|key| !key.is_empty());
    Some(if has_key {
        SignIn::Account("api-key".into())
    } else {
        SignIn::SignedOut
    })
}

pub const LOST_CONNECTION: &str = "Lost the connection to the model provider";

/// Words an agent's error uses when the network, not the request, was at fault.
const CONNECTION_WORDS: [&str; 10] = [
    "connection error",
    "connection refused",
    "connection reset",
    "econnreset",
    "econnrefused",
    "enotfound",
    "etimedout",
    "fetch failed",
    "socket hang up",
    "network is unreachable",
];

/// Whether an agent's error reads as the network failing it.
pub fn sounds_offline(said: &str) -> bool {
    let said = said.to_lowercase();
    CONNECTION_WORDS.iter().any(|words| said.contains(words))
}

/// What a failed turn needs from the person, or from the chat on their behalf.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// The account is signed out, or its sign-in no longer works.
    SignIn,
    /// The account ran out of usage, for now or until its plan resets.
    Limit,
    /// The network or the model provider could not be reached; the same
    /// prompt can go again once it is back.
    Network,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    pub title: String,
    pub details: Option<String>,
    /// When the account can be used again, in Unix milliseconds, when the
    /// agent says.
    pub resets_at: Option<u64>,
}

impl Failure {
    pub fn sign_in(title: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::SignIn,
            title: title.into(),
            details: None,
            resets_at: None,
        }
    }

    /// A turn the network cut short, worded for the person, with what the
    /// agent said kept for whoever opens the details.
    pub fn lost_connection(said: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Network,
            title: LOST_CONNECTION.into(),
            details: Some(said.into()).filter(|said: &String| !said.trim().is_empty()),
            resets_at: None,
        }
    }

    /// The chat's `error` event for it, naming the account it happened on.
    /// A lost connection says so plainly; the agent's own words stay in `details`.
    pub fn payload(&self, account: Option<&str>) -> Value {
        let message = match &self.details {
            Some(details)
                if !details.is_empty()
                    && details != &self.title
                    && self.kind != FailureKind::Network =>
            {
                format!("{}\n{}", self.title, details)
            }
            _ => self.title.clone(),
        };
        json!({
            "message": message,
            "failure": {
                "kind": self.kind,
                "title": self.title,
                "details": self.details,
                "account": account,
            },
        })
    }
}

/// The update that says the agent is retrying a request the network or the
/// service failed, as a warning in the same AIR shape ACP agents send. It
/// holds until the agent streams again or the turn ends.
pub fn retrying(id: &str, category: &str, attempt: u64, detail: &str) -> Value {
    json!({
        "sessionUpdate": "session_info_update",
        "_meta": { "jetbrains": { "air": { "version": 1, "sessionFailure": {
            "id": id,
            "revision": attempt,
            "category": category,
            "severity": "warning",
            "title": detail,
            "attempt": attempt,
            "actions": [],
        } } } },
    })
}

/// The failure an ACP agent attached to a prompt's answer or a session update.
///
/// It comes as the `sessionFailure` part of the AIR extension, once the client
/// says it understands it. Its kind is not sent, only a category and the
/// actions that would fix it.
pub fn failure(meta: &Value) -> Option<Failure> {
    let failure = meta.pointer("/jetbrains/air/sessionFailure")?;
    if failure.get("severity").and_then(Value::as_str) == Some("warning") {
        return None;
    }
    let category = failure.get("category").and_then(Value::as_str);
    let offers = |action: &str| {
        failure
            .get("actions")
            .and_then(Value::as_array)
            .is_some_and(|actions| actions.iter().any(|item| item == action))
    };
    let kind = match category {
        Some("access") if offers("login") => FailureKind::SignIn,
        // A context or budget limit belongs to the session, not the account,
        // and its way out is a new session rather than another account.
        Some("limit") if !offers("new_session") => FailureKind::Limit,
        Some("connection") => FailureKind::Network,
        _ => FailureKind::Other,
    };
    let title = failure
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .unwrap_or("The agent could not finish this turn")
        .to_owned();
    let details = failure
        .get("details")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if kind == FailureKind::Network {
        return Some(Failure::lost_connection(details.unwrap_or(title)));
    }
    Some(Failure {
        kind,
        title,
        details,
        resets_at: failure.get("resetsAt").and_then(Value::as_u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_failure_is_the_networks_and_reads_plainly() {
        let lost = failure(&air(json!({
            "id": "t:error", "revision": 1, "category": "connection", "severity": "error",
            "title": "stream disconnected before completion", "actions": ["retry", "new_session"],
        })))
        .unwrap();
        assert_eq!(lost.kind, FailureKind::Network);
        assert_eq!(lost.title, LOST_CONNECTION);
        assert_eq!(
            lost.details.as_deref(),
            Some("stream disconnected before completion")
        );
        let payload = lost.payload(None);
        assert_eq!(payload["message"], LOST_CONNECTION);
        assert_eq!(payload["failure"]["kind"], "network");
    }

    #[test]
    fn an_agents_words_say_when_the_network_failed_it() {
        assert!(sounds_offline("API Error: Connection error."));
        assert!(sounds_offline(
            "request to https://api.anthropic.com failed: getaddrinfo ENOTFOUND"
        ));
        assert!(sounds_offline("TypeError: fetch failed"));
        assert!(!sounds_offline("Invalid request: prompt is too long"));
    }

    #[test]
    fn a_retry_is_a_warning_that_no_failure_is_read_from() {
        let update = retrying("turn-1:retry", "connection", 3, "Connection error");
        assert_eq!(update["sessionUpdate"], "session_info_update");
        let notice = &update["_meta"]["jetbrains"]["air"]["sessionFailure"];
        assert_eq!(notice["severity"], "warning");
        assert_eq!(notice["attempt"], 3);
        assert!(failure(&update["_meta"]).is_none());
    }

    fn air(failure: Value) -> Value {
        json!({ "jetbrains": { "air": { "version": 1, "sessionFailure": failure } } })
    }

    #[test]
    fn a_usage_limit_is_the_accounts_and_a_context_limit_the_sessions() {
        let quota = air(json!({
            "id": "t:error", "revision": 1, "category": "limit", "severity": "error",
            "title": "You've hit your limit · resets 3pm", "actions": [],
        }));
        let failure = failure(&quota).unwrap();
        assert_eq!(failure.kind, FailureKind::Limit);
        assert_eq!(failure.title, "You've hit your limit · resets 3pm");

        let rate = air(
            json!({ "category": "limit", "severity": "error", "title": "Slow down", "actions": ["retry"] }),
        );
        assert_eq!(super::failure(&rate).unwrap().kind, FailureKind::Limit);

        let context = air(
            json!({ "category": "limit", "severity": "error", "title": "Full", "actions": ["new_session"] }),
        );
        assert_eq!(super::failure(&context).unwrap().kind, FailureKind::Other);
    }

    #[test]
    fn a_sign_in_failure_offers_a_login() {
        let signed_out = air(
            json!({ "category": "access", "severity": "error", "title": "Sign in", "actions": ["login"] }),
        );
        assert_eq!(failure(&signed_out).unwrap().kind, FailureKind::SignIn);
        let denied = air(
            json!({ "category": "access", "severity": "error", "title": "Verify", "actions": ["retry"] }),
        );
        assert_eq!(failure(&denied).unwrap().kind, FailureKind::Other);
    }

    #[test]
    fn a_retry_warning_is_no_failure() {
        let warning = air(
            json!({ "category": "limit", "severity": "warning", "title": "Retrying", "actions": [] }),
        );
        assert_eq!(failure(&warning), None);
        assert_eq!(failure(&json!({ "quota": {} })), None);
    }

    #[test]
    fn claude_is_signed_in_as_its_oauth_account() {
        assert_eq!(
            claude_sign_in(
                &json!({ "oauthAccount": { "accountUuid": "a-1", "emailAddress": "me@x" } })
            ),
            Some(SignIn::Account("a-1".into()))
        );
        assert_eq!(
            claude_sign_in(&json!({ "numStartups": 3 })),
            Some(SignIn::SignedOut)
        );
        assert_eq!(claude_sign_in(&Value::Null), Some(SignIn::SignedOut));
    }

    #[test]
    fn codex_is_signed_in_as_its_chatgpt_account_or_a_key() {
        assert_eq!(
            codex_sign_in(&json!({ "auth_mode": "chatgpt", "tokens": { "account_id": "acct-9" } })),
            Some(SignIn::Account("acct-9".into()))
        );
        assert_eq!(
            codex_sign_in(&json!({ "OPENAI_API_KEY": "sk-1", "tokens": null })),
            Some(SignIn::Account("api-key".into()))
        );
        assert_eq!(
            codex_sign_in(&json!({ "OPENAI_API_KEY": null })),
            Some(SignIn::SignedOut)
        );
    }

    #[test]
    fn a_named_directory_holds_the_account_files() {
        let dir = tempfile::tempdir().unwrap();
        let env = BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".to_string(),
            dir.path().to_string_lossy().into_owned(),
        )]);
        assert_eq!(signed_in("claude", &env), Some(SignIn::SignedOut));
        std::fs::write(
            dir.path().join(".claude.json"),
            r#"{"oauthAccount":{"accountUuid":"work"}}"#,
        )
        .unwrap();
        assert_eq!(
            signed_in("claude", &env),
            Some(SignIn::Account("work".into()))
        );
        std::fs::write(dir.path().join(".claude.json"), "{\"oauthAcc").unwrap();
        assert_eq!(signed_in("claude", &env), None);
        assert_eq!(signed_in("opencode", &env), None);
    }

    #[test]
    fn a_failure_payload_keeps_the_message_the_banner_shows() {
        let failure = Failure {
            kind: FailureKind::Limit,
            title: "Limit reached".into(),
            details: Some("Resets at 3pm".into()),
            resets_at: None,
        };
        let payload = failure.payload(Some("work"));
        assert_eq!(payload["message"], "Limit reached\nResets at 3pm");
        assert_eq!(payload["failure"]["kind"], "limit");
        assert_eq!(payload["failure"]["account"], "work");
    }
}
