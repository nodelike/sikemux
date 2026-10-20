//! The languages and environment files in each folder of a project. An
//! environment file is told by what is in it, not by its name, so
//! `.environment.prod` and `config/dev.env` count as much as `.env`.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::detect::inner_folders;
use super::read_text;

const LANGUAGES: [(&str, &[&str]); 5] = [
    (
        "java",
        &[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "mvnw",
            "gradlew",
        ],
    ),
    ("node", &["package.json"]),
    ("go", &["go.mod", "go.work"]),
    ("rust", &["Cargo.toml", "rust-toolchain.toml"]),
    (
        "python",
        &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"],
    ),
];

/// Files of `key=value` lines that configure a tool, not the processes started in the folder.
const TOOL_FILES: [&str; 11] = [
    ".npmrc",
    ".yarnrc",
    ".editorconfig",
    ".sdkmanrc",
    ".tool-versions",
    ".nvmrc",
    ".python-version",
    ".envrc",
    ".gitconfig",
    ".gitattributes",
    ".gitmodules",
];

/// Source, config and document files, which can hold `key=value` lines without being environment files.
const OTHER_EXTENSIONS: [&str; 37] = [
    "json",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "md",
    "lock",
    "toml",
    "yaml",
    "yml",
    "xml",
    "gradle",
    "kts",
    "java",
    "kt",
    "go",
    "rs",
    "py",
    "rb",
    "php",
    "swift",
    "c",
    "h",
    "cpp",
    "sh",
    "bash",
    "zsh",
    "properties",
    "ini",
    "cfg",
    "conf",
    "txt",
    "html",
    "css",
    "sql",
];

