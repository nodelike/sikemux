//! The agents the person added themselves, each an ACP program listed in `custom-agents.json`
//! in Sikemux's config folder. They join the built-in catalog without being built into it.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
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

/// One agent as `custom-agents.json` lists it. Everything but `type` and `command` may be left
/// out, and the defaults fill in what is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAgentConfig {
    /// Its id, such as `dima`.
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asks_before_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<CustomAgentModes>,
}

/// The ACP session modes that ask before acting and that do not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomAgentModes {
    pub normal: String,
    pub bypass: String,
}

#[derive(Default)]
struct Parsed {
    entries: Vec<AgentEntry>,
    env: BTreeMap<String, BTreeMap<String, String>>,
}

struct Loaded {
    path: Option<PathBuf>,
    stamp: Option<(SystemTime, u64)>,
    raw: Option<Vec<u8>>,
    checked: Instant,
    entries: &'static [AgentEntry],
    env: BTreeMap<String, BTreeMap<String, String>>,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn leak_all(items: Vec<String>) -> &'static [&'static str] {
    let items: Vec<&'static str> = items.into_iter().map(leak).collect();
    Box::leak(items.into_boxed_slice())
}

pub fn file_path() -> Option<PathBuf> {
    sikemux_process::paths::env_path(CUSTOM_AGENTS_PATH)
        .or_else(|| Some(sikemux_process::paths::config_dir()?.join(FILE_NAME)))
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
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

/// `raw` as it will be kept: trimmed, and refused when it cannot be used. `taken` holds the ids
/// listed before it.
fn check(raw: CustomAgentConfig, taken: &HashSet<String>) -> Result<CustomAgentConfig, String> {
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
        None => None,
        Some(brand) => Some(
            hex_colour(&brand)
                .ok_or_else(|| format!("{kind} brand {brand:?} is not a #rrggbb colour"))?,
        ),
    };
    if let Some(name) = raw
        .env
        .iter()
        .flatten()
        .find(|(name, value)| name.is_empty() || name.contains(['=', '\0']) || value.contains('\0'))
        .map(|(name, _)| name)
    {
        return Err(format!(
            "{kind} has the invalid environment variable {name:?}"
        ));
    }
    let modes = match raw.modes {
        None => None,
        Some(modes) => {
            let normal = modes.normal.trim().to_owned();
            let bypass = modes.bypass.trim().to_owned();
            if normal.is_empty() || bypass.is_empty() {
                return Err(format!("{kind} modes need both normal and bypass"));
            }
            Some(CustomAgentModes { normal, bypass })
        }
    };
    Ok(CustomAgentConfig {
        r#type: kind,
        label: non_blank(raw.label),
        about: non_blank(raw.about),
        command,
        args: raw.args,
        brand,
        env: raw.env.filter(|env| !env.is_empty()),
        probe: raw.probe.filter(|probe| !probe.is_empty()),
        compact: non_blank(raw.compact),
        asks_before_tools: raw.asks_before_tools,
        modes,
    })
}

/// The list entry for an agent `check` let through, with its variables.
fn entry(raw: CustomAgentConfig) -> (AgentEntry, BTreeMap<String, String>) {
    let kind = raw.r#type;
    let command = raw.command;
    let brand = raw.brand.unwrap_or_else(|| DEFAULT_BRAND.to_owned());
    let env = raw.env.unwrap_or_default();
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
    (entry, env)
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
        let built = serde_json::from_value::<CustomAgentConfig>(value)
            .map_err(|error| error.to_string())
            .and_then(|raw| check(raw, &taken))
            .map(entry);
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

fn read_bytes(path: &Path) -> Option<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            eprintln!("sikemux core: cannot read {}: {error}", path.display());
            None
        }
    }
}

