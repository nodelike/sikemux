// Whether each agent can be used right now: found, working, and signed in.
// Every agent answers sign-in its own way, and some cannot answer at all, so
// `Unknown` is an honest answer rather than a failure.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::accounts::{agent_account_status, agent_command};
use super::AgentKind;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum AgentStatus {
    /// No binary was found.
    Missing,
    /// A binary was found but does not run.
    Broken { reason: String },
    /// Installed, with no usable credentials.
    SignedOut,
    /// Installed and signed in. `account` is the kind of sign-in when the CLI says, never who.
    #[serde(rename_all = "camelCase")]
    Ready { account: Option<String> },
    /// The agent has no reliable way to say whether it is signed in.
    Unknown,
}

/// How long an answer about sign-in is trusted. Checking starts the agent's CLI, so a list
/// redrawn every few seconds should not start them all again.
const STATUS_TTL: Duration = Duration::from_secs(60);

fn status_cache() -> &'static Mutex<HashMap<String, (Instant, AgentStatus)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, AgentStatus)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One account: an agent and the profile folder it keeps its sign-in in.
fn account_key(agent: AgentKind, config_path: Option<&str>) -> String {
    format!("{}\0{}", agent.as_str(), config_path.unwrap_or(""))
}

pub(super) fn cached_status(agent: AgentKind, config_path: Option<&str>) -> Option<AgentStatus> {
    let key = account_key(agent, config_path);
    let mut cache = status_cache().lock().ok()?;
    let (stored, status) = cache.get(&key)?;
    if stored.elapsed() < STATUS_TTL {
        return Some(status.clone());
    }
    cache.remove(&key);
    None
}

pub(super) fn remember_status(agent: AgentKind, config_path: Option<&str>, status: AgentStatus) {
    if let Ok(mut cache) = status_cache().lock() {
        cache.insert(account_key(agent, config_path), (Instant::now(), status));
    }
}

/// Forgets one account's answer, as after it signs in or out from Sikemux.
pub(crate) fn forget_status(agent: AgentKind, config_path: Option<&str>) {
    if let Ok(mut cache) = status_cache().lock() {
        cache.remove(&account_key(agent, config_path));
    }
}

/// Forgets every answer, as when the person comes back to the app after signing in somewhere else.
fn forget_all_statuses() {
    if let Ok(mut cache) = status_cache().lock() {
        cache.clear();
    }
}

/// Forgets every status, so the next look asks each agent again. Called when the person comes back
/// to the app, since they may have signed in or out in a terminal meanwhile.
#[tauri::command]
pub fn refresh_agent_statuses() {
    forget_all_statuses();
}

/// Marks an account signed out at once, when a chat on it was refused for sign-in. Its own CLI may
/// still think it is signed in, for example when the token was revoked.
#[tauri::command]
pub fn mark_agent_signed_out(agent: AgentKind, config_path: Option<String>) {
    remember_status(agent, config_path.as_deref(), AgentStatus::SignedOut);
}

/// Provider keys OpenCode reads from the environment; any one of them is a way in.
const OPENCODE_KEY_VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_GENERATIVE_AI_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
    "DEEPSEEK_API_KEY",
];

/// `pi auth check` answers with its exit code: 0 ready, 1 not ready, 2 credentials that no longer work.
/// Anything else means this pi does not know the command.
pub(super) fn pi_status(exit_code: Option<i32>) -> AgentStatus {
    match exit_code {
        Some(0) => AgentStatus::Ready { account: None },
        Some(1 | 2) => AgentStatus::SignedOut,
        _ => AgentStatus::Unknown,
    }
}

fn without_escapes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        plain.push(c);
    }
    plain
}

