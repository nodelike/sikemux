//! Points each agent Sikemux launches at the Sikemux skills library, always by
//! adding to the agent's own skills and never by writing into its folders.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sikemux_process::user_environment::shell_var;

use crate::agent_catalog::AgentKind;
use crate::skills::{library, ready_library, LIBRARY_ENV};

/// `data_dir` with the skills library ready in it. Without one, agents launch
/// without Sikemux skills rather than pointed at a missing folder.
pub fn ready(data_dir: &Path) -> Option<&Path> {
    match ready_library(data_dir) {
        Ok(_) => Some(data_dir),
        Err(error) => {
            eprintln!("Sikemux skills are unavailable: {error}");
            None
        }
    }
}

/// Gives a chat agent the library in `data_dir`. Hermes and Grok take it in a
/// private home of their own, which their launcher makes.
pub(super) fn chat(
    data_dir: &Path,
    provider: &str,
    args: &mut Vec<String>,
    env: &mut BTreeMap<String, String>,
) {
    let library = library(data_dir);
    env.insert(LIBRARY_ENV.into(), library.to_string_lossy().into_owned());
    match AgentKind::from_name(provider) {
        Some(AgentKind::Claude) => {
            args.splice(0..0, skill_args(Some(data_dir), claude_args));
        }
        Some(AgentKind::Omp) => args.extend(skill_args(Some(data_dir), omp_args)),
        Some(AgentKind::Opencode) => {
            let existing = env
                .get("OPENCODE_CONFIG_CONTENT")
                .cloned()
                .or_else(inherited_opencode_config);
            if let Some(config) =
                chat_opencode_config(existing.as_deref(), &opencode_user_paths(), &library)
            {
                env.insert("OPENCODE_CONFIG_CONTENT".into(), config);
            }
        }
        _ => {}
    }
}

/// An agent's skill arguments, or none when they cannot be made.
pub fn skill_args(
    data_dir: Option<&Path>,
    make: impl FnOnce(&Path) -> std::io::Result<Vec<String>>,
) -> Vec<String> {
    let Some(data_dir) = data_dir else {
        return Vec::new();
    };
    make(data_dir).unwrap_or_else(|error| {
        eprintln!("Sikemux skills are unavailable to this agent: {error}");
        Vec::new()
    })
}

/// The `skills.paths` in the person's global OpenCode config.
pub fn opencode_user_paths() -> Vec<String> {
    let config_home = shell_var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            sikemux_process::paths::home()
                .unwrap_or_default()
                .join(".config")
        });
    opencode_own_paths(&config_home.join("opencode"))
}

pub fn inherited_opencode_config() -> Option<String> {
    shell_var("OPENCODE_CONFIG_CONTENT")
}

/// An OpenCode chat's inline config with the skills library added. A config
/// that is not plain JSON is left as the person wrote it, without the skills.
fn chat_opencode_config(
    existing: Option<&str>,
    own_paths: &[String],
    library: &Path,
) -> Option<String> {
    let mut config = match existing.map(str::trim).filter(|text| !text.is_empty()) {
        None => json!({}),
        Some(text) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)?,
    };
    add_opencode_paths(&mut config, own_paths, library);
    Some(config.to_string())
}

/// `--plugin-dir` loads the library as a plugin for this session only. One `=`
/// token, because the flag takes a list and would swallow the next plain word.
pub fn claude_args(data_dir: &Path) -> std::io::Result<Vec<String>> {
    let plugin = crate::skills::claude_plugin(data_dir)?;
    Ok(vec![format!("--plugin-dir={}", plugin.to_string_lossy())])
}

pub fn pi_args(data_dir: &Path) -> Vec<String> {
    vec![
        "--skill".into(),
        library(data_dir).to_string_lossy().into_owned(),
    ]
}

/// OMP adds the directories a `--config` file lists to its own.
pub fn omp_args(data_dir: &Path) -> std::io::Result<Vec<String>> {
    let library = library(data_dir);
    std::fs::create_dir_all(&library)?;
    let path = data_dir.join("omp-skills.yml");
    let quoted = serde_json::to_string(&library.to_string_lossy()).unwrap_or_default();
    std::fs::write(
        &path,
        format!("skills:\n  customDirectories:\n    - {quoted}\n"),
    )?;
    Ok(vec!["--config".into(), path.to_string_lossy().into_owned()])
}

