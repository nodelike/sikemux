//! Claude's limit resets. Its CLI offers them only on its own terminal screen, so the core asks
//! Claude's service itself, as that screen does, with the account's sign-in.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use reqwest::{Client, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;
use tokio::process::Command;

use super::folders;
use super::usage::{ResetKind, ResetOutcome, Resets, Usage, UsageWindow};

const API: &str = "https://api.anthropic.com";
const PROGRAM: &str = "cedar_ember";
const SPEND_TIMEOUT: Duration = Duration::from_secs(25);

/// The resets the account holds, or none when Claude offers it none or could not be asked.
pub(crate) async fn read(
    program: &Path,
    config_path: Option<&str>,
    windows: &[UsageWindow],
) -> Option<Resets> {
    let grants = grants(program, config_path).await.ok()??;
    Some(grants.resets(windows))
}

/// Spends the grant `grant` names, else the one Claude would use next.
pub(crate) async fn spend(
    program: &Path,
    config_path: Option<&str>,
    grant: Option<&str>,
) -> Result<ResetOutcome, String> {
    let Some(grants) = grants(program, config_path).await? else {
        return Ok(ResetOutcome::NoReset);
    };
    let grant = match grant {
        Some(id) => (grants.live.iter())
            .find(|live| live.id == id)
            .map(|live| live.id.clone()),
        None => grants.next.clone(),
    };
    let Some(grant) = grant else {
        return Ok(ResetOutcome::NoReset);
    };
    let organization = organization(config_path).ok_or(SIGNED_OUT)?;
    let attempt = format!("{}/{grant}", config_path.unwrap_or_default());
    let request_id = attempts()
        .locked()
        .entry(attempt.clone())
        .or_insert_with(|| uuid::Uuid::new_v4().simple().to_string())
        .clone();
    let body = serde_json::json!({
        "program": PROGRAM,
        "grant_id": grant,
        "request_id": request_id,
    });
    let path = format!("/api/organizations/{organization}/reset_rate_limits");
    let version = version(program).await;
    let answer = ask(program, config_path, |client, token| {
        signed(client.post(format!("{API}{path}")), token, &version)
            .json(&body)
            .timeout(SPEND_TIMEOUT)
    })
    .await?;
    attempts().locked().remove(&attempt);
    match answer.get("result").and_then(Value::as_str) {
        Some("reset" | "already_used") => Ok(ResetOutcome::Reset),
        Some("not_limited") => Ok(ResetOutcome::NothingToReset),
        Some("ineligible") => Ok(ResetOutcome::NoReset),
        Some("cooldown") => Err("Claude resets are cooling down. Try again later".to_owned()),
        _ => Err(
            "Claude could not confirm the reset. If you're still limited in a moment, try again"
                .to_owned(),
        ),
    }
}

const SIGNED_OUT: &str = "Sign in to Claude again to use resets";

/// The request id of a spend that got no answer, by account and grant, sent again with a retry
/// so Claude takes the two as one and never spends a second reset.
fn attempts() -> &'static Mutex<HashMap<String, String>> {
    static ATTEMPTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ATTEMPTS.get_or_init(Default::default)
}

#[derive(Deserialize)]
struct Grant {
    id: String,
    #[serde(default)]
    label: String,
    resets_left: u32,
    ends_at: Option<String>,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    usable_now: bool,
    #[serde(default)]
    clears: Vec<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// A grant Claude would spend now.
struct Live {
    id: String,
    label: String,
    left: u32,
    ends: Option<u64>,
    clears: Vec<String>,
    kind: Option<String>,
}

/// The account's usable grants, in Claude's order, and the one Claude spends next.
struct Grants {
    live: Vec<Live>,
    next: Option<String>,
}

impl Grants {
    fn from(block: &Value, now_ms: u64) -> Option<Self> {
        if block.get("eligible").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let next_id = block.get("next_grant_id").and_then(Value::as_str);
        let listed = block
            .get("grants")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        let mut live = Vec::new();
        for grant in listed.filter_map(|grant| Grant::deserialize(grant).ok()) {
            let ends = grant
                .ends_at
                .as_deref()
                .map(crate::accounts::live::parse_rfc3339);
            let expired = ends.is_some_and(|ends| ends.is_none_or(|ends| ends <= now_ms));
            if grant.paused || !grant.usable_now || expired || grant.resets_left == 0 {
                continue;
            }
            live.push(Live {
                label: match grant.label.trim() {
                    "" => "Reset".to_owned(),
                    label => label.to_owned(),
                },
                id: grant.id,
                left: grant.resets_left,
                ends: ends.flatten(),
                clears: grant.clears,
                kind: grant.kind,
            });
        }
        let next = next_id
            .filter(|id| live.iter().any(|grant| grant.id == *id))
            .map(str::to_owned);
        Some(Self { live, next })
    }

