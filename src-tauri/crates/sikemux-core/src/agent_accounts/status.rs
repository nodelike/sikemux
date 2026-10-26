//! Whether each agent can be used right now: found, working, and signed in,
//! and who an account is signed in as. Every agent answers sign-in its own
//! way through its own CLI, and some cannot answer at all, so `Unknown` is an
//! honest answer rather than a failure. Sikemux never reads a credential.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;

use super::folders::{link_shared, root, sessions_root};
use super::usage::codex_app_server;
use super::{cli, key};
use crate::agent_catalog::{AgentKind, AGENTS};
use crate::launch::{discovery, AgentSpec};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
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

impl AgentStatus {
    /// What a device is told: `ready`, `signedOut` and so on.
    pub fn state(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Broken { .. } => "broken",
            Self::SignedOut => "signedOut",
            Self::Ready { .. } => "ready",
            Self::Unknown => "unknown",
        }
    }

    /// Why an agent labelled `label` cannot be started now, if it cannot.
    pub fn refusal(&self, label: &str) -> Option<String> {
        match self {
            Self::Missing => Some(format!("{label} is not installed on this host")),
            Self::SignedOut => Some(format!(
                "{label} is signed out on this host; ask the person to sign in"
            )),
            Self::Broken { .. } => Some(format!("{label} does not start on this host")),
            Self::Ready { .. } | Self::Unknown => None,
        }
    }
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    signed_in: bool,
    /// The person's own name on the account, where the agent keeps one.
    name: Option<String>,
    email: Option<String>,
    plan: Option<String>,
    organization: Option<String>,
    /// `subscription`, `apiKey`, or what else the CLI says it signs in with.
    method: Option<String>,
    sessions: Option<String>,
}

/// Long enough for a CLI that starts slowly; a check that takes longer answers `Unknown`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_KEPT: usize = 64;

/// What one look at an account found: whether it is signed in and, for
/// Claude and Codex, who as.
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    pub status: AgentStatus,
    pub account: Option<Result<AccountStatus, String>>,
}

/// An account's last answer, and whether it still holds or is only shown
/// while the account is asked again.
struct Kept {
    checked: Checked,
    holds: bool,
}

fn kept() -> &'static Mutex<HashMap<String, Kept>> {
    static KEPT: OnceLock<Mutex<HashMap<String, Kept>>> = OnceLock::new();
    KEPT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Moves on each time answers are forgotten, so an answer asked for before
/// is not kept after.
static FORGOTTEN: AtomicU64 = AtomicU64::new(0);

pub(crate) fn generation() -> u64 {
    FORGOTTEN.load(Ordering::SeqCst)
}

/// The last answer for one account, and whether it still holds.
pub(crate) fn known(provider: &str, config_path: Option<&str>) -> Option<(Checked, bool)> {
    let kept = kept().locked();
    let kept = kept.get(&key(provider, config_path))?;
    Some((kept.checked.clone(), kept.holds))
}

/// The last sign-in heard for one account.
pub(crate) fn cached(provider: &str, config_path: Option<&str>) -> Option<AgentStatus> {
    known(provider, config_path).map(|(checked, _)| checked.status)
}

/// Keeps what an account answered unless its answers were forgotten since
/// `generation`, and says whether it differs from the last answer.
pub(crate) fn keep(
    provider: &str,
    config_path: Option<&str>,
    checked: &Checked,
    generation: u64,
) -> bool {
    let mut kept = kept().locked();
    if self::generation() != generation {
        return false;
    }
    if kept.len() >= MAX_KEPT {
        kept.clear();
    }
    let previous = kept.insert(
        key(provider, config_path),
        Kept {
            checked: checked.clone(),
            holds: true,
        },
    );
    previous.is_none_or(|previous| previous.checked != *checked)
}

