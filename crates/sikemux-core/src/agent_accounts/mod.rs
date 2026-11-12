//! The person's accounts with their agents, which the core keeps so a host
//! with no app still knows them. A profile names an agent, the folder its CLI
//! keeps one sign-in in, and optionally its program and which of the person's
//! variables it gets. Signing in runs the agent's own login; Sikemux never
//! reads or stores a credential.

pub mod allocate;
mod claude_resets;
pub mod defaults;
pub mod folders;
pub mod plan_limits;
pub mod sign_in;
pub mod status;
pub mod usage;

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_pty::user_shell::login_shell_environment;
use tokio::process::Command;

use crate::launch::{account_environment, AgentSpec};
use sikemux_wire::protocol::ChatAccount;

const MAX_PROFILES: usize = 64;
const MAX_TEXT: usize = 4_096;

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
    /// What the account is called: who it is signed in as while a built-in one still has its
    /// agent's own name, else the name the person gave it.
    pub fn shown_name(&self, status: Option<&status::AccountStatus>) -> String {
        let unnamed = self.id.starts_with(BUILT_IN)
            && crate::agent_catalog::agent(&self.provider)
                .is_some_and(|agent| agent.label == self.name);
        status
            .and_then(|status| status.name.clone())
            .filter(|_| unnamed)
            .unwrap_or_else(|| self.name.clone())
    }

    /// The name the account goes by, with who it was last heard signed in as.
    pub fn known_name(&self) -> String {
        let known = status::known(&self.provider, self.config_path.as_deref());
        let account = known
            .as_ref()
            .and_then(|(checked, _)| checked.account.as_ref());
        self.shown_name(account.and_then(|account| account.as_ref().ok()))
    }

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub selected: BTreeMap<String, String>,
    /// Whether each new chat starts on the account whose week ends soonest, and a chat at a
    /// usage limit carries on in the account with the most left. On unless the person turns
    /// it off; a chat never moves to another agent's account.
    #[serde(default = "smart_by_default")]
    pub smart: bool,
    /// The account each agent's new chats go to while smart is on, by provider, as the person
    /// pinned it.
    #[serde(default)]
    pub pinned: BTreeMap<String, String>,
    /// The accounts new chats in a project may start on, by the project's folder, so a work
    /// project never lands on a personal account. A project not listed takes any.
    #[serde(default)]
    pub projects: BTreeMap<String, Vec<String>>,
}

fn smart_by_default() -> bool {
    true
}

impl Default for Book {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            selected: BTreeMap::new(),
            smart: smart_by_default(),
            pinned: BTreeMap::new(),
            projects: BTreeMap::new(),
        }
    }
}

