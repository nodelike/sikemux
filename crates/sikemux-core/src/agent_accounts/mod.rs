//! The person's accounts with their agents, which the core keeps so a host
//! with no app still knows them. A profile names an agent, the folder its CLI
//! keeps one sign-in in, and optionally its program and which of the person's
//! variables it gets. Signing in runs the agent's own login; Sikemux never
//! reads or stores a credential.

pub mod defaults;
pub mod folders;
pub mod sign_in;
pub mod status;
pub mod usage;

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_pty::user_shell::login_shell_environment;
use tokio::process::Command;

use crate::launch::{account_environment, AgentSpec};
use crate::protocol::ChatAccount;

const MAX_PROFILES: usize = 64;
const MAX_TEXT: usize = 4_096;

/// One account with one agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub accent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    /// The account folder, or a config file in it; the agent's default when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_keys: Vec<String>,
}

impl Profile {
    pub fn spec(&self) -> AgentSpec {
        AgentSpec {
            provider: self.provider.clone(),
            executable_path: self.executable_path.clone(),
            config_path: self.config_path.clone(),
            environment_keys: self.environment_keys.clone(),
        }
    }
}

/// Every profile, and which one each agent starts on, by provider.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub selected: BTreeMap<String, String>,
    /// Whether a chat moves to the next signed-in account at a usage limit,
    /// by provider. Off unless the person turns it on.
    #[serde(default)]
    pub auto_switch: BTreeMap<String, bool>,
}

/// What the ids of the accounts each agent starts with begin with.
const BUILT_IN: &str = "builtin-";

impl Book {
    pub fn check(&self) -> Result<(), String> {
        let long = |text: &str| text.len() > MAX_TEXT;
        let oversized = self.profiles.iter().any(|profile| {
            [
                &profile.id,
                &profile.name,
                &profile.provider,
                &profile.accent,
            ]
            .into_iter()
            .chain(profile.executable_path.as_ref())
            .chain(profile.config_path.as_ref())
            .chain(&profile.environment_keys)
            .any(|text| long(text))
                || profile.environment_keys.len() > 64
        });
        let selected = |(provider, id): (&String, &String)| long(provider) || long(id);
        let oversized = oversized
            || self.selected.iter().any(selected)
            || self.auto_switch.keys().any(|provider| long(provider));
        let many = [self.selected.len(), self.auto_switch.len()]
            .into_iter()
            .any(|len| len > MAX_PROFILES);
        if self.profiles.len() > MAX_PROFILES || oversized || many {
            return Err("the app sent more agent accounts than the core keeps".into());
        }
        Ok(())
    }

    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// Keeps `profile`, in place of the one with its id. A selection of it
    /// for another provider goes.
    pub fn save(&mut self, profile: Profile) {
        self.selected
            .retain(|provider, id| id != &profile.id || provider == &profile.provider);
        self.profiles.retain(|kept| kept.id != profile.id);
        self.profiles.push(profile);
    }