fn build_loaded(
    path: Option<PathBuf>,
    stamp: Option<(SystemTime, u64)>,
    raw: Option<Vec<u8>>,
) -> Loaded {
    let parsed = match (&path, &raw) {
        (Some(path), Some(bytes)) => match std::str::from_utf8(bytes) {
            Ok(text) => parse(text, &path.display().to_string()),
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
        raw,
        checked: Instant::now(),
        entries: Box::leak(parsed.entries.into_boxed_slice()),
        env: parsed.env,
    }
}

fn sync_loaded(slot: &mut Option<Loaded>, force: bool) -> bool {
    if !force
        && slot
            .as_ref()
            .is_some_and(|loaded| loaded.checked.elapsed() < RECHECK_AFTER)
    {
        return false;
    }
    let path = file_path();
    let stamp = path.as_ref().and_then(stamp_of);
    if !force {
        if let Some(loaded) = slot.as_mut() {
            if loaded.path == path && loaded.stamp == stamp {
                loaded.checked = Instant::now();
                return false;
            }
        }
    }
    let raw = match (&path, stamp) {
        (Some(path), Some(_)) => read_bytes(path),
        _ => None,
    };
    if let Some(loaded) = slot.as_mut() {
        if loaded.path == path && loaded.raw == raw {
            loaded.stamp = stamp;
            loaded.checked = Instant::now();
            return false;
        }
    }
    let had_loaded = slot.is_some();
    *slot = Some(build_loaded(path, stamp, raw));
    if had_loaded {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        true
    } else {
        false
    }
}

/// Runs `read` on the list as the file has it now. The file is looked at again only when its
/// size or date changed, and no more than once a second.
fn with_loaded<R>(read_from: impl FnOnce(&Loaded) -> R) -> R {
    let (result, changed) = {
        let mut slot = LOADED.locked();
        let changed = sync_loaded(&mut slot, false);
        let result = match slot.as_ref() {
            Some(loaded) => read_from(loaded),
            None => read_from(&build_loaded(None, None, None)),
        };
        (result, changed)
    };
    if changed {
        crate::launch::discovery::forget_custom();
    }
    result
}

/// Re-reads the file immediately if its bytes changed, and answers with the current generation.
pub fn refresh() -> u64 {
    let changed = {
        let mut slot = LOADED.locked();
        sync_loaded(&mut slot, true)
    };
    if changed {
        crate::launch::discovery::forget_custom();
    }
    generation()
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

/// Whether `kind` is an agent the person added, as opposed to a built-in one.
pub fn is_custom(kind: &str) -> bool {
    entries().iter().any(|entry| entry.kind == kind)
}

/// Serializes changes to the file, so two adds never overwrite one another.
static WRITING: Mutex<()> = Mutex::new(());

/// The list in the file as it stands, each entry as the person wrote it, even one that cannot
/// be used. A missing file is an empty list; one that is not a list is an error, so it is never
/// written over.
fn stored(
    path: &Path,
) -> Result<
    (
        serde_json::Map<String, serde_json::Value>,
        Vec<serde_json::Value>,
    ),
    String,
> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok((serde_json::Map::new(), Vec::new()));
    }
    let serde_json::Value::Object(mut top) = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not a list of custom agents: {error}", path.display()))?
    else {
        return Err(format!("{} is not a list of custom agents", path.display()));
    };
    let agents = match top.remove("agents") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(agents)) => agents,
        Some(_) => {
            return Err(format!(
                "{} has an \"agents\" that is not a list",
                path.display()
            ))
        }
    };
    Ok((top, agents))
}

/// Keeps `agents` as the file's list, through a rename, and has the next read see it at once.
fn store(
    path: &Path,
    mut top: serde_json::Map<String, serde_json::Value>,
    agents: Vec<serde_json::Value>,
) -> Result<(), String> {
    top.insert("agents".to_owned(), serde_json::Value::Array(agents));
    let mut text = serde_json::to_string_pretty(&top).map_err(|error| error.to_string())?;
    text.push('\n');
    sikemux_process::files::write_atomic(path, text.as_bytes(), true)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    refresh();
    crate::launch::discovery::forget_custom();
    Ok(())
}

fn type_of(value: &serde_json::Value) -> Option<&str> {
    value.get("type")?.as_str().map(str::trim)
}

