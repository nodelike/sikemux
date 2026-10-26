//! The folders the person's accounts with Claude Code and Codex keep their
//! sign-ins in, named by `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
//!
//! An account Sikemux adds links its chats, settings and tools to the default
//! folder, so a chat can move between accounts and only the sign-in differs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::agent_catalog::{AgentKind, Home};
use crate::sessions::roots::account_root;

/// Marks a folder as one Sikemux made, whose shared entries it may link.
const MARKER: &str = ".sikemux-account";

/// What a Claude account shares with the default one. Anything else, such as
/// the sign-in and the CLI's own caches, stays with the account.
const CLAUDE_SHARED: &[&str] = &[
    "projects",
    "settings.json",
    "CLAUDE.md",
    "agents",
    "commands",
    "skills",
    "plugins",
    "hooks",
    "output-styles",
    "todos",
    "plans",
    "file-history",
    "history.jsonl",
    "keybindings.json",
    "session-env",
    "shell-snapshots",
];

/// What a Codex account keeps to itself. Everything else is shared.
const CODEX_OWN: &[&str] = &[
    "auth.json",
    "models_cache.json",
    "log",
    "memories",
    "tmp",
    MARKER,
];

fn home() -> PathBuf {
    sikemux_process::paths::home().unwrap_or_default()
}

fn shares(provider: &str, name: &str) -> bool {
    match provider {
        "claude" => CLAUDE_SHARED.contains(&name),
        "codex" => !CODEX_OWN.contains(&name),
        _ => false,
    }
}

/// `provider` and its home, when it keeps one account per folder.
fn accounts_home(provider: &str) -> Option<(AgentKind, &'static Home)> {
    let kind = AgentKind::from_name(provider)?;
    let entry = kind.entry();
    Some((kind, entry.home.as_ref().filter(|_| entry.accounts)?))
}

/// The folder an account of `provider` keeps its sign-in in: the one
/// `config_path` names, else the one the person's shell names, else the default.
pub fn root(provider: &str, config_path: Option<&str>) -> Option<PathBuf> {
    let (kind, _) = accounts_home(provider)?;
    account_root(kind, config_path, &BTreeMap::new())
}

fn default_root(provider: &str) -> Option<PathBuf> {
    Some(home().join(accounts_home(provider)?.1.dir))
}

/// The folder an account keeps its chats in. Accounts that share one can
/// take over each other's chats.
pub fn sessions_root(provider: &str, config_path: Option<&str>) -> Option<PathBuf> {
    let folder = accounts_home(provider)?.1.sessions?;
    let path = root(provider, config_path)?.join(folder);
    Some(sikemux_process::os::canonicalize(&path).unwrap_or(path))
}

pub fn sessions_shared(provider: &str, one: Option<&str>, other: Option<&str>) -> bool {
    sessions_root(provider, one).is_some_and(|root| sessions_root(provider, other) == Some(root))
}

/// Links the default folder's shared entries into an account Sikemux made.
/// Entries already there are left alone, so a file the account wrote itself
/// is never replaced.
pub fn link_shared(provider: &str, config_path: Option<&str>) {
    let (Some(account), Some(shared)) = (
        config_path.and_then(|path| root(provider, Some(path))),
        default_root(provider),
    ) else {
        return;
    };
    link_into(provider, &account, &shared);
}

fn link_into(provider: &str, account: &Path, shared: &Path) {
    if !account.join(MARKER).is_file() || account == shared {
        return;
    }
    if let Some(sessions) = accounts_home(provider).and_then(|(_, home)| home.sessions) {
        let _ = std::fs::create_dir_all(shared.join(sessions));
    }
    let Ok(entries) = std::fs::read_dir(shared) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let target = account.join(name);
        if !shares(provider, name) || target.symlink_metadata().is_ok() {
            continue;
        }
        if let Err(error) = sikemux_process::os::symlink(entry.path().as_path(), &target) {
            eprintln!("Could not share {name} with the account in {account:?}: {error}");
        }
    }
}

fn slug(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug: String = slug.chars().take(32).collect();
    if slug.is_empty() {
        "account".into()
    } else {
        slug
    }
}

/// What a new Claude account starts with from the default one: its tool
/// servers and which projects it trusts. Claude keeps both beside the
/// sign-in, so they cannot be linked.
fn claude_seed(default: &Value) -> Value {
    let mut seed = Map::new();
    seed.insert("hasCompletedOnboarding".into(), json!(true));
    for key in ["mcpServers", "theme", "lastOnboardingVersion"] {
        if let Some(value) = default.get(key) {
            seed.insert(key.into(), value.clone());
        }
    }
    if let Some(projects) = default.get("projects").and_then(Value::as_object) {
        let kept: Map<String, Value> = projects
            .iter()
            .map(|(path, project)| {
                let mut kept = Map::new();
                for key in [
                    "allowedTools",
                    "mcpServers",
                    "enabledMcpjsonServers",
                    "disabledMcpjsonServers",
                    "hasTrustDialogAccepted",
                ] {
                    if let Some(value) = project.get(key) {
                        kept.insert(key.into(), value.clone());
                    }
                }
                (path.clone(), Value::Object(kept))
            })
            .collect();
        seed.insert("projects".into(), Value::Object(kept));
    }
    Value::Object(seed)
}

