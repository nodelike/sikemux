//! How Sikemux starts an agent: its program, arguments, environment and
//! account folder. Every launch, in a terminal or a chat, by the app or by the
//! core, is built here.

pub mod discovery;
pub mod homes;
pub mod model_feed;
pub mod providers;
pub mod skills;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sikemux_pty::launch::{account_dir, account_variable};
use sikemux_pty::user_shell::user_variable;

/// One agent, as the person set it up: which program, which account and which
/// of their shell's variables it gets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    /// The account folder, or a config file in it; the agent's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_keys: Vec<String>,
}

/// What a launch runs, on top of the launching process's own environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub account_dir: Option<PathBuf>,
}

impl AgentSpec {
    pub fn new(provider: &str) -> Self {
        Self {
            provider: provider.to_owned(),
            ..Self::default()
        }
    }

    /// The program this agent runs as.
    pub async fn program(&self) -> Result<PathBuf, String> {
        discovery::resolve(&self.provider, self.executable_path.as_deref()).await
    }

    /// `program` run as this agent: the variables its profile names, with the
    /// values the person's shell gives them, its account folder, the hosted
    /// model providers' keys in `providers`, and the person's PATH, which
    /// holds the system folders Claude reads its sign-in with.
    pub fn launch(&self, program: PathBuf, providers: BTreeMap<String, String>) -> AgentLaunch {
        let mut env = forwarded_environment(&self.environment_keys);
        let account = account_environment(&self.provider, self.config_path.as_deref());
        let account_dir = account.as_ref().map(|(_, dir)| PathBuf::from(dir));
        env.extend(account);
        env.extend(providers);
        if let Some(path) = sikemux_process::user_environment::var("PATH") {
            env.insert("PATH".into(), path);
        }
        AgentLaunch {
            program,
            args: Vec::new(),
            env,
            account_dir,
        }
    }

    /// Like [`AgentSpec::launch`], for a chat: the agent in its ACP mode, on
    /// the provider `providers` wires it to, given the skills library in
    /// `skills`, a data folder [`skills::ready`] made.
    pub fn chat(
        &self,
        program: PathBuf,
        providers: providers::Wiring,
        skills: Option<&Path>,
    ) -> AgentLaunch {
        let mut launch = self.launch(program, providers.env);
        launch.args = crate::acp::native::arguments(&self.provider)
            .unwrap_or_default()
            .iter()
            .map(|argument| (*argument).to_owned())
            .chain(providers.args)
            .collect();
        if let Some(data_dir) = skills {
            skills::chat(data_dir, &self.provider, &mut launch.args, &mut launch.env);
        }
        launch
    }
}

/// The variable naming the account folder an agent keeps its sign-in and
/// sessions in: the one `config_path` names, or the one the person's shell
/// points it at.
pub fn account_environment(provider: &str, config_path: Option<&str>) -> Option<(String, String)> {
    let variable = account_variable(provider)?;
    let directory = match config_path.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => account_dir(path).to_string_lossy().into_owned(),
        None => user_variable(variable)?,
    };
    Some((variable.to_owned(), directory))
}

fn forwarded_environment(environment_keys: &[String]) -> BTreeMap<String, String> {
    let mut seen = HashSet::new();
    environment_keys
        .iter()
        .take(64)
        .filter(|key| seen.insert(*key) && valid_environment_key(key))
        .filter_map(|key| Some((key.clone(), user_variable(key)?)))
        .collect()
}

fn valid_environment_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z' | 'a'..='z'))
        && chars.all(|ch| matches!(ch, '_' | 'A'..='Z' | 'a'..='z' | '0'..='9'))
        && key.len() <= 128
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_names_the_agents_account_directory() {
        for (provider, key) in [("codex", "CODEX_HOME"), ("claude", "CLAUDE_CONFIG_DIR")] {
            let mut spec = AgentSpec::new(provider);
            spec.config_path = Some("/profile".into());
            let launch = spec.launch(PathBuf::from("/bin/agent"), BTreeMap::new());
            assert_eq!(launch.env.get(key).map(String::as_str), Some("/profile"));
            assert_eq!(launch.account_dir.as_deref(), Some(Path::new("/profile")));
        }
        assert_eq!(account_environment("opencode", Some("/profile")), None);
    }

    #[test]
    fn native_agents_run_their_own_binary_in_acp_mode() {
        let launch =
            AgentSpec::new("grok").chat(PathBuf::from("/bin/grok"), Default::default(), None);
        assert_eq!(launch.program, Path::new("/bin/grok"));
        assert_eq!(launch.args, ["agent", "--no-leader", "stdio"]);
    }

    #[test]
    fn only_named_valid_variables_are_forwarded() {
        let env = forwarded_environment(&["PATH".into(), "PATH".into(), "BAD-KEY".into()]);
        assert_eq!(env.keys().collect::<Vec<_>>(), ["PATH"]);
    }

    #[test]
    fn config_files_name_their_directory() {
        assert_eq!(
            account_dir("/home/me/.codex/config.toml"),
            Path::new("/home/me/.codex")
        );
        assert_eq!(
            account_dir("/home/me/.claude"),
            Path::new("/home/me/.claude")
        );
    }
}