    /// Forgets the account, and that it was chosen. A built-in one stays.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        if id.starts_with(BUILT_IN) {
            return Err("a built-in account cannot be removed".into());
        }
        self.profiles.retain(|profile| profile.id != id);
        self.selected.retain(|_, selected| selected != id);
        Ok(())
    }

    /// The account `provider`'s agents run on when none is named: the one
    /// chosen, else the built-in one.
    pub fn chosen(&self, provider: &str) -> String {
        self.selected
            .get(provider)
            .cloned()
            .unwrap_or_else(|| format!("{BUILT_IN}{provider}"))
    }

    /// The agent `provider` on the account in `config_path`.
    pub fn spec(&self, provider: &str, config_path: Option<&str>) -> AgentSpec {
        self.profiles
            .iter()
            .find(|profile| {
                profile.provider == provider && profile.config_path.as_deref() == config_path
            })
            .map(Profile::spec)
            .unwrap_or_else(|| AgentSpec {
                config_path: config_path.map(str::to_owned),
                ..AgentSpec::new(provider)
            })
    }

    /// The account a chat of `provider` runs on when its folder is `folder`,
    /// and the others that keep chats where it does, which it may move to.
    pub fn chat_accounts(
        &self,
        provider: &str,
        folder: Option<&str>,
    ) -> (Option<ChatAccount>, Vec<ChatAccount>) {
        let folder_of = |profile: &Profile| {
            account_environment(provider, profile.config_path.as_deref()).map(|(_, dir)| dir)
        };
        let own: Vec<&Profile> = self
            .profiles
            .iter()
            .filter(|profile| profile.provider == provider)
            .collect();
        let Some(current) = own
            .iter()
            .find(|profile| folder_of(profile).as_deref() == folder)
        else {
            return (None, Vec::new());
        };
        let config_path = current.config_path.as_deref();
        let others = own
            .iter()
            .filter(|other| {
                other.id != current.id
                    && folders::sessions_shared(provider, config_path, other.config_path.as_deref())
            })
            .map(|other| {
                chat_account(
                    provider,
                    &other.id,
                    &other.name,
                    other.config_path.as_deref(),
                )
            })
            .collect();
        let current = chat_account(provider, &current.id, &current.name, config_path);
        (Some(current), others)
    }
}

/// The account in `config_path` as a chat names it, with its folder sharing
/// the default one's chats.
pub fn chat_account(
    provider: &str,
    id: &str,
    label: &str,
    config_path: Option<&str>,
) -> ChatAccount {
    folders::link_shared(provider, config_path);
    ChatAccount {
        id: id.to_owned(),
        label: label.to_owned(),
        env: account_environment(provider, config_path)
            .into_iter()
            .collect(),
    }
}

/// One account, as a call names it.
fn key(provider: &str, config_path: Option<&str>) -> String {
    format!("{provider}\0{}", config_path.unwrap_or(""))
}

/// `program` run as `provider` on the account in `config_path`, with the
/// person's shell variables, for a one-off question to its CLI.
pub fn cli(
    provider: &str,
    program: &Path,
    config_path: Option<&str>,
    providers: BTreeMap<String, String>,
) -> Command {
    let spec = AgentSpec {
        config_path: config_path.map(str::to_owned),
        ..AgentSpec::new(provider)
    };
    let mut command = Command::from(sikemux_process::user_environment::agent_command(program));
    command
        .envs(login_shell_environment())
        .envs(spec.launch(program.to_path_buf(), providers).env)
        .kill_on_drop(true);
    command
}

/// The core's calls about accounts and their limits, all of them the app's own.
pub const METHODS: [&str; 16] = [
    "agentAccounts",
    "setAgentAccounts",
    "saveAgentAccount",
    "removeAgentAccount",
    "selectAgentAccount",
    "setAccountAutoSwitch",
    "addAgentAccount",
    "agentStatus",
    "agentAccountStatus",
    "agentUsage",
    "forgetAgentStatus",
    "markAgentSignedOut",
    "cancelResume",
    "retryFailed",
    "discardFailed",
    "authenticateChat",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_moved_to_another_agent_is_no_longer_that_agents_choice() {
        let local = |provider: &str| Profile {
            id: "local".into(),
            name: "Local".into(),
            provider: provider.into(),
            accent: "#abcdef".into(),
            executable_path: None,
            config_path: None,
            environment_keys: Vec::new(),
        };
        let mut book = Book {
            profiles: vec![local("claude")],
            selected: BTreeMap::from([("claude".to_owned(), "local".to_owned())]),
            ..Book::default()
        };
        book.save(local("codex"));
        assert!(!book.selected.contains_key("claude"));
        assert_eq!(book.profiles.len(), 1);
        assert_eq!(
            book.profile("local")
                .map(|profile| profile.provider.as_str()),
            Some("codex")
        );
    }
}