const MAX_ENV_BYTES: u64 = 64 * 1024;

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_.]*)\s*=\s*(.*)$").expect("valid pattern")
});
static UPPER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]*$").expect("valid pattern"));
static SCRIPT_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:(?:dotenv(?:-cli)?|env-cmd)\s+(?:-[ef]|--file|--env)\s+|--env-file[= ])([^\s"'&;|]+)"#)
        .expect("valid pattern")
});
static ENVRC_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*dotenv(?:_if_exists)?\s+(\S+)").expect("valid pattern"));
static MAKE_INCLUDE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^-?include\s+(\S+)").expect("valid pattern"));
static ENVIRONMENT_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(dev|development|local|test|testing|stage|staging|qa|uat|preview|demo|sandbox|ci|prod|production|prd|live)$")
        .expect("valid pattern")
});
static PRODUCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)prod|^prd$|^live$").expect("valid pattern"));

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// Relative to the project, `.` for its root.
    pub path: String,
    pub languages: Vec<Language>,
    pub env_files: Vec<EnvFile>,
    pub environments: Vec<Environment>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    pub id: &'static str,
    /// The files that show it, such as `pom.xml`.
    pub markers: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvFile {
    /// Relative to the folder.
    pub path: String,
    /// The environment it belongs to; none when every environment reads it.
    pub environment: Option<String>,
    /// Kept on one machine to override the others, such as `.env.local`.
    pub local: bool,
    /// An example to copy from, such as `.env.example`, which nothing reads.
    pub template: bool,
    /// The project files that load it, such as `package.json`.
    pub loaded_by: Vec<String>,
    pub variables: Vec<Variable>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Variable {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub name: String,
    /// Looks like production, so it is never loaded without the person choosing it.
    pub production: bool,
}

/// Every folder of the project with a language or an environment file in it.
pub fn folders(project: &Path) -> Vec<Folder> {
    std::iter::once(".".to_owned())
        .chain(inner_folders(project))
        .filter_map(|folder| {
            let found = folder_at(project, &folder);
            (!found.languages.is_empty() || !found.env_files.is_empty()).then_some(found)
        })
        .collect()
}

/// What processes in `folder` get from its files when `environment` is chosen: the files every
/// environment shares, then its own, each followed by its local copy, a later one winning.
/// An empty `environment` reads only the shared files.
pub fn variables(project: &Path, folder: &str, environment: &str) -> Vec<(String, String)> {
    let found = folder_at(project, folder);
    let mut files: Vec<&EnvFile> = found
        .env_files
        .iter()
        .filter(|file| {
            !file.template
                && file
                    .environment
                    .as_deref()
                    .is_none_or(|name| name == environment)
        })
        .collect();
    files.sort_by_key(|file| (file.environment.is_some(), file.local));
    files
        .into_iter()
        .flat_map(|file| {
            file.variables
                .iter()
                .map(|variable| (variable.key.clone(), variable.value.clone()))
        })
        .collect()
}

pub fn folder_at(project: &Path, folder: &str) -> Folder {
    let dir = if folder == "." {
        project.to_path_buf()
    } else {
        project.join(folder)
    };
    let names: BTreeSet<String> = std::fs::read_dir(&dir)
        .map(|read| {
            read.flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let languages = LANGUAGES
        .iter()
        .filter_map(|(id, markers)| {
            let markers: Vec<String> = markers
                .iter()
                .filter(|marker| names.contains(**marker))
                .map(|marker| (*marker).to_owned())
                .collect();
            (!markers.is_empty()).then_some(Language { id, markers })
        })
        .collect();
    let env_files = env_files(&dir, &names);
    let environments = env_files
        .iter()
        .filter(|file| !file.template)
        .filter_map(|file| file.environment.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|name| Environment {
            production: PRODUCTION.is_match(&name),
            name,
        })
        .collect();
    Folder {
        path: folder.to_owned(),
        languages,
        env_files,
        environments,
    }
}

fn env_files(dir: &Path, names: &BTreeSet<String>) -> Vec<EnvFile> {
    let loaded = loaded_files(dir, names);
    let mut paths: BTreeSet<String> = names
        .iter()
        .filter(|name| looks_like_env_name(name))
        .cloned()
        .collect();
    paths.extend(loaded.iter().map(|(path, _)| path.clone()));
    paths
        .into_iter()
        .filter_map(|path| {
            let text = read_env_text(&dir.join(&path))?;
            let variables = parse(&text);
            let loaders: Vec<String> = loaded
                .iter()
                .filter(|(file, _)| *file == path)
                .map(|(_, by)| by.clone())
                .collect();
            if variables.is_empty() || (loaders.is_empty() && !reads_as_env(&text)) {
                return None;
            }
            let name = path.rsplit('/').next().unwrap_or(&path);
            let (environment, local, template) = environment_of(name);
            let for_containers =
                !loaders.is_empty() && loaders.iter().all(|by| is_compose_file(by));
            if for_containers && !environment.as_deref().is_some_and(is_environment_name) {
                return None;
            }
            Some(EnvFile {
                path,
                environment,
                local,
                template,
                loaded_by: loaders,
                variables,
            })
        })
        .collect()
}

fn looks_like_env_name(name: &str) -> bool {
    if TOOL_FILES.contains(&name) {
        return false;
    }
    let extension = name
        .trim_start_matches('.')
        .rsplit_once('.')
        .map(|(_, ext)| ext);
    !extension.is_some_and(|ext| OTHER_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

fn read_env_text(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > MAX_ENV_BYTES {
        return None;
    }
    read_text(path)
}

/// Most of its lines set a variable, and most of those are named in capitals.
fn reads_as_env(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let keys: Vec<&str> = lines
        .iter()
        .filter_map(|line| LINE.captures(line).and_then(|found| found.get(1)))
        .map(|key| key.as_str())
        .collect();
    let upper = keys.iter().filter(|key| UPPER.is_match(key)).count();
    !keys.is_empty() && keys.len() * 5 >= lines.len() * 4 && upper * 5 >= keys.len() * 4
}

pub fn parse(text: &str) -> Vec<Variable> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let found = LINE.captures(line)?;
            Some(Variable {
                key: found[1].to_owned(),
                value: unquote(found[2].trim()),
            })
        })
        .collect()
}

fn unquote(raw: &str) -> String {
    for quote in ['"', '\''] {
        if let Some(inner) = raw
            .strip_prefix(quote)
            .and_then(|rest| rest.split_once(quote))
        {
            return if quote == '"' {
                inner.0.replace("\\n", "\n").replace("\\\"", "\"")
            } else {
                inner.0.to_owned()
            };
        }
    }
    raw.split_once(" #")
        .map_or(raw, |(value, _)| value)
        .trim_end()
        .to_owned()
}

/// `.env.stage.local` is the stage environment's, kept on this machine; `.env` is every environment's.
fn environment_of(name: &str) -> (Option<String>, bool, bool) {
    let mut words: Vec<String> = name
        .trim_start_matches('.')
        .split(['.', '-', '_'])
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .filter(|word| {
            !matches!(
                word.as_str(),
                "env" | "environment" | "environments" | "dotenv"
            )
        })
        .collect();
    let template = words
        .iter()
        .any(|word| matches!(word.as_str(), "example" | "sample" | "template" | "dist"));
    let local = words.iter().any(|word| word == "local");
    words.retain(|word| word != "local");
    let named = words.iter().rev().find(|word| is_environment_name(word));
    let environment =
        (!words.is_empty() && !template).then(|| named.cloned().unwrap_or_else(|| words.join("-")));
    (environment, local, template)
}

/// Files the folder's own scripts load, with what loads each.
fn loaded_files(dir: &Path, names: &BTreeSet<String>) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut take = |text: &str, pattern: &Regex, by: &str| {
        for captured in pattern.captures_iter(text) {
            let path = captured[1].trim_start_matches("./").to_owned();
            if !path.contains("..") && dir.join(&path).is_file() {
                found.push((path, by.to_owned()));
            }
        }
    };
    if let Some(text) = names
        .get("package.json")
        .and_then(|name| read_text(&dir.join(name)))
    {
        take(&text, &SCRIPT_FILE, "package.json");
    }
    if let Some(text) = names
        .get(".envrc")
        .and_then(|name| read_text(&dir.join(name)))
    {
        take(&text, &ENVRC_FILE, ".envrc");
    }
    if let Some(text) = names
        .get("Makefile")
        .and_then(|name| read_text(&dir.join(name)))
    {
        take(&text, &MAKE_INCLUDE, "Makefile");
    }
    for name in names.iter().filter(|name| is_compose_file(name)) {
        if let Some(text) = read_text(&dir.join(name)) {
            for path in compose_env_files(&text) {
                let path = path.trim_start_matches("./").to_owned();
                if !path.contains("..") && dir.join(&path).is_file() {
                    found.push((path, name.clone()));
                }
            }
        }
    }
    found
}

/// A word people name an environment with, such as `stage`, as opposed to a service such as `web`.
fn is_environment_name(name: &str) -> bool {
    ENVIRONMENT_NAME.is_match(name)
}

fn is_compose_file(name: &str) -> bool {
    (name.starts_with("docker-compose") || name.starts_with("compose"))
        && (name.ends_with(".yml") || name.ends_with(".yaml"))
}

/// The files named under `env_file:`, given as one path or as a list of them.
fn compose_env_files(text: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut listing: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("env_file:") {
            let rest = rest.trim().trim_matches(['"', '\'']);
            if rest.is_empty() {
                listing = Some(indent);
            } else {
                files.push(rest.to_owned());
                listing = None;
            }
            continue;
        }
        match listing {
            Some(at) if indent > at || trimmed.starts_with('-') => {
                if let Some(item) = trimmed.strip_prefix('-') {
                    let item = item.trim();
                    let item = item.strip_prefix("path:").map_or(item, str::trim);
                    files.push(item.trim_matches(['"', '\'']).to_owned());
                }
            }
            Some(_) if !trimmed.is_empty() => listing = None,
            _ => {}
        }
    }
    files
}