/// `opencode auth list` ends with a count such as "1 credentials". A provider key in the
/// environment counts as well, since OpenCode reads those without a stored credential.
pub(super) fn opencode_status<'a>(
    listing: &str,
    environment: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> AgentStatus {
    let plain = without_escapes(listing);
    let stored = plain.lines().rev().find_map(|line| {
        let words: Vec<&str> = line
            .trim_matches(|c: char| !c.is_alphanumeric())
            .split_whitespace()
            .collect();
        match words.as_slice() {
            [count, noun, ..] if noun.starts_with("credential") => count.parse::<u32>().ok(),
            _ => None,
        }
    });
    if stored.is_some_and(|count| count > 0) {
        return AgentStatus::Ready { account: None };
    }
    let has_key = environment
        .into_iter()
        .any(|(name, value)| OPENCODE_KEY_VARIABLES.contains(&name) && !value.trim().is_empty());
    if has_key {
        return AgentStatus::Ready {
            account: Some("apiKey".into()),
        };
    }
    match stored {
        Some(_) => AgentStatus::SignedOut,
        None => AgentStatus::Unknown,
    }
}

/// The status of an agent whose binary was found: from the last minute's answer, or asked now.
pub(super) async fn signed_in_status(
    agent: AgentKind,
    program: &Path,
    config_path: Option<&str>,
) -> AgentStatus {
    if let Some(status) = cached_status(agent, config_path) {
        return status;
    }
    let status = check_sign_in(agent, program, config_path).await;
    remember_status(agent, config_path, status.clone());
    status
}

/// Long enough for a CLI that starts slowly; a check that takes longer answers `Unknown`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(12);