/// Holds one account at `status`, whatever its CLI last said.
pub(crate) fn remember(provider: &str, config_path: Option<&str>, status: AgentStatus) {
    let mut kept = kept().locked();
    let kept = kept
        .entry(key(provider, config_path))
        .or_insert_with(|| Kept {
            checked: Checked {
                status: status.clone(),
                account: None,
            },
            holds: true,
        });
    kept.checked.status = status;
    kept.holds = true;
}

/// Forgets one account's answer, so the next look waits for a new one. With
/// no provider, every answer is still shown but only until it is asked again.
pub(crate) fn forget(provider: Option<&str>, config_path: Option<&str>) {
    let mut kept = kept().locked();
    FORGOTTEN.fetch_add(1, Ordering::SeqCst);
    match provider {
        Some(provider) => {
            kept.remove(&key(provider, config_path));
        }
        None => kept.values_mut().for_each(|kept| kept.holds = false),
    }
}

/// Asks the account's own CLI whether it is signed in, and for Claude and
/// Codex who as, waiting for a turn among the other programs being asked.
pub async fn ask(
    provider: &str,
    program: &Path,
    config_path: Option<&str>,
    providers: BTreeMap<String, String>,
) -> Checked {
    let _turn = discovery::probe_turn().await;
    if matches!(provider, "claude" | "codex") {
        let account = account(provider, program, config_path).await;
        return Checked {
            status: sign_in_of(&account),
            account: Some(account),
        };
    }
    Checked {
        status: check_sign_in(provider, program, config_path, providers).await,
        account: None,
    }
}

fn sign_in_of(account: &Result<AccountStatus, String>) -> AgentStatus {
    match account {
        Ok(account) if account.signed_in => AgentStatus::Ready {
            account: account.method.clone(),
        },
        Ok(_) => AgentStatus::SignedOut,
        Err(_) => AgentStatus::Unknown,
    }
}

/// Where `spec`'s program is, and why each program tried before it failed.
pub async fn locate(spec: &AgentSpec) -> (Option<PathBuf>, Vec<String>) {
    let Some(def) = AGENTS.iter().find(|def| def.kind == spec.provider) else {
        return (None, Vec::new());
    };
    let candidates = discovery::candidates(def, spec.executable_path.as_deref());
    discovery::first_healthy(def, candidates).await
}

/// An agent with no working program: missing, or broken by what `failures` say.
pub fn unfound(failures: &[String]) -> AgentStatus {
    if failures.is_empty() {
        AgentStatus::Missing
    } else {
        AgentStatus::Broken {
            reason: failures.join("; "),
        }
    }
}