/// OpenCode replaces a list set in `OPENCODE_CONFIG_CONTENT` rather than
/// joining it, so the paths from the user's own config ride along.
pub fn add_opencode_paths(config: &mut Value, own_paths: &[String], library: &Path) {
    let Some(root) = config.as_object_mut() else {
        return;
    };
    let skills = root.entry("skills").or_insert_with(|| json!({}));
    let Some(skills) = skills.as_object_mut() else {
        return;
    };
    let mut paths: Vec<String> = skills
        .get("paths")
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let library = library.to_string_lossy().into_owned();
    for path in own_paths.iter().chain(std::iter::once(&library)) {
        if !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    skills.insert("paths".into(), json!(paths));
}

/// The `skills.paths` in the user's global OpenCode config.
pub fn opencode_own_paths(config_dir: &Path) -> Vec<String> {
    ["opencode.json", "opencode.jsonc"]
        .iter()
        .filter_map(|name| std::fs::read_to_string(config_dir.join(name)).ok())
        .filter_map(|text| serde_json::from_str::<Value>(&plain_json(&text)).ok())
        .flat_map(|config| {
            config
                .pointer("/skills/paths")
                .and_then(Value::as_array)
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect()
}

/// JSON with comments and trailing commas, as OpenCode allows, made plain.
fn plain_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(char) = chars.next() {
        if in_string {
            out.push(char);
            match char {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (char, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(char);
            }
            ('/', Some('/')) => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for skipped in chars.by_ref() {
                    if last == '*' && skipped == '/' {
                        break;
                    }
                    last = skipped;
                }
            }
            (',', _) => {
                let rest: String = chars.clone().collect();
                if !matches!(rest.trim_start().chars().next(), Some('}' | ']')) {
                    out.push(char);
                }
            }
            _ => out.push(char),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = "/Users/me/Library/Application Support/sikemux/skills";

    #[test]
    fn opencode_keeps_the_users_paths_and_adds_the_library_once() {
        let mut config = json!({ "mcp": {}, "skills": { "paths": ["/inherited"] } });
        add_opencode_paths(
            &mut config,
            &["/own".into(), "/inherited".into()],
            Path::new(LIBRARY),
        );
        add_opencode_paths(&mut config, &[], Path::new(LIBRARY));
        assert_eq!(
            config["skills"]["paths"],
            json!(["/inherited", "/own", LIBRARY])
        );
        assert_eq!(config["mcp"], json!({}));
    }

    #[test]
    fn opencode_jsonc_keeps_its_paths_through_comments_and_trailing_commas() {
        let dir = std::env::temp_dir().join(format!("sikemux-opencode-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("opencode.jsonc"),
            "{\n  // my skills\n  \"skills\": { /* team */ \"paths\": [\"~/team//skills\", \"/a,b\",], },\n}\n",
        )
        .unwrap();
        assert_eq!(
            opencode_own_paths(&dir),
            vec!["~/team//skills".to_string(), "/a,b".to_string()]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn omp_gets_a_config_listing_the_library() {
        let data = std::env::temp_dir().join(format!("sikemux-omp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&data).unwrap();
        let args = omp_args(&data).unwrap();
        assert_eq!(args[0], "--config");
        let text = std::fs::read_to_string(&args[1]).unwrap();
        assert!(text.contains("customDirectories:"));
        assert!(text.contains(&library(&data).to_string_lossy().into_owned()));
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn claude_gets_the_plugin_as_one_token() {
        let data = std::env::temp_dir().join(format!("sikemux-claude-{}", uuid::Uuid::new_v4()));
        let args = claude_args(&data).unwrap();
        assert_eq!(args.len(), 1);
        assert!(args[0].starts_with("--plugin-dir=") && args[0].ends_with("skills-plugin"));
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn an_opencode_chat_keeps_an_inline_config_it_cannot_read_untouched() {
        let library = Path::new("/data/skills");
        let added = chat_opencode_config(Some(r#"{"provider":{"x":{}}}"#), &[], library).unwrap();
        assert!(added.contains("/data/skills") && added.contains("provider"));
        assert!(chat_opencode_config(None, &[], library)
            .unwrap()
            .contains("/data/skills"));
        assert_eq!(
            chat_opencode_config(Some("{ // mine\n \"provider\": {}, }"), &[], library),
            None
        );
    }
}