async fn exit_code_and_output(
    agent: AgentKind,
    program: &Path,
    config_path: Option<&str>,
    args: &[&str],
) -> Option<(Option<i32>, String)> {
    let mut command = agent_command(agent, program, config_path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = tokio::time::timeout(CHECK_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    Some((
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

/// Asks the agent's own CLI whether it is signed in, with the profile's environment so it answers
/// for the right account. A check that fails or times out says `Unknown` rather than signed out.
pub(super) async fn check_sign_in(
    agent: AgentKind,
    program: &Path,
    config_path: Option<&str>,
) -> AgentStatus {
    match agent {
        AgentKind::Claude | AgentKind::Codex => {
            let account = agent_account_status(
                agent,
                Some(program.to_string_lossy().into_owned()),
                config_path.map(str::to_string),
            )
            .await;
            match account {
                Ok(account) if account.is_signed_in() => AgentStatus::Ready {
                    account: account.method().map(str::to_string),
                },
                Ok(_) => AgentStatus::SignedOut,
                Err(_) => AgentStatus::Unknown,
            }
        }
        AgentKind::Pi => {
            match exit_code_and_output(agent, program, config_path, &["auth", "check", "--json"])
                .await
            {
                Some((code, _)) => pi_status(code),
                None => AgentStatus::Unknown,
            }
        }
        AgentKind::Opencode => {
            let Some((_, listing)) =
                exit_code_and_output(agent, program, config_path, &["auth", "list"]).await
            else {
                return AgentStatus::Unknown;
            };
            let providers = crate::model_providers::environment(agent.as_str()).await;
            let shell = sikemux_pty::user_shell::login_shell_environment();
            let environment = shell
                .iter()
                .chain(providers.iter())
                .map(|(name, value)| (name.as_str(), value.as_str()));
            opencode_status(&listing, environment)
        }
        // Hermes answers in free text that changes with the provider, and Grok and OMP have no
        // check; a chat that is refused for sign-in marks them signed out instead.
        AgentKind::Hermes | AgentKind::Grok | AgentKind::Omp => AgentStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_read_as_a_state_and_its_details() {
        let ready = serde_json::to_value(AgentStatus::Ready {
            account: Some("subscription".into()),
        })
        .unwrap();
        assert_eq!(
            ready,
            serde_json::json!({ "state": "ready", "account": "subscription" })
        );
        assert_eq!(
            serde_json::to_value(AgentStatus::SignedOut).unwrap(),
            serde_json::json!({ "state": "signedOut" })
        );
        assert_eq!(
            serde_json::to_value(AgentStatus::Broken {
                reason: "exit 127".into()
            })
            .unwrap(),
            serde_json::json!({ "state": "broken", "reason": "exit 127" })
        );
    }

    #[cfg(unix)]
    fn fake_cli(dir: &Path, script: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("agent");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pi_is_asked_with_auth_check() {
        let dir = tempfile::tempdir().unwrap();
        let ready = fake_cli(
            dir.path(),
            r#"[ "$1 $2" = "auth check" ] && exit 0; exit 64"#,
        );
        assert_eq!(
            check_sign_in(AgentKind::Pi, &ready, None).await,
            AgentStatus::Ready { account: None }
        );
        let signed_out = fake_cli(dir.path(), "exit 1");
        assert_eq!(
            check_sign_in(AgentKind::Pi, &signed_out, None).await,
            AgentStatus::SignedOut
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn opencode_is_asked_for_its_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let listed = fake_cli(
            dir.path(),
            r#"[ "$1 $2" = "auth list" ] && printf '┌  Credentials\n│\n└  2 credentials\n'"#,
        );
        assert_eq!(
            check_sign_in(AgentKind::Opencode, &listed, None).await,
            AgentStatus::Ready { account: None }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_answers_through_its_account_check() {
        let dir = tempfile::tempdir().unwrap();
        let signed_in = fake_cli(
            dir.path(),
            r#"[ "$1" = "--version" ] && echo "2.1.0 (Claude Code)" && exit 0
[ "$1 $2" = "auth status" ] && echo '{"loggedIn":true,"authMethod":"claude.ai","email":"a@b.c"}' && exit 0
exit 64"#,
        );
        assert_eq!(
            check_sign_in(AgentKind::Claude, &signed_in, None).await,
            AgentStatus::Ready {
                account: Some("subscription".into())
            }
        );
        let other = tempfile::tempdir().unwrap();
        let signed_out = fake_cli(
            other.path(),
            r#"[ "$1" = "--version" ] && echo "2.1.0 (Claude Code)" && exit 0
echo '{"loggedIn":false,"authMethod":"none"}'; exit 1"#,
        );
        assert_eq!(
            check_sign_in(AgentKind::Claude, &signed_out, None).await,
            AgentStatus::SignedOut
        );
    }

    #[test]
    fn pi_answers_with_its_exit_code() {
        assert_eq!(pi_status(Some(0)), AgentStatus::Ready { account: None });
        assert_eq!(pi_status(Some(1)), AgentStatus::SignedOut);
        assert_eq!(pi_status(Some(2)), AgentStatus::SignedOut);
        assert_eq!(pi_status(Some(64)), AgentStatus::Unknown);
        assert_eq!(pi_status(None), AgentStatus::Unknown);
    }

    #[test]
    fn opencode_counts_its_credentials_and_provider_keys() {
        let one = "\u{1b}[0m\n┌  Credentials \u{1b}[90m~/.local/share/opencode/auth.json\n│\n●  Anthropic \u{1b}[90mapi\n│\n└  1 credentials\n";
        assert_eq!(
            opencode_status(one, []),
            AgentStatus::Ready { account: None }
        );
        let none = "┌  Credentials ~/.local/share/opencode/auth.json\n│\n└  0 credentials\n";
        assert_eq!(opencode_status(none, []), AgentStatus::SignedOut);
        assert_eq!(
            opencode_status(none, [("OPENAI_API_KEY", "sk-test")]),
            AgentStatus::Ready {
                account: Some("apiKey".into())
            }
        );
        assert_eq!(
            opencode_status(none, [("OPENAI_API_KEY", " "), ("PATH", "/bin")]),
            AgentStatus::SignedOut
        );
        assert_eq!(opencode_status("something else", []), AgentStatus::Unknown);
    }
}
