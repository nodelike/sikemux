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

/// The variable each provider reads its account directory from.
pub fn directory_variable(provider: &str) -> Option<&'static str> {
    match provider {
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        "codex" => Some("CODEX_HOME"),
        _ => None,
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The directory the provider keeps its account in under `env`, which wins
/// over the core's own environment.
pub fn directory(provider: &str, env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let variable = directory_variable(provider)?;
    let named = env
        .get(variable)
        .cloned()
        .or_else(|| std::env::var(variable).ok())
        .filter(|value| !value.trim().is_empty());
    Some(match (named, provider) {
        (Some(path), _) => PathBuf::from(path),
        (None, "claude") => home().join(".claude"),
        (None, _) => home().join(".codex"),
    })
}

/// Who the provider is signed in as: an account id, or `SignedOut`. `None`
/// when that cannot be told, such as for a provider without account files or
/// a file caught halfway through being written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignIn {
    Account(String),
    SignedOut,
}

pub fn signed_in(provider: &str, env: &BTreeMap<String, String>) -> Option<SignIn> {
    let directory = directory(provider, env)?;
    match provider {
        "claude" => {
            // The default account keeps this file beside its directory, not in it.
            let named = env.contains_key("CLAUDE_CONFIG_DIR")
                || std::env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|value| !value.is_empty());
            let file = if named {
                directory.join(".claude.json")
            } else {
                home().join(".claude.json")
            };
            claude_sign_in(&read_json(&file)?)
        }
        "codex" => codex_sign_in(&read_json(&directory.join("auth.json"))?),
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

/// What a failed turn needs from the person, or from the chat on their behalf.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// The account is signed out, or its sign-in no longer works.
    SignIn,
    /// The account ran out of usage, for now or until its plan resets.
    Limit,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub kind: FailureKind,
    pub title: String,
    pub details: Option<String>,
}

impl Failure {
    pub fn sign_in(title: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::SignIn,
            title: title.into(),
            details: None,
        }
    }

    /// The chat's `error` event for it, naming the account it happened on.
    pub fn payload(&self, account: Option<&str>) -> Value {
        let message = match &self.details {
            Some(details) if !details.is_empty() && details != &self.title => {
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
    Some(Failure {
        kind,
        title,
        details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let payload = failure.payload(Some("work"));
        assert_eq!(payload["message"], "Limit reached\nResets at 3pm");
        assert_eq!(payload["failure"]["kind"], "limit");
        assert_eq!(payload["failure"]["account"], "work");
    }
}