fn valid_configs(agents: &[serde_json::Value]) -> Vec<CustomAgentConfig> {
    let mut taken = HashSet::new();
    let mut listed = Vec::new();
    for value in agents {
        let config = serde_json::from_value::<CustomAgentConfig>(value.clone())
            .map_err(|error| error.to_string())
            .and_then(|raw| check(raw, &taken));
        if let Ok(config) = config {
            taken.insert(config.r#type.clone());
            listed.push(config);
        }
    }
    listed
}

/// The agents the person added that can be used, as the file lists them.
pub fn list() -> Vec<CustomAgentConfig> {
    let Some(path) = file_path() else {
        return Vec::new();
    };
    let Ok((_, agents)) = stored(&path) else {
        return Vec::new();
    };
    valid_configs(&agents)
}

/// Adds `config` to the file and returns it as kept. It is refused for the reasons the file's
/// own entries are skipped: an id that is not a plain name, one that is a built-in agent or
/// already listed, no command, a brand that is not a colour, or a bad variable.
pub fn add(config: CustomAgentConfig) -> Result<CustomAgentConfig, String> {
    let path = file_path().ok_or("Sikemux has no config folder to keep custom agents in")?;
    let _writing = WRITING.locked();
    let (top, mut agents) = stored(&path)?;
    let taken: HashSet<String> = valid_configs(&agents)
        .into_iter()
        .map(|agent| agent.r#type)
        .collect();
    let config = check(config, &taken)?;
    // An entry with this id that cannot be used is replaced rather than left beside it.
    agents.retain(|value| type_of(value) != Some(config.r#type.as_str()));
    agents.push(serde_json::to_value(&config).map_err(|error| error.to_string())?);
    store(&path, top, agents)?;
    Ok(config)
}

/// Takes the custom agent `kind` out of the file. A built-in agent is refused; whether an agent
/// was there to take out is the answer.
pub fn remove(kind: &str) -> Result<bool, String> {
    let kind = kind.trim();
    if AGENTS.iter().any(|built_in| built_in.kind == kind) {
        return Err(format!("{kind} is a built-in agent and cannot be removed"));
    }
    let path = file_path().ok_or("Sikemux has no config folder to keep custom agents in")?;
    let _writing = WRITING.locked();
    let (top, mut agents) = stored(&path)?;
    let before = agents.len();
    agents.retain(|value| type_of(value) != Some(kind));
    if agents.len() == before {
        return Ok(false);
    }
    store(&path, top, agents)?;
    Ok(true)
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

    /// Holds the environment variable that names the file while a test uses it.
    static ENV: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os(CUSTOM_AGENTS_PATH);
            std::env::set_var(CUSTOM_AGENTS_PATH, path);
            *LOADED.locked() = None;
            crate::launch::discovery::forget_custom();
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(previous) => std::env::set_var(CUSTOM_AGENTS_PATH, previous),
                None => std::env::remove_var(CUSTOM_AGENTS_PATH),
            }
            *LOADED.locked() = None;
            crate::launch::discovery::forget_custom();
        }
    }

    fn config(kind: &str, command: &str) -> CustomAgentConfig {
        CustomAgentConfig {
            r#type: kind.to_owned(),
            command: command.to_owned(),
            ..CustomAgentConfig::default()
        }
    }

    #[test]
    fn added_agents_are_listed_found_and_removed_at_once() {
        let _env = ENV.locked();
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("sub").join(FILE_NAME);
        let _guard = EnvGuard::set(&file);

        assert!(list().is_empty());
        assert!(!is_custom("dima"));
        let mut dima = config(" dima ", " /bin/dima ");
        dima.label = Some("Dima".to_owned());
        dima.brand = Some("#ABCDEF".to_owned());
        dima.env = Some(BTreeMap::from([("KEY".to_owned(), "value".to_owned())]));
        let kept = add(dima).unwrap();
        assert_eq!(kept.r#type, "dima");
        assert_eq!(kept.command, "/bin/dima");
        assert_eq!(kept.brand.as_deref(), Some("#abcdef"));

        assert!(is_custom("dima"));
        assert_eq!(list(), [kept]);
        let found = agent("dima").unwrap();
        assert_eq!((found.label, found.command), ("Dima", "/bin/dima"));
        assert_eq!(env_for("dima")["KEY"], "value");
        assert!(crate::agent_catalog::agent("dima").is_some());

        // The file is the camelCase list the loader reads, and nothing is left beside it.
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"type\": \"dima\""), "{text}");
        let leftovers = fs::read_dir(file.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1);

        let again = add(config("dima", "/bin/other")).unwrap_err();
        assert!(again.contains("listed twice"), "{again}");
        let built_in = add(config("claude", "claude")).unwrap_err();
        assert!(built_in.contains("built-in"), "{built_in}");
        assert!(add(config("Bad Id", "x")).is_err());
        assert!(add(config("nocmd", "  ")).is_err());
        let mut colour = config("colour", "x");
        colour.brand = Some("red".to_owned());
        assert!(add(colour).is_err());
        let mut variable = config("variable", "x");
        variable.env = Some(BTreeMap::from([("A=B".to_owned(), "c".to_owned())]));
        assert!(add(variable).is_err());
        let mut blank_modes = config("blank-modes", "x");
        blank_modes.modes = Some(CustomAgentModes {
            normal: " ".to_owned(),
            bypass: "yolo".to_owned(),
        });
        assert!(add(blank_modes).is_err());
        assert_eq!(list().len(), 1);

        add(config("other", "other")).unwrap();
        assert_eq!(list().len(), 2);
        let refused = remove("claude").unwrap_err();
        assert!(refused.contains("built-in"), "{refused}");
        assert_eq!(remove("dima"), Ok(true));
        assert_eq!(remove("dima"), Ok(false));
        assert!(agent("dima").is_none());
        assert!(!is_custom("dima"));
        assert!(is_custom("other"));
        assert_eq!(remove("other"), Ok(true));
        assert!(entries().is_empty());
    }

    #[test]
    fn a_file_that_is_not_a_list_is_never_written_over() {
        let _env = ENV.locked();
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join(FILE_NAME);
        let _guard = EnvGuard::set(&file);
        fs::write(&file, "not json").unwrap();
        assert!(add(config("dima", "dima")).is_err());
        assert!(remove("dima").is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "not json");

        // An entry that cannot be used stays when another is added, and is replaced by its id.
        fs::write(
            &file,
            r#"{ "agents": [ { "type": "broken", "command": " " } ], "extra": 1 }"#,
        )
        .unwrap();
        add(config("broken", "fixed")).unwrap();
        add(config("fine", "fine")).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"extra\": 1"), "{text}");
        let kinds: Vec<_> = list().into_iter().map(|agent| agent.r#type).collect();
        assert_eq!(kinds, ["broken", "fine"]);
    }
}
