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
use super::usage::{ResetOutcome, Resets};

const API: &str = "https://api.anthropic.com";
const PROGRAM: &str = "cedar_ember";
const SPEND_TIMEOUT: Duration = Duration::from_secs(25);

/// The resets the account holds, or none when Claude offers it none or could not be asked.
pub(crate) async fn read(program: &Path, config_path: Option<&str>) -> Option<Resets> {
    let grants = grants(program, config_path).await.ok()??;
    Some(grants.resets())
}

/// Spends the reset Claude would use next.
pub(crate) async fn spend(
    program: &Path,
    config_path: Option<&str>,
) -> Result<ResetOutcome, String> {
    let Some(grants) = grants(program, config_path).await? else {
        return Ok(ResetOutcome::NoReset);
    };
    let Some(grant) = grants.next else {
        return Ok(ResetOutcome::NoReset);
    };
    let organization = organization(config_path).ok_or(SIGNED_OUT)?;
    let body = serde_json::json!({
        "program": PROGRAM,
        "grant_id": grant,
        "request_id": uuid::Uuid::new_v4().simple().to_string(),
    });
    let path = format!("/api/organizations/{organization}/reset_rate_limits");
    let version = version(program).await;
    let answer = ask(program, config_path, |client, token| {
        signed(client.post(format!("{API}{path}")), token, &version)
            .json(&body)
            .timeout(SPEND_TIMEOUT)
    })
    .await?;
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

#[derive(Deserialize)]
struct Grant {
    id: String,
    resets_left: u32,
    ends_at: Option<String>,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    usable_now: bool,
}

/// The account's usable grants and the one Claude spends next.
struct Grants {
    live: Vec<(u32, Option<u64>)>,
    next: Option<String>,
    next_ends: Option<u64>,
}

impl Grants {
    fn from(block: &Value, now_ms: u64) -> Option<Self> {
        if block.get("eligible").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let next_id = block.get("next_grant_id").and_then(Value::as_str);
        let mut grants = Self {
            live: Vec::new(),
            next: None,
            next_ends: None,
        };
        let listed = block
            .get("grants")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for grant in listed.filter_map(|grant| Grant::deserialize(grant).ok()) {
            let ends = grant
                .ends_at
                .as_deref()
                .map(crate::accounts::live::parse_rfc3339);
            let expired = ends.is_some_and(|ends| ends.is_none_or(|ends| ends <= now_ms));
            if grant.paused || !grant.usable_now || expired {
                continue;
            }
            let ends = ends.flatten();
            if next_id == Some(grant.id.as_str()) {
                (grants.next, grants.next_ends) = (Some(grant.id), ends);
            }
            grants.live.push((grant.resets_left, ends));
        }
        Some(grants)
    }

    fn resets(&self) -> Resets {
        Resets {
            left: match self.next {
                Some(_) => self.live.iter().map(|(left, _)| left).sum(),
                None => 0,
            },
            use_by: self.next_ends,
        }
    }
}

async fn grants(program: &Path, config_path: Option<&str>) -> Result<Option<Grants>, String> {
    let version = version(program).await;
    let answer = ask(program, config_path, |client, token| {
        signed(
            client.get(format!("{API}/api/oauth/usage")),
            token,
            &version,
        )
        .query(&[("cedar_ember", "1"), ("skip_spend", "1")])
    })
    .await?;
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
                return Err("Claude is turning away reset requests. Try again soon".to_owned())
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
