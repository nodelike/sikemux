//! The agents the person added themselves, each an ACP program listed in `custom-agents.json`
//! in Sikemux's config folder. They join the built-in catalog without being built into it.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use sikemux_process::sync::Locked;
use sikemux_process::vars::CUSTOM_AGENTS_PATH;

use crate::agent_catalog::{AcpModes, AgentEntry, ChatDriver, Resume, Terminal, AGENTS};

const FILE_NAME: &str = "custom-agents.json";
const MAX_TYPE_LEN: usize = 64;
const DEFAULT_ABOUT: &str = "Custom ACP agent";
const DEFAULT_BRAND: &str = "#8b949e";
const DEFAULT_ARGS: &[&str] = &["acp"];
const DEFAULT_PROBE: &[&str] = &["--version"];
const DEFAULT_COMPACT: &str = "compact";
/// How long the file's last look is trusted before its size and date are read again.
const RECHECK_AFTER: Duration = Duration::from_secs(1);

#[derive(Deserialize)]
struct Listing {
    #[serde(default)]
    agents: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    r#type: String,
    label: Option<String>,
    about: Option<String>,
    command: String,
    args: Option<Vec<String>>,
    brand: Option<String>,
    env: Option<BTreeMap<String, String>>,
    probe: Option<Vec<String>>,
    compact: Option<String>,
    asks_before_tools: Option<bool>,
    modes: Option<RawModes>,
}

#[derive(Deserialize)]
struct RawModes {
    normal: String,
    bypass: String,
}

#[derive(Default)]
struct Parsed {
    entries: Vec<AgentEntry>,
    env: BTreeMap<String, BTreeMap<String, String>>,
}

struct Loaded {
    path: Option<PathBuf>,
    stamp: Option<(SystemTime, u64)>,
    checked: Instant,
    entries: &'static [AgentEntry],
    env: BTreeMap<String, BTreeMap<String, String>>,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn leak_all(items: Vec<String>) -> &'static [&'static str] {
    let items: Vec<&'static str> = items.into_iter().map(leak).collect();
    Box::leak(items.into_boxed_slice())
}

fn file_path() -> Option<PathBuf> {
    sikemux_process::paths::env_path(CUSTOM_AGENTS_PATH)
        .or_else(|| Some(sikemux_process::paths::config_dir()?.join(FILE_NAME)))
}

