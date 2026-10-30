// Profile discovery and identity/SSO-login surface.
//
//   profiles     — parse ~/.aws/{config,credentials}, classify
//   resolve      — which profile and region an agent's call runs as
//   identity     — `sts get-caller-identity`, 60s per-process cache
//   status       — whether the profile agents start on can sign in
//   sso_login    — `aws sso login` (spawn-and-wait)

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use sikemux_plugin_api::process;
use sikemux_plugin_api::process::sync::Locked;
use sikemux_plugin_api::{PluginContext, SignInStatus};

use crate::common::{classify_cli_err, run_aws_cli_async, Target};
use crate::error::{AwsError, AwsResult};

// ============================================================
// profile discovery
// ============================================================

#[derive(Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AwsKind {
    Sso,
    Role,
    CredentialProcess,
    Credentials,
}

#[derive(Serialize, Clone)]
pub struct AwsProfile {
    pub(crate) name: String,
    pub(crate) region: Option<String>,
    pub(crate) sso_start_url: Option<String>,
    pub(crate) sso_region: Option<String>,
    pub(crate) sso_account_id: Option<String>,
    pub(crate) sso_role_name: Option<String>,
    pub(crate) kind: AwsKind,
}

/// Where the AWS CLI looks, so the profiles listed are the ones it can use.
fn aws_file(variable: &str, default: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();
    match process::user_environment::var(variable).filter(|path| !path.is_empty()) {
        Some(path) => Some(expand_home(&path, home.as_deref())),
        None => Some(PathBuf::from(home?).join(".aws").join(default)),
    }
}

fn expand_home(path: &str, home: Option<&str>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

fn config_path() -> Option<PathBuf> {
    aws_file("AWS_CONFIG_FILE", "config")
}

fn credentials_path() -> Option<PathBuf> {
    aws_file("AWS_SHARED_CREDENTIALS_FILE", "credentials")
}

/// Parse an INI-style AWS config. Supports `[profile X]`, `[default]`, and
/// `[sso-session Y]`. Returns `(profile_blocks, sso_session_blocks)`.
type IniBlocks = HashMap<String, HashMap<String, String>>;

fn parse_ini(content: &str) -> (IniBlocks, IniBlocks) {
    let mut profiles: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut sessions: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current: Option<(bool, String)> = None;

    for raw in content.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let body = rest.trim();
            if let Some(name) = body.strip_prefix("profile ") {
                current = Some((false, name.trim().to_string()));
            } else if let Some(name) = body.strip_prefix("sso-session ") {
                current = Some((true, name.trim().to_string()));
            } else if body == "default" {
                current = Some((false, "default".to_string()));
            } else {
                current = None;
            }
            continue;
        }
        let Some((ref is_session, ref key)) = current else {
            continue;
        };
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim().to_string();
        let v = v.trim().to_string();
        let bucket = if *is_session {
            sessions.entry(key.clone()).or_default()
        } else {
            profiles.entry(key.clone()).or_default()
        };
        bucket.insert(k, v);
    }
    (profiles, sessions)
}

fn classify(p: &HashMap<String, String>) -> AwsKind {
    if p.contains_key("sso_session") || p.contains_key("sso_start_url") {
        AwsKind::Sso
    } else if p.contains_key("role_arn") {
        AwsKind::Role
    } else if p.contains_key("credential_process") {
        AwsKind::CredentialProcess
    } else {
        AwsKind::Credentials
    }
}

/// Whether this computer looks set up for AWS at all, so agents elsewhere are not offered the tools.
pub(crate) fn aws_in_use() -> bool {
    use sikemux_plugin_api::process::user_environment::{find_executable, var};
    [config_path(), credentials_path()]
        .into_iter()
        .flatten()
        .any(|path| path.exists())
        || var("AWS_PROFILE").is_some()
        || var("AWS_ACCESS_KEY_ID").is_some()
        || find_executable(&crate::common::aws_bin()).is_some()
}

#[derive(Serialize)]
pub struct AwsSetup {
    cli: bool,
    homebrew: bool,
}