fn make_account(provider: &str, name: &str, home: &Path) -> Result<String, String> {
    let prefix = accounts_home(provider)
        .ok_or("Only Claude and Codex keep separate accounts")?
        .1
        .dir;
    let base = format!("{prefix}-{}", slug(name));
    let mut folder = base.clone();
    let mut counter = 2;
    while home.join(&folder).exists() {
        folder = format!("{base}-{counter}");
        counter += 1;
    }
    let directory = home.join(&folder);
    write_private(
        &directory.join(MARKER),
        b"Made by Sikemux for one account. Links here point at the default account's shared files.\n",
    )?;
    if provider == "claude" {
        let default = std::fs::read(home.join(".claude.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or(Value::Null);
        let seed = serde_json::to_vec_pretty(&claude_seed(&default)).map_err(|e| e.to_string())?;
        write_private(&directory.join(".claude.json"), &seed)?;
    }
    Ok(format!("~/{folder}"))
}

/// The seed copies MCP servers whose environment may hold tokens, so the
/// folder and its files are the person's alone.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    sikemux_process::files::write_atomic(path, bytes, true).map_err(|error| error.to_string())
}

/// Makes a folder for another account, sharing what the default one has, and
/// returns it as a profile's config path.
pub fn add(provider: &str, name: &str) -> Result<String, String> {
    let path = make_account(provider, name, &home())?;
    link_shared(provider, Some(&path));
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_name_becomes_a_folder_name() {
        assert_eq!(slug("Work"), "work");
        assert_eq!(slug("  My Team / Max  "), "my-team-max");
        assert_eq!(slug("???"), "account");
    }

    #[test]
    fn a_new_claude_account_shares_chats_and_settings_but_not_its_sign_in() {
        let home = tempfile::tempdir().unwrap();
        let shared = home.path().join(".claude");
        std::fs::create_dir_all(shared.join("projects/-tmp")).unwrap();
        std::fs::write(shared.join("settings.json"), "{}").unwrap();
        std::fs::write(shared.join(".credentials.json"), "secret").unwrap();
        std::fs::write(
            home.path().join(".claude.json"),
            r#"{"oauthAccount":{"accountUuid":"me"},"mcpServers":{"x":{}},"projects":{"/p":{"hasTrustDialogAccepted":true,"lastCost":3}}}"#,
        )
        .unwrap();

        let path = make_account("claude", "Work", home.path()).unwrap();
        assert_eq!(path, "~/.claude-work");
        let account = home.path().join(".claude-work");
        assert!(account.join(MARKER).is_file());
        let seed: Value =
            serde_json::from_slice(&std::fs::read(account.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(seed["mcpServers"], json!({ "x": {} }));
        assert_eq!(
            seed["projects"]["/p"],
            json!({ "hasTrustDialogAccepted": true })
        );
        assert!(seed.get("oauthAccount").is_none());

        assert_eq!(
            make_account("claude", "Work", home.path()).unwrap(),
            "~/.claude-work-2"
        );
    }

    #[test]
    fn linking_shares_only_what_is_shared_and_keeps_what_is_there() {
        let home = tempfile::tempdir().unwrap();
        let shared = home.path().join(".codex");
        std::fs::create_dir_all(shared.join("sessions")).unwrap();
        std::fs::write(shared.join("config.toml"), "").unwrap();
        std::fs::write(shared.join("auth.json"), "{}").unwrap();
        let account = home.path().join(".codex-work");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("config.toml"), "own = true").unwrap();

        link_into("codex", &account, &shared);
        assert!(
            !account.join("sessions").exists(),
            "linked into a folder it did not make"
        );

        std::fs::write(account.join(MARKER), "").unwrap();
        link_into("codex", &account, &shared);
        assert_eq!(
            std::fs::read_link(account.join("sessions")).unwrap(),
            shared.join("sessions")
        );
        assert_eq!(
            std::fs::read_to_string(account.join("config.toml")).unwrap(),
            "own = true"
        );
        assert!(!account.join("auth.json").exists());

        assert!(shares("claude", "projects"));
        assert!(!shares("claude", ".credentials.json"));
        assert!(!shares("claude", "daemon"));
    }
}
