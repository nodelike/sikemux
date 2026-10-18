//! Points each agent Sikemux launches at the Sikemux skills library, always by
//! adding to the agent's own skills and never by writing into its folders.

use std::path::Path;

use serde_json::json;

/// The `skills.external_dirs` a Hermes `config.yaml` already lists, with the
/// library added, ready for `hermes config set`.
pub fn hermes_external_dirs(config_yaml: &str, library: &Path) -> String {
    let mut dirs = yaml_list(config_yaml, "skills", "external_dirs");
    let library = library.to_string_lossy().into_owned();
    if !dirs.contains(&library) {
        dirs.push(library);
    }
    json!(dirs).to_string()
}

/// Reads `parent: { key: [...] }` from YAML in its block or flow form. Hermes
/// writes this file itself, so these two forms cover it.
fn yaml_list(text: &str, parent: &str, key: &str) -> Vec<String> {
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim_end() != format!("{parent}:"));
    if lines.next().is_none() {
        return Vec::new();
    }
    let mut inside = lines.take_while(|line| line.is_empty() || line.starts_with(' '));
    let Some(line) = inside.find(|line| line.trim_start().starts_with(&format!("{key}:"))) else {
        return Vec::new();
    };
    let rest = line
        .trim_start()
        .trim_start_matches(&format!("{key}:"))
        .trim();
    let unquote = |item: &str| item.trim().trim_matches('"').trim_matches('\'').to_string();
    if let Some(flow) = rest
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return flow
            .split(',')
            .map(unquote)
            .filter(|item| !item.is_empty())
            .collect();
    }
    inside
        .map(str::trim)
        .take_while(|line| line.starts_with("- "))
        .filter_map(|line| line.strip_prefix("- "))
        .map(unquote)
        .collect()
}

/// Grok's copied `config.toml` with the library added to `[skills] paths`.
pub fn grok_config(config_toml: &str, library: &Path) -> Result<String, String> {
    let mut config: toml::Table = config_toml.parse().map_err(|error| format!("{error}"))?;
    let skills = config
        .entry("skills")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let skills = skills
        .as_table_mut()
        .ok_or("[skills] in config.toml is not a table")?;
    let paths = skills
        .entry("paths")
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    let paths = paths
        .as_array_mut()
        .ok_or("skills.paths in config.toml is not a list")?;
    let library = toml::Value::String(library.to_string_lossy().into_owned());
    if !paths.contains(&library) {
        paths.push(library);
    }
    toml::to_string(&config).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = "/Users/me/Library/Application Support/sikemux/skills";

    #[test]
    fn hermes_keeps_external_dirs_in_either_yaml_form() {
        let block = "model: x\nskills:\n  enabled: true\n  external_dirs:\n    - ~/team-skills\n    - \"/opt/skills\"\nmcp_servers: {}\n";
        assert_eq!(
            hermes_external_dirs(block, Path::new(LIBRARY)),
            json!(["~/team-skills", "/opt/skills", LIBRARY]).to_string()
        );
        let flow = "skills:\n  external_dirs: [~/a, '/b']\n";
        assert_eq!(
            hermes_external_dirs(flow, Path::new(LIBRARY)),
            json!(["~/a", "/b", LIBRARY]).to_string()
        );
        assert_eq!(
            hermes_external_dirs("{}\n", Path::new(LIBRARY)),
            json!([LIBRARY]).to_string()
        );
    }

    #[test]
    fn grok_adds_the_library_to_its_skills_paths_and_keeps_the_rest() {
        let config = "model = \"grok-5\"\n\n[skills]\npaths = [\"/own\"]\n\n[mcp_servers.x]\ncommand = \"x\"\n";
        let merged: toml::Table = grok_config(config, Path::new(LIBRARY))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(merged["model"].as_str(), Some("grok-5"));
        assert_eq!(merged["skills"]["paths"].as_array().unwrap().len(), 2);
        assert_eq!(merged["mcp_servers"]["x"]["command"].as_str(), Some("x"));
        let empty: toml::Table = grok_config("", Path::new(LIBRARY))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(empty["skills"]["paths"][0].as_str(), Some(LIBRARY));
        assert!(grok_config("skills = 3", Path::new(LIBRARY)).is_err());
    }
}