/// What the pane needs before it can offer profiles: the CLI, and Homebrew to install it with.
pub(crate) fn setup() -> AwsSetup {
    use sikemux_plugin_api::process::user_environment::find_executable;
    let bin = crate::common::aws_bin();
    AwsSetup {
        cli: std::path::Path::new(&bin).is_file() || find_executable(&bin).is_some(),
        homebrew: find_executable("brew").is_some(),
    }
}

/// The profile and region a call runs as: the one asked for, else the one
/// agents start on, else `default`. Several others and no choice is an error
/// that lists them.
pub(crate) async fn resolve(ctx: &PluginContext, asked: Target) -> AwsResult<Target> {
    let known = profiles().await;
    let names = || known.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let env = |name| process::user_environment::var(name).filter(|v| !v.is_empty());
    let profile = match asked.profile {
        Some(profile) if known.iter().any(|p| p.name == profile) => Some(profile),
        Some(profile) => return Err(AwsError::UnknownProfile(profile, names())),
        None => match crate::agent_profile::agent_profile(ctx, None) {
            Some(picked) => picked.profile,
            None if known.iter().any(|p| p.name == "default") => Some("default".to_string()),
            None if known.is_empty() => return Err(AwsError::NoProfiles),
            None => return Err(AwsError::ChooseProfile(names())),
        },
    };
    let region = asked
        .region
        .or_else(|| {
            let name = profile.as_deref()?;
            known.iter().find(|p| p.name == name)?.region.clone()
        })
        .or_else(|| env("AWS_REGION"))
        .or_else(|| env("AWS_DEFAULT_REGION"));
    Ok(Target { profile, region })
}

pub(crate) async fn profiles() -> Vec<AwsProfile> {
    tokio::task::spawn_blocking(read_aws_profiles)
        .await
        .unwrap_or_default()
}

fn read_aws_profiles() -> Vec<AwsProfile> {
    let mut out: Vec<AwsProfile> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    if let Some(path) = config_path() {
        if let Ok(content) = fs::read_to_string(&path) {
            let (profiles, sessions) = parse_ini(&content);
            for (name, p) in profiles {
                let session = p.get("sso_session").and_then(|s| sessions.get(s)).cloned();
                let pick = |key: &str| -> Option<String> {
                    p.get(key)
                        .cloned()
                        .or_else(|| session.as_ref().and_then(|s| s.get(key).cloned()))
                };
                let entry = AwsProfile {
                    region: p.get("region").cloned(),
                    sso_start_url: pick("sso_start_url"),
                    sso_region: pick("sso_region"),
                    sso_account_id: p.get("sso_account_id").cloned(),
                    sso_role_name: p.get("sso_role_name").cloned(),
                    kind: classify(&p),
                    name: name.clone(),
                };
                seen.insert(name);
                out.push(entry);
            }
        }
    }

    if let Some(path) = credentials_path() {
        if let Ok(content) = fs::read_to_string(&path) {
            let (creds, _) = parse_ini(&content);
            for (name, _p) in creds {
                if seen.insert(name.clone()) {
                    out.push(AwsProfile {
                        name,
                        region: None,
                        sso_start_url: None,
                        sso_region: None,
                        sso_account_id: None,
                        sso_role_name: None,
                        kind: AwsKind::Credentials,
                    });
                }
            }
        }
    }

    out.sort_by_key(|profile| profile.name.to_lowercase());
    out
}

/// Every profile's name, read now rather than on the async runtime.
pub(crate) fn profile_names() -> Vec<String> {
    read_aws_profiles()
        .into_iter()
        .map(|profile| profile.name)
        .collect()
}

// ============================================================
// caller identity (cached)
// ============================================================

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AwsStatus {
    Authed,
    Expired,
    NoCredentials,
    Error,
    CliMissing,
}

#[derive(Serialize, Clone)]
pub struct AwsIdentity {
    arn: Option<String>,
    account: Option<String>,
    user_id: Option<String>,
    status: AwsStatus,
    message: Option<String>,
}

fn id_cache() -> &'static Mutex<HashMap<String, (Instant, AwsIdentity)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, AwsIdentity)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const ID_TTL: Duration = Duration::from_secs(60);
const ID_CACHE_CAPACITY: usize = 128;