fn valid_type(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TYPE_LEN
        && name
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

fn hex_colour(brand: &str) -> Option<String> {
    let digits = brand.strip_prefix('#')?;
    (digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| brand.to_ascii_lowercase())
}

fn non_blank(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn entry(
    raw: Raw,
    taken: &HashSet<String>,
) -> Result<(AgentEntry, BTreeMap<String, String>), String> {
    let kind = raw.r#type.trim().to_owned();
    if !valid_type(&kind) {
        return Err(format!(
            "type {kind:?} must be 1 to {MAX_TYPE_LEN} characters of a-z, 0-9, _ and -"
        ));
    }
    if AGENTS.iter().any(|built_in| built_in.kind == kind) {
        return Err(format!("type {kind:?} is a built-in agent"));
    }
    if taken.contains(&kind) {
        return Err(format!("type {kind:?} is listed twice"));
    }
    let command = raw.command.trim().to_owned();
    if command.is_empty() {
        return Err(format!("{kind} has no command"));
    }
    let brand = match non_blank(raw.brand) {
        None => DEFAULT_BRAND.to_owned(),
        Some(brand) => hex_colour(&brand)
            .ok_or_else(|| format!("{kind} brand {brand:?} is not a #rrggbb colour"))?,
    };
    let env = raw.env.unwrap_or_default();
    if let Some(name) = env
        .iter()
        .find(|(name, value)| name.is_empty() || name.contains(['=', '\0']) || value.contains('\0'))
        .map(|(name, _)| name)
    {
        return Err(format!(
            "{kind} has the invalid environment variable {name:?}"
        ));
    }
    let args = raw
        .args
        .unwrap_or_else(|| DEFAULT_ARGS.iter().map(|arg| (*arg).to_owned()).collect());
    let probe = raw
        .probe
        .filter(|probe| !probe.is_empty())
        .unwrap_or_else(|| DEFAULT_PROBE.iter().map(|arg| (*arg).to_owned()).collect());
    let modes = raw.modes.map(|modes| AcpModes {
        normal: leak(modes.normal),
        bypass: leak(modes.bypass),
    });
    let label = non_blank(raw.label).unwrap_or_else(|| kind.clone());
    let about = non_blank(raw.about).unwrap_or_else(|| DEFAULT_ABOUT.to_owned());
    let compact = non_blank(raw.compact).unwrap_or_else(|| DEFAULT_COMPACT.to_owned());
    let entry = AgentEntry {
        kind: leak(kind),
        label: leak(label),
        about: leak(about),
        command: leak(command),
        brand: leak(brand),
        maker: None,
        featured: false,
        accounts: false,
        usage: false,
        free_models: false,
        deletable_chats: false,
        resumes_by_path: false,
        relocatable_sessions: false,
        history_before_start: false,
        accounts_by_hand: false,
        bypass: true,
        auto: false,
        commit_models: &[],
        careful_model: None,
        efforts: &[],
        install: None,
        login: None,
        update: None,
        models_since: &[],
        latest: None,
        probe: leak_all(probe),
        install_paths: &[],
        git_shim_args: &[],
        home: None,
        terminal: Terminal {
            normal: &[],
            bypass: None,
            auto: None,
            effort: None,
            resume: Resume::Flag("--resume"),
            with_choices: &[],
        },
        chat: Some(ChatDriver::Acp {
            args: leak_all(args),
            modes,
            compact: leak(compact),
            asks_before_tools: raw.asks_before_tools.unwrap_or(false),
        }),
        providers: None,
        routed: None,
    };
    Ok((entry, env))
}

/// The agents `text` lists. An entry that cannot be used is skipped and said so, so one
/// mistake never hides the others.
fn parse(text: &str, source: &str) -> Parsed {
    let listing: Listing = match serde_json::from_str(text) {
        Ok(listing) => listing,
        Err(error) => {
            eprintln!("sikemux core: {source} is not a list of custom agents: {error}");
            return Parsed::default();
        }
    };
    let mut parsed = Parsed::default();
    let mut taken = HashSet::new();
    for (at, value) in listing.agents.into_iter().enumerate() {
        let built = serde_json::from_value::<Raw>(value)
            .map_err(|error| error.to_string())
            .and_then(|raw| entry(raw, &taken));
        match built {
            Ok((agent, env)) => {
                taken.insert(agent.kind.to_owned());
                parsed.env.insert(agent.kind.to_owned(), env);
                parsed.entries.push(agent);
            }
            Err(error) => {
                eprintln!(
                    "sikemux core: {source} skips custom agent #{}: {error}",
                    at + 1
                );
            }
        }
    }
    parsed
}

fn stamp_of(path: &PathBuf) -> Option<(SystemTime, u64)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn read(path: Option<PathBuf>, stamp: Option<(SystemTime, u64)>) -> Loaded {
    let parsed = match (&path, stamp) {
        (Some(path), Some(_)) => match fs::read_to_string(path) {
            Ok(text) => parse(&text, &path.display().to_string()),
            Err(error) => {
                eprintln!("sikemux core: cannot read {}: {error}", path.display());
                Parsed::default()
            }
        },
        _ => Parsed::default(),
    };
    Loaded {
        path,
        stamp,
        checked: Instant::now(),
        entries: Box::leak(parsed.entries.into_boxed_slice()),
        env: parsed.env,
    }
}

/// Runs `read` on the list as the file has it now. The file is looked at again only when its
/// size or date changed, and no more than once a second.
fn with_loaded<R>(read_from: impl FnOnce(&Loaded) -> R) -> R {
    let mut slot = LOADED.locked();
    let fresh = slot
        .as_ref()
        .is_some_and(|loaded| loaded.checked.elapsed() < RECHECK_AFTER);
    if !fresh {
        let path = file_path();
        let stamp = path.as_ref().and_then(stamp_of);
        match slot.as_mut() {
            Some(loaded) if loaded.path == path && loaded.stamp == stamp => {
                loaded.checked = Instant::now();
            }
            _ => *slot = Some(read(path, stamp)),
        }
    }
    match slot.as_ref() {
        Some(loaded) => read_from(loaded),
        None => read_from(&read(None, None)),
    }
}

/// The agents the person added, in the order they listed them.
pub fn entries() -> &'static [AgentEntry] {
    with_loaded(|loaded| loaded.entries)
}

/// The custom agent called `name`.
pub fn agent(name: &str) -> Option<&'static AgentEntry> {
    entries().iter().find(|entry| entry.kind == name)
}

/// The variables the person set for the custom agent `provider`.
pub fn env_for(provider: &str) -> BTreeMap<String, String> {
    with_loaded(|loaded| loaded.env.get(provider).cloned().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_what_cannot_be_used_and_keeps_the_rest() {
        let parsed = parse(
            r##"{ "agents": [
                { "type": "dima", "command": "/bin/dima", "env": { "A": "b" } },
                { "type": "claude", "command": "claude" },
                { "type": "Bad", "command": "x" },
                { "type": "nocmd", "command": " " },
                { "type": "dima", "command": "/bin/other" },
                { "type": "colour", "command": "x", "brand": "red" },
                { "type": "full", "command": "x", "label": "Full", "brand": "#ABCDEF",
                  "args": ["serve"], "modes": { "normal": "n", "bypass": "b" } }
            ] }"##,
            "test",
        );
        let kinds: Vec<_> = parsed.entries.iter().map(|entry| entry.kind).collect();
        assert_eq!(kinds, ["dima", "full"]);
        let dima = &parsed.entries[0];
        assert_eq!(dima.label, "dima");
        assert_eq!(dima.brand, "#8b949e");
        assert_eq!(dima.probe, ["--version"]);
        assert!(matches!(
            &dima.chat,
            Some(ChatDriver::Acp {
                args: ["acp"],
                modes: None,
                compact: "compact",
                asks_before_tools: false
            })
        ));
        assert_eq!(parsed.env["dima"]["A"], "b");
        assert_eq!(parsed.entries[1].brand, "#abcdef");
    }
}