/// What the ids of the accounts each agent starts with begin with.
pub const BUILT_IN: &str = "builtin-";

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
        let oversized =
            oversized || self.selected.iter().any(selected) || self.pinned.iter().any(selected);
        let listed = |(project, ids): (&String, &Vec<String>)| {
            long(project) || ids.len() > MAX_PROFILES || ids.iter().any(|id| long(id))
        };
        let oversized = oversized || self.projects.iter().any(listed);
        let many = [self.selected.len(), self.pinned.len(), self.projects.len()]
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
        self.pinned
            .retain(|provider, id| id != &profile.id || provider == &profile.provider);
        self.profiles.retain(|kept| kept.id != profile.id);
        self.profiles.push(profile);
    }

    /// The book as clients read it: first the account each agent starts with, where the book has
    /// none for it yet, as it keeps only what the person changed.
    pub fn with_built_ins(mut self) -> Self {
        let built_ins: Vec<Profile> = crate::agent_catalog::AGENTS
            .iter()
            .filter(|agent| agent.accounts)
            .filter_map(|agent| {
                let id = format!("{BUILT_IN}{}", agent.kind);
                (!self.profiles.iter().any(|profile| profile.id == id)).then(|| Profile {
                    id,
                    name: agent.label.to_owned(),
                    provider: agent.kind.to_owned(),
                    accent: agent.brand.to_owned(),
                    executable_path: None,
                    config_path: None,
                    environment_keys: Vec::new(),
                })
            })
            .collect();
        self.profiles.splice(0..0, built_ins);
        self
    }

    /// Forgets the account, and that it was chosen. A built-in one stays.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        if id.starts_with(BUILT_IN) {
            return Err("a built-in account cannot be removed".into());
        }
        self.profiles.retain(|profile| profile.id != id);
        self.selected.retain(|_, selected| selected != id);
        self.pinned.retain(|_, pinned| pinned != id);
        for ids in self.projects.values_mut() {
            ids.retain(|kept| kept != id);
        }
        self.projects.retain(|_, ids| !ids.is_empty());
        Ok(())
    }

    /// Whether a new chat working in `cwd` may start on the account `id`: always, unless the
    /// project holding `cwd` lists the accounts it uses.
    pub fn allowed_in(&self, cwd: &std::path::Path, id: &str) -> bool {
        let project = (self.projects.iter())
            .filter(|(folder, _)| cwd.starts_with(folder))
            .max_by_key(|(folder, _)| folder.len());
        project.is_none_or(|(_, ids)| ids.iter().any(|allowed| allowed == id))
    }

    /// The account `provider`'s agents run on when none is named: the one
    /// chosen, else the built-in one.
    pub fn chosen(&self, provider: &str) -> String {
        self.selected
            .get(provider)
            .cloned()
            .unwrap_or_else(|| format!("{BUILT_IN}{provider}"))
    }

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

    /// The account of `provider` whose folder is `folder`, built-in ones included.
    pub fn account_in(&self, provider: &str, folder: Option<&str>) -> Option<Profile> {
        self.clone()
            .with_built_ins()
            .profiles
            .into_iter()
            .filter(|profile| profile.provider == provider)
            .find(|profile| {
                account_environment(provider, profile.config_path.as_deref())
                    .map(|(_, dir)| dir)
                    .as_deref()
                    == folder
            })
    }

    /// The accounts a chat of `provider` on the account `current` may carry on in: those signed
    /// in, and once it has started, only those that keep its chats where `current` does.
    pub fn takers(&self, provider: &str, current: Option<&str>, started: bool) -> Takers {
        let book = self.clone().with_built_ins();
        let own: Vec<&Profile> = (book.profiles.iter())
            .filter(|profile| profile.provider == provider)
            .collect();
        let current = own
            .iter()
            .find(|profile| Some(profile.id.as_str()) == current);
        let named = |profile: &Profile| chat_account(profile);
        let others: Vec<&&Profile> = (own.iter())
            .filter(|other| current.is_none_or(|current| current.id != other.id))
            .collect();
        let signed_in: Vec<&&Profile> = (others.iter().copied())
            .filter(|other| {
                matches!(
                    sign_in::signed_in(provider, &named(other).env),
                    Some(sign_in::SignIn::Account(_))
                )
            })
            .collect();
        let from = current.and_then(|current| current.config_path.as_deref());
        let taking: Vec<ChatAccount> = (signed_in.iter())
            .filter(|other| {
                !started || folders::shares_chats(provider, from, other.config_path.as_deref())
            })
            .map(|other| named(other))
            .collect();
        let agent = crate::agent_catalog::agent(provider).map_or(provider, |agent| agent.label);
        let none_because = taking.is_empty().then(|| match () {
            _ if others.is_empty() => format!("{agent} has no other account here"),
            _ if signed_in.is_empty() => format!("No other {agent} account is signed in"),
            _ => format!("The other {agent} accounts keep their chats apart from this one"),
        });
        Takers {
            current: current.map(|current| named(current)),
            others: taking,
            none_because,
        }
    }
}

/// The account a chat runs on, the others it may carry on in, and why there are none when
/// there are none.
pub struct Takers {
    pub current: Option<ChatAccount>,
    pub others: Vec<ChatAccount>,
    pub none_because: Option<String>,
}

/// The account as a chat names it.
pub fn chat_account(profile: &Profile) -> ChatAccount {
    ChatAccount {
        id: profile.id.clone(),
        label: profile.known_name(),
        env: account_environment(&profile.provider, profile.config_path.as_deref())
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

/// What an agent that keeps no accounts of its own answers when asked for one.
pub const NO_ACCOUNTS: &str = "Only Claude and Codex keep separate accounts";

/// The core's calls about accounts and their limits, all of them the app's own.
pub const METHODS: [&str; 18] = [
    "agentAccounts",
    "setAgentAccounts",
    "saveAgentAccount",
    "removeAgentAccount",
    "selectAgentAccount",
    "setSmartAccounts",
    "unpinAgentAccount",
    "setAccountInProject",
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