pub(crate) async fn identity(target: Target, force: bool) -> AwsIdentity {
    let profile = target.label().to_string();
    if !force {
        let cached = id_cache().locked().get(&profile).cloned();
        if let Some(cached) = cached {
            if cached.0.elapsed() < ID_TTL {
                return cached.1;
            }
        }
    }

    let id = match run_aws_cli_async(&["sts", "get-caller-identity"], &target).await {
        Err(AwsError::CliMissing(msg)) => AwsIdentity {
            arn: None,
            account: None,
            user_id: None,
            status: AwsStatus::CliMissing,
            message: Some(msg),
        },
        Err(e) => AwsIdentity {
            arn: None,
            account: None,
            user_id: None,
            status: AwsStatus::Error,
            message: Some(e.to_string()),
        },
        Ok((true, stdout, _)) => {
            let mut arn = None;
            let mut account = None;
            let mut user_id = None;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
                arn = v.get("Arn").and_then(|s| s.as_str()).map(String::from);
                account = v.get("Account").and_then(|s| s.as_str()).map(String::from);
                user_id = v.get("UserId").and_then(|s| s.as_str()).map(String::from);
            }
            AwsIdentity {
                arn,
                account,
                user_id,
                status: AwsStatus::Authed,
                message: None,
            }
        }
        Ok((false, _, stderr)) => {
            let status = match classify_cli_err(&stderr, &target) {
                AwsError::TokenExpired(_) => AwsStatus::Expired,
                AwsError::NoCredentials(_) => AwsStatus::NoCredentials,
                _ => AwsStatus::Error,
            };
            AwsIdentity {
                arn: None,
                account: None,
                user_id: None,
                status,
                message: Some(stderr.trim().to_string()),
            }
        }
    };

    let mut cache = id_cache().locked();
    cache.retain(|_, (created, _)| created.elapsed() < ID_TTL);
    if cache.len() >= ID_CACHE_CAPACITY && !cache.contains_key(&profile) {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (created, _))| *created)
            .map(|(profile, _)| profile.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(profile, (Instant::now(), id.clone()));
    drop(cache);
    id
}

#[derive(Serialize)]
pub(crate) struct Signed {
    profile: Option<String>,
    account: Option<String>,
}

/// Whether AWS calls work as the profile the AWS pane shows.
pub(crate) async fn status(ctx: &PluginContext) -> SignInStatus<Signed> {
    let Some(profile) = crate::agent_profile::pane_profile(ctx) else {
        return SignInStatus {
            configured: false,
            ok: false,
            auth_failed: false,
            message: None,
            details: Signed {
                profile: None,
                account: None,
            },
        };
    };
    let id = identity(Target::named(&profile), false).await;
    let profile = Some(profile);
    SignInStatus {
        configured: true,
        ok: id.status == AwsStatus::Authed,
        auth_failed: matches!(id.status, AwsStatus::Expired | AwsStatus::NoCredentials),
        message: id.message,
        details: Signed {
            profile,
            account: id.account,
        },
    }
}

// ============================================================
// SSO login
// ============================================================

#[derive(Serialize, Clone)]
pub struct AwsLoginResult {
    success: bool,
    stdout: String,
    stderr: String,
}

pub(crate) async fn sso_login(profile: String) -> AwsLoginResult {
    let target = Target::named(&profile);
    let r = run_aws_cli_async(&["sso", "login"], &target).await;
    // Drop the cached "expired" identity so the next identity call doesn't
    // lie about state.
    id_cache().locked().remove(&profile);
    match r {
        Ok((ok, out, err)) => AwsLoginResult {
            success: ok,
            stdout: out,
            stderr: err,
        },
        Err(e) => AwsLoginResult {
            success: false,
            stdout: String::new(),
            stderr: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_from_the_shell_may_start_at_home() {
        assert_eq!(
            expand_home("~/work/aws-config", Some("/Users/me")),
            PathBuf::from("/Users/me/work/aws-config")
        );
        assert_eq!(
            expand_home("/etc/aws/config", Some("/Users/me")),
            PathBuf::from("/etc/aws/config")
        );
        assert_eq!(expand_home("~/x", None), PathBuf::from("~/x"));
    }
}