    /// The grants added up by kind, each matched to the window it refills.
    fn resets(&self, windows: &[UsageWindow]) -> Resets {
        let mut kinds: Vec<ResetKind> = Vec::new();
        for grant in &self.live {
            let window = refills(grant, windows);
            match kinds.iter_mut().find(|kind| kind.window == window) {
                Some(kind) => {
                    kind.left += grant.left;
                    if grant.ends.unwrap_or(u64::MAX) < kind.use_by.unwrap_or(u64::MAX) {
                        (kind.id, kind.use_by) = (Some(grant.id.clone()), grant.ends);
                    }
                }
                None => kinds.push(ResetKind {
                    id: Some(grant.id.clone()),
                    label: kind_name(window.as_deref(), windows),
                    left: grant.left,
                    use_by: grant.ends,
                    window,
                }),
            }
        }
        Resets {
            left: kinds.iter().map(|kind| kind.left).sum(),
            kinds,
        }
    }
}

/// What Claude's own screen calls resets refilling `window`: weekly, a model's, or full.
fn kind_name(window: Option<&str>, windows: &[UsageWindow]) -> String {
    let refilled = window.and_then(|label| windows.iter().find(|window| window.label == label));
    match refilled {
        Some(UsageWindow {
            scope: Some(scope), ..
        }) => format!("{scope} reset"),
        Some(UsageWindow {
            window_minutes: Some(300),
            ..
        }) => "5-hour reset".to_owned(),
        Some(_) => "Weekly reset".to_owned(),
        None => "Full reset".to_owned(),
    }
}

/// The window a grant refills: one whose model it names, such as Fable, else the session or the
/// week its type says. None for a grant that starts every used-up window over.
fn refills(grant: &Live, windows: &[UsageWindow]) -> Option<String> {
    let named = grant.label.to_lowercase();
    if let Some(window) = windows.iter().find(|window| {
        (window.scope.as_deref()).is_some_and(|scope| {
            named
                .split_whitespace()
                .any(|word| word == scope.to_lowercase())
        })
    }) {
        return Some(window.label.clone());
    }
    let minutes = match (grant.kind.as_deref(), grant.clears.as_slice()) {
        (Some("session"), _) => 300,
        (_, clears) if clears.iter().any(|clear| clear == "five_hour") && clears.len() == 1 => 300,
        (Some("full"), _) => return None,
        _ => 10_080,
    };
    (windows.iter())
        .find(|window| window.scope.is_none() && window.window_minutes == Some(minutes))
        .map(|window| window.label.clone())
}

/// The account's limits and resets in one request to Claude's usage endpoint, as claude.ai and
/// Claude Code's /usage read them. None when Claude could not be asked, or answered in a shape
/// with no limits in it, so the caller asks the CLI instead; an error when Claude refused, so
/// nothing asks again for a while.
pub(crate) async fn usage(
    program: &Path,
    config_path: Option<&str>,
) -> Result<Option<Usage>, String> {
    let answer = match usage_answer(program, config_path).await {
        Ok(answer) => answer,
        Err(error) if super::usage::refused(&error) => return Err(error),
        Err(_) => return Ok(None),
    };
    let windows = super::usage::claude_windows(&answer);
    if windows.is_empty() {
        return Ok(None);
    }
    let now = sikemux_process::clock::epoch_ms();
    let resets = (answer.get("cedar_ember"))
        .and_then(|block| Grants::from(block, now))
        .map(|grants| grants.resets(&windows));
    Ok(Some(Usage {
        provider: "claude".to_owned(),
        plan: None,
        tier: None,
        windows,
        unavailable_reason: None,
        resets,
    }))
}

async fn usage_answer(program: &Path, config_path: Option<&str>) -> Result<Value, String> {
    let version = version(program).await;
    ask(program, config_path, |client, token| {
        signed(
            client.get(format!("{API}/api/oauth/usage")),
            token,
            &version,
        )
        .query(&[("cedar_ember", "1"), ("skip_spend", "1")])
    })
    .await
}

async fn grants(program: &Path, config_path: Option<&str>) -> Result<Option<Grants>, String> {
    let answer = usage_answer(program, config_path).await?;
    let now = sikemux_process::clock::epoch_ms();
    Ok(answer
        .get("cedar_ember")
        .and_then(|block| Grants::from(block, now)))
}

/// Sends what `request` builds with the account's token. A token Claude's service no longer
/// takes is renewed by running the account's CLI once, then the request is sent again.
async fn ask(
    program: &Path,
    config_path: Option<&str>,
    request: impl Fn(&Client, &str) -> RequestBuilder,
) -> Result<Value, String> {
    let client = client()?;
    for renewed in [false, true] {
        let token = token(config_path).await.ok_or(SIGNED_OUT)?;
        let response = request(client, &token)
            .send()
            .await
            .map_err(|_| "Could not reach Claude".to_owned())?;
        match response.status() {
            StatusCode::UNAUTHORIZED if !renewed => {
                super::usage::renew_claude_sign_in(program, config_path).await;
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(SIGNED_OUT.to_owned()),
            StatusCode::TOO_MANY_REQUESTS => {
                return Err("Claude is rate limiting these requests. Try again soon".to_owned())
            }
            status if !status.is_success() => return Err(format!("Claude answered {status}")),
            _ => {
                return response
                    .json()
                    .await
                    .map_err(|_| "Claude sent an unreadable answer".to_owned())
            }
        }
    }
    Err(SIGNED_OUT.to_owned())
}

fn signed(request: RequestBuilder, token: &str, version: &str) -> RequestBuilder {
    request
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(
            "user-agent",
            format!("claude-cli/{version} (external, cli)"),
        )
}

fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    crate::http::shared(&CLIENT, || {
        crate::http::builder().timeout(Duration::from_secs(10))
    })
    .map_err(|_| "Could not reach Claude".to_owned())
}

/// The version the CLI at `program` reports, asked once per program.
async fn version(program: &Path) -> String {
    static VERSIONS: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let versions = VERSIONS.get_or_init(Default::default);
    if let Some(version) = versions.locked().get(program) {
        return version.clone();
    }
    let version = Command::from(sikemux_process::user_environment::command(program))
        .arg("--version")
        .output()
        .await
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "0".to_owned());
    versions
        .locked()
        .insert(program.to_owned(), version.clone());
    version
}

