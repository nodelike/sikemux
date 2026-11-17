//! Which account a chat agent signs in as. Claude Code and Codex each keep
//! one account per directory, named by an environment variable. The account
//! is read from the files their own CLIs write, so a sign-in made in a
//! terminal is noticed before the next turn.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::agent_accounts::folders::claude_config_file;
use crate::agent_catalog::AgentKind;
use crate::sessions::roots::account_root;

/// Who the provider is signed in as: an account id, or `SignedOut`. `None`
/// when that cannot be told, such as for a provider without account files or
/// a file caught halfway through being written.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SignIn {
    Account(String),
    SignedOut,
}

pub fn signed_in(provider: &str, env: &BTreeMap<String, String>) -> Option<SignIn> {
    AgentKind::from_name(provider)?.ops().signed_in(env)
}

pub(crate) fn claude_signed_in(env: &BTreeMap<String, String>) -> Option<SignIn> {
    claude_sign_in(&read_json(&claude_config_file(None, env))?)
}

pub(crate) fn codex_signed_in(env: &BTreeMap<String, String>) -> Option<SignIn> {
    let directory = account_root(AgentKind::Codex, None, env)?;
    codex_sign_in(&read_json(&directory.join("auth.json"))?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