/// Where `spec`'s program is, or why there is none: it is missing or broken.
pub async fn find(spec: &AgentSpec) -> Result<PathBuf, AgentStatus> {
    match locate(spec).await {
        (Some(program), _) => Ok(program),
        (None, failures) => Err(unfound(&failures)),
    }
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
fn pi_status(exit_code: Option<i32>) -> AgentStatus {
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
fn opencode_status<'a>(
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

async fn exit_code_and_output(
    provider: &str,
    program: &Path,
    config_path: Option<&str>,
    args: &[&str],
) -> Option<(Option<i32>, String)> {
    let mut command = cli(provider, program, config_path, BTreeMap::new());
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
async fn check_sign_in(
    provider: &str,
    program: &Path,
    config_path: Option<&str>,
    providers: BTreeMap<String, String>,
) -> AgentStatus {
    let Some(kind) = AgentKind::from_name(provider) else {
        return AgentStatus::Unknown;
    };
    match kind {
        AgentKind::Claude | AgentKind::Codex => {
            sign_in_of(&account(provider, program, config_path).await)
        }
        AgentKind::Pi => {
            match exit_code_and_output(provider, program, config_path, &["auth", "check", "--json"])
                .await
            {
                Some((code, _)) => pi_status(code),
                None => AgentStatus::Unknown,
            }
        }
        AgentKind::Opencode => {
            let Some((_, listing)) =
                exit_code_and_output(provider, program, config_path, &["auth", "list"]).await
            else {
                return AgentStatus::Unknown;
            };
            let shell = sikemux_pty::user_shell::login_shell_environment();
            let environment = shell
                .iter()
                .chain(providers.iter())
                .map(|(name, value)| (name.as_str(), value.as_str()));
            opencode_status(&listing, environment)
        }
        // Hermes answers in free text that changes with the provider, and Grok, OMP and Muse
        // have no check; a chat that is refused for sign-in marks them signed out instead.
        AgentKind::Hermes | AgentKind::Grok | AgentKind::Omp | AgentKind::Muse => {
            AgentStatus::Unknown
        }
    }
}

fn parse_claude_status(text: &str) -> Option<AccountStatus> {
    let status: Value = serde_json::from_str(text.trim()).ok()?;
    let text_of = |key: &str| {
        status
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let method = text_of("authMethod").filter(|method| method != "none");
    Some(AccountStatus {
        signed_in: status
            .get("loggedIn")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        name: None,
        email: text_of("email"),
        plan: text_of("subscriptionType"),
        organization: text_of("orgName"),
        method: method.map(|method| {
            if method == "claude.ai" {
                "subscription".into()
            } else {
                method
            }
        }),
        sessions: None,
    })
}

/// `auth status` leaves the name out; Claude keeps it in the account's
/// `.claude.json`, which holds no credentials.
fn claude_account_name(config: &Value) -> Option<String> {
    let account = config.get("oauthAccount")?;
    ["displayName", "fullName"]
        .into_iter()
        .filter_map(|key| account.get(key).and_then(Value::as_str))
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Claude puts `.claude.json` inside a chosen config folder, but beside the
/// default `~/.claude` folder rather than in it.
fn claude_config_file(config_path: Option<&str>) -> Option<PathBuf> {
    let root = root("claude", config_path)?;
    let inside = root.join(".claude.json");
    if inside.exists() {
        return Some(inside);
    }
    let home = sikemux_process::paths::home()?;
    (root == home.join(".claude")).then(|| home.join(".claude.json"))
}

fn parse_codex_status(result: &Value) -> AccountStatus {
    let account = result.get("account").filter(|account| !account.is_null());
    let text_of = |key: &str| {
        account
            .and_then(|account| account.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let method = text_of("type").map(|kind| match kind.as_str() {
        "chatgpt" => "subscription".to_owned(),
        _ => kind,
    });
    AccountStatus {
        signed_in: account.is_some(),
        name: None,
        email: text_of("email"),
        plan: text_of("planType"),
        organization: None,
        method,
        sessions: None,
    }
}

/// Who an account is signed in as, asked of the agent's own CLI.
async fn account(
    provider: &str,
    program: &Path,
    config_path: Option<&str>,
) -> Result<AccountStatus, String> {
    link_shared(provider, config_path);
    let mut status = match provider {
        "claude" => {
            let mut command = cli(provider, program, config_path, BTreeMap::new());
            command
                .args(["auth", "status", "--json"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let output = tokio::time::timeout(CHECK_TIMEOUT, command.output())
                .await
                .map_err(|_| "Claude took too long to say who is signed in".to_string())?
                .map_err(|_| "Could not ask Claude who is signed in".to_string())?;
            // A signed-out account still answers, with a failing exit code.
            parse_claude_status(&String::from_utf8_lossy(&output.stdout))
                .ok_or_else(|| "Claude did not say who is signed in".to_string())?
        }
        "codex" => parse_codex_status(
            &codex_app_server(program, config_path, "account/read", "account").await?,
        ),
        _ => return Err("Only Claude and Codex keep separate accounts".into()),
    };
    if provider == "claude" && status.signed_in {
        status.name = claude_config_file(config_path)
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|config| claude_account_name(&config));
    }
    status.sessions =
        sessions_root(provider, config_path).map(|path| path.to_string_lossy().into_owned());
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    #[tokio::test]
    async fn an_agent_is_found_working_broken_or_missing() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let pi = script(
            "pi",
            "#!/bin/sh\n[ \"$1\" = --version ] && echo 'pi 1.0' && exit 0\nexit 64\n",
        );
        let broken = script(
            "omp",
            "#!/bin/sh\nprintf 'launcher missing\\n' >&2\nexit 127\n",
        );
        let spec = |provider: &str, path: &Path| AgentSpec {
            provider: provider.into(),
            executable_path: Some(path.to_string_lossy().into_owned()),
            config_path: None,
            environment_keys: Vec::new(),
        };

        assert_eq!(locate(&spec("pi", &pi)).await.0, Some(pi.clone()));
        let (program, failures) = locate(&spec("omp", &broken)).await;
        assert!(program.is_none());
        assert!(
            matches!(unfound(&failures), AgentStatus::Broken { reason } if reason.contains("launcher missing"))
        );
        let (program, failures) = locate(&spec("grok", &dir.path().join("nowhere/grok"))).await;
        assert!(program.is_none());
        assert!(
            matches!(
                unfound(&failures),
                AgentStatus::Missing | AgentStatus::Broken { .. }
            ),
            "a profile whose binary is gone is not ready"
        );
    }

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
    fn fake_cli(dir: &Path, script: &str) -> PathBuf {
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
            check_sign_in("pi", &ready, None, BTreeMap::new()).await,
            AgentStatus::Ready { account: None }
        );
        let signed_out = fake_cli(dir.path(), "exit 1");
        assert_eq!(
            check_sign_in("pi", &signed_out, None, BTreeMap::new()).await,
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
            check_sign_in("opencode", &listed, None, BTreeMap::new()).await,
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
            check_sign_in("claude", &signed_in, None, BTreeMap::new()).await,
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
            check_sign_in("claude", &signed_out, None, BTreeMap::new()).await,
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

    #[test]
    fn claude_says_who_is_signed_in() {
        let status = parse_claude_status(
            r#"{"loggedIn":true,"authMethod":"claude.ai","email":"me@x.com","orgName":"Me","subscriptionType":"max"}"#,
        )
        .unwrap();
        assert!(status.signed_in);
        assert_eq!(status.email.as_deref(), Some("me@x.com"));
        assert_eq!(status.plan.as_deref(), Some("max"));
        assert_eq!(status.method.as_deref(), Some("subscription"));
        let out = parse_claude_status(r#"{"loggedIn":false,"authMethod":"none"}"#).unwrap();
        assert!(!out.signed_in);
        assert_eq!(out.method, None);
        assert_eq!(parse_claude_status("not json"), None);
    }

    #[test]
    fn claude_keeps_the_account_name_beside_the_sign_in() {
        let config =
            json!({ "oauthAccount": { "displayName": "Kishore", "fullName": "Kishore G" } });
        assert_eq!(claude_account_name(&config).as_deref(), Some("Kishore"));
        let full = json!({ "oauthAccount": { "displayName": " ", "fullName": "Kishore G" } });
        assert_eq!(claude_account_name(&full).as_deref(), Some("Kishore G"));
        assert_eq!(claude_account_name(&json!({ "projects": {} })), None);
    }

    #[test]
    fn codex_says_who_is_signed_in() {
        let status = parse_codex_status(&json!({
            "account": { "type": "chatgpt", "email": "me@x.com", "planType": "pro" },
            "requiresOpenaiAuth": true,
        }));
        assert!(status.signed_in);
        assert_eq!(status.plan.as_deref(), Some("pro"));
        assert_eq!(status.method.as_deref(), Some("subscription"));
        let key = parse_codex_status(&json!({ "account": { "type": "apiKey" } }));
        assert_eq!(key.method.as_deref(), Some("apiKey"));
        assert!(!parse_codex_status(&json!({ "account": null })).signed_in);
    }
}