/// The folder Claude was pointed at with `CLAUDE_CONFIG_DIR`, or none for its default one.
fn named_folder(config_path: Option<&str>) -> Option<PathBuf> {
    let file = folders::claude_config_file(config_path, &BTreeMap::new());
    let folder = file.parent()?.to_owned();
    (file != folders::home().join(".claude.json")).then_some(folder)
}

fn organization(config_path: Option<&str>) -> Option<String> {
    let file = folders::claude_config_file(config_path, &BTreeMap::new());
    let config: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    let organization = config
        .pointer("/oauthAccount/organizationUuid")?
        .as_str()?
        .trim();
    (!organization.is_empty()
        && organization
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-'))
    .then(|| organization.to_owned())
}

/// The account's sign-in token, read where its CLI keeps it each time and never kept here.
async fn token(config_path: Option<&str>) -> Option<String> {
    let folder = named_folder(config_path);
    let stored = if cfg!(target_os = "macos") {
        let output = Command::from(sikemux_process::user_environment::command(
            "/usr/bin/security",
        ))
        .args([
            "find-generic-password",
            "-w",
            "-s",
            &keychain_item(folder.as_deref()),
        ])
        .output()
        .await
        .ok()?;
        output.status.success().then_some(output.stdout)?
    } else {
        let folder = folder.unwrap_or_else(|| folders::home().join(".claude"));
        tokio::fs::read(folder.join(".credentials.json"))
            .await
            .ok()?
    };
    let stored: Value = serde_json::from_slice(&stored).ok()?;
    let token = stored
        .pointer("/claudeAiOauth/accessToken")?
        .as_str()?
        .trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// The Keychain item Claude keeps an account's sign-in in: one more for each `CLAUDE_CONFIG_DIR`,
/// named by its path's hash.
fn keychain_item(folder: Option<&Path>) -> String {
    match folder {
        None => "Claude Code-credentials".to_owned(),
        Some(folder) => {
            let hash = hex::encode(Sha256::digest(folder.to_string_lossy().as_bytes()));
            format!("Claude Code-credentials-{}", &hash[..8])
        }
    }
}
