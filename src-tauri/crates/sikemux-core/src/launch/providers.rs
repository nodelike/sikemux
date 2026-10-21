//! Where agents get models besides their makers' own accounts: pay-per-token
//! APIs, coding plans and runners on this computer. The catalogue in
//! `model-providers.json` holds how each provider is reached, and models.dev
//! what its models are; the keys the person connected stay in the Keychain
//! and reach agents as they start.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::model_feed::{Feed, Snapshot};
use crate::agent_catalog::{self, ProviderUse, AGENTS};

/// A development build keeps entries of its own, so it never spends or replaces the
/// installed app's sign-in.
#[cfg(not(test))]
pub const KEY_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-model-provider-dev"
} else {
    "sikemux-model-provider"
};
/// Tests keep to an entry of their own, so they never replace or delete a real key.
#[cfg(test)]
pub const KEY_SERVICE: &str = "sikemux-model-provider-test";

/// Names the provider a chat runs on in place of its agent's own account.
pub const ROUTED_ENV: &str = "SIKEMUX_MODEL_PROVIDER";

/// The agents Sikemux can point at a provider in place of their makers' accounts.
pub fn routed_agents() -> impl Iterator<Item = &'static str> {
    AGENTS
        .iter()
        .filter(|agent| matches!(agent.providers, Some(ProviderUse::Routed)))
        .map(|agent| agent.kind)
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Api,
    Plan,
    Local,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub kind: Kind,
    pub sign_up_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<KeySpec>,
    /// Base URL of its OpenAI-compatible API.
    pub openai: String,
    /// That API also serves OpenAI's Responses API, the only one Codex speaks.
    #[serde(default, skip_serializing)]
    pub responses: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<AnthropicApi>,
    /// The provider's id on models.dev.
    #[serde(skip_serializing)]
    pub models_dev: Option<String>,
    /// The coding model the provider's own docs recommend, as its API names it.
    #[serde(skip_serializing)]
    pub model: Option<String>,
    /// The model an agent's own guide from the provider names, where it differs.
    #[serde(default, skip_serializing)]
    pub agent_models: BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeySpec {
    /// Every name an agent looks for this key under.
    pub env: Vec<String>,
    /// Answers 401 or 403 to a key the provider does not know.
    pub check: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnthropicApi {
    pub url: String,
    pub auth: AnthropicAuth,
}

/// How the Anthropic-compatible API wants the key: Claude Code sends
/// `ANTHROPIC_AUTH_TOKEN` as a bearer token and `ANTHROPIC_API_KEY` as `x-api-key`.
#[derive(Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum AnthropicAuth {
    Bearer,
    XApiKey,
}

static CATALOGUE: LazyLock<Vec<Provider>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../model-providers.json"))
        .expect("model-providers.json is a valid catalogue")
});

pub fn catalogue() -> &'static [Provider] {
    &CATALOGUE
}

pub fn find(id: &str) -> Option<&'static Provider> {
    CATALOGUE.iter().find(|provider| provider.id == id)
}

/// Every variable a launch can carry a provider's key in.
pub fn key_names() -> BTreeSet<&'static str> {
    catalogue()
        .iter()
        .filter_map(|provider| provider.key.as_ref())
        .flat_map(|key| key.env.iter().map(String::as_str))
        .chain(["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"])
        .collect()
}

fn is_keyed(id: &str) -> bool {
    find(id).is_some_and(|provider| provider.key.is_some())
}

/// What a launch adds to the agent's own: variables and arguments.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Wiring {
    pub env: BTreeMap<String, String>,
    pub args: Vec<String>,
}

/// What helpers run on: workers started without a model named, and Claude
/// Code's own subagents when the chat runs where the helpers do.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Helper {
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Helper {
    pub fn check(&self) -> Result<(), String> {
        if self.agent.is_empty() || self.agent.len() > 64 {
            return Err("Pick the agent helpers run as".into());
        }
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty() || model.len() > 200)
        {
            return Err("A helper's model is a name of at most 200 characters".into());
        }
        match self.provider.as_deref().map(|id| (id, find(id))) {
            None => Ok(()),
            Some((id, None)) => Err(format!("Sikemux does not know a provider called {id}")),
            Some((_, Some(provider))) if !provider.routes(&self.agent) => {
                Err(format!("{} cannot run on {}", self.agent, provider.label))
            }
            Some(_) => Ok(()),
        }
    }

    /// The model Claude Code's subagents take in a chat of `agent` on `provider`.
    fn subagent_model(&self, agent: &str, provider: Option<&str>) -> Option<&str> {
        (agent == "claude" && self.agent == agent && self.provider.as_deref() == provider)
            .then_some(self.model.as_deref())
            .flatten()
    }
}

impl Provider {
    /// The model a chat of `agent` starts on here when none is picked: the
    /// recommended one while models.dev lists it and the provider serves it,
    /// else the newest that reasons and calls tools, else the first served.
    /// `served` is what the provider listed, when it was asked. Claude Code
    /// is told of a context of a million tokens by the `[1m]` suffix.
    pub fn coding_model(
        &self,
        agent: &str,
        feed: &Snapshot,
        served: Option<&[String]>,
    ) -> Option<String> {
        let serves = |id: &str| served.is_none_or(|served| served.iter().any(|model| model == id));
        let listed = self.models_dev.as_deref().and_then(|id| feed.models(id));
        let recommended = self
            .agent_models
            .get(agent)
            .or(self.model.as_ref())
            .filter(|model| {
                listed.is_some_and(|listed| listed.contains_key(*model)) && serves(model)
            });
        let newest = || {
            listed?
                .iter()
                .filter(|(id, model)| {
                    model.tool_call && model.reasoning && model.status.is_none() && serves(id)
                })
                .max_by(|(a_id, a), (b_id, b)| {
                    (&a.release_date, a_id).cmp(&(&b.release_date, b_id))
                })
                .map(|(id, _)| id)
        };
        let model = recommended.or_else(newest).or_else(|| served?.first())?;
        let long = agent == "claude"
            && listed
                .and_then(|listed| listed.get(model))
                .is_some_and(|facts| facts.limit.context >= 1_000_000);
        Some(if long {
            format!("{model}[1m]")
        } else {
            model.clone()
        })
    }

    pub fn routes(&self, agent: &str) -> bool {
        match agent {
            "claude" => self.anthropic.is_some(),
            "codex" => self.kind == Kind::Local || self.responses,
            _ => false,
        }
    }

    /// `agent` pointed at this provider, paying with `key` and running `model`.
    /// Local runners take any token; Ollama's and LM Studio's guides use their names.
    fn wiring(&self, agent: &str, key: Option<&str>, model: Option<&str>) -> Option<Wiring> {
        if !self.routes(agent) {
            return None;
        }
        let mut env = BTreeMap::from([(ROUTED_ENV.to_owned(), self.id.clone())]);
        let token = key.unwrap_or(&self.id).to_owned();
        if agent == "claude" {
            let api = self.anthropic.as_ref()?;
            let (used, blanked) = match api.auth {
                AnthropicAuth::Bearer => ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"),
                AnthropicAuth::XApiKey => ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"),
            };
            env.insert("ANTHROPIC_BASE_URL".into(), api.url.clone());
            env.insert(used.into(), token);
            env.insert(blanked.into(), String::new());
            if let Some(model) = model {
                for tier in ["FABLE", "OPUS", "SONNET", "HAIKU"] {
                    env.insert(format!("ANTHROPIC_DEFAULT_{tier}_MODEL"), model.to_owned());
                }
            }
            return Some(Wiring {
                env,
                args: Vec::new(),
            });
        }
        let mut args = Vec::new();
        let mut config = |key: &str, value: &str| {
            args.push("-c".to_owned());
            args.push(format!("{key}={}", serde_json::Value::from(value)));
        };
        if let Some(spec) = &self.key {
            let name = spec.env.first()?;
            let table = format!("model_providers.{}", self.id);
            config(&format!("{table}.name"), &self.label);
            config(&format!("{table}.base_url"), &self.openai);
            config(&format!("{table}.env_key"), name);
            env.insert(name.clone(), token);
        }
        config("model_provider", &self.id);
        if let Some(model) = model {
            config("model", model);
        }
        Some(Wiring { env, args })
    }
}

/// Which providers have a key saved, kept in a file the app and the core both
/// read, and the keys this process has read, so a launch does not start the
/// Keychain tool for every provider every time.
pub struct Providers {
    path: PathBuf,
    helper_path: PathBuf,
    state: Mutex<State>,
    feed: Feed,
}

#[derive(Default)]
struct State {
    /// When the list was last written, so a change another process made is noticed.
    stamp: Option<SystemTime>,
    connected: BTreeSet<String>,
    keys: HashMap<String, String>,
}

impl Providers {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("model-providers.json"),
            helper_path: data_dir.join("model-helper.json"),
            state: Mutex::default(),
            feed: Feed::new(data_dir),
        }
    }

    /// What models.dev says of every provider's models. Reads the disk, so it blocks.
    pub fn feed(&self) -> Arc<Snapshot> {
        self.feed.current()
    }

    /// The model a chat of `agent` starts on at the provider `id` when none
    /// is picked. Reads the disk, so it blocks.
    pub fn coding_model(&self, id: &str, agent: &str) -> Option<String> {
        find(id)?.coding_model(agent, &self.feed(), None)
    }

    fn fresh(&self) -> MutexGuard<'_, State> {
        let mut state = self.state.locked();
        let stamp = std::fs::metadata(&self.path)
            .and_then(|metadata| metadata.modified())
            .ok();
        if stamp != state.stamp || stamp.is_none() {
            state.connected = std::fs::read(&self.path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<BTreeSet<String>>(&bytes).ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|id| is_keyed(id))
                .collect();
            state.keys.clear();
            state.stamp = stamp;
        }
        state
    }

    pub fn connected(&self) -> BTreeSet<String> {
        self.fresh().connected.clone()
    }

    /// Written even when unchanged, so a process holding a replaced key reads it again.
    pub fn set_connected(&self, id: &str, connected: bool) -> Result<(), String> {
        let mut state = self.fresh();
        if connected {
            state.connected.insert(id.to_owned());
        } else {
            state.connected.remove(id);
        }
        let bytes = serde_json::to_vec(&state.connected).map_err(|error| error.to_string())?;
        sikemux_process::files::write_atomic(&self.path, &bytes, false)
            .map_err(|error| error.to_string())?;
        state.stamp = std::fs::metadata(&self.path)
            .and_then(|metadata| metadata.modified())
            .ok();
        Ok(())
    }

    /// Read afresh each time: it is small, and asked for only as agents start.
    pub fn helper(&self) -> Option<Helper> {
        let bytes = std::fs::read(&self.helper_path).ok()?;
        serde_json::from_slice::<Helper>(&bytes)
            .ok()
            .filter(|helper| helper.check().is_ok())
    }

    pub fn set_helper(&self, helper: Option<&Helper>) -> Result<(), String> {
        let Some(helper) = helper else {
            return match std::fs::remove_file(&self.helper_path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(error.to_string())
                }
                _ => Ok(()),
            };
        };
        helper.check()?;
        let bytes = serde_json::to_vec(helper).map_err(|error| error.to_string())?;
        sikemux_process::files::write_atomic(&self.helper_path, &bytes, false)
            .map_err(|error| error.to_string())
    }

    pub fn remember_key(&self, id: &str, key: Option<String>) {
        let mut state = self.fresh();
        match key {
            Some(key) => state.keys.insert(id.to_owned(), key),
            None => state.keys.remove(id),
        };
    }

    /// The key of a connected provider. Reads the Keychain, so it blocks.
    pub fn key(&self, id: &str) -> Option<String> {
        {
            let state = self.fresh();
            if !state.connected.contains(id) {
                return None;
            }
            if let Some(key) = state.keys.get(id) {
                return Some(key.clone());
            }
        }
        let key = match sikemux_keychain::read(KEY_SERVICE, id) {
            Ok(key) => key?,
            Err(error) => {
                eprintln!("Sikemux could not read the {id} key: {error}");
                return None;
            }
        };
        self.remember_key(id, Some(key.clone()));
        Some(key)
    }

    /// The keys `agent` starts with, under the names it looks for. Reads the
    /// Keychain, so it blocks.
    pub fn environment(&self, agent: &str) -> BTreeMap<String, String> {
        let reads = agent_catalog::agent(agent)
            .is_some_and(|agent| matches!(agent.providers, Some(ProviderUse::Reads)));
        if !reads {
            return BTreeMap::new();
        }
        self.connected()
            .iter()
            .filter_map(|id| Some((&find(id)?.key.as_ref()?.env, self.key(id)?)))
            .flat_map(|(names, key)| names.iter().map(move |name| (name.clone(), key.clone())))
            .collect()
    }

    /// What `agent` starts with: the keys it reads and, when `provider` is
    /// named, what points it there in place of its maker's account. Fails
    /// rather than let that account pay for a turn meant for the provider.
    /// `model` is the one picked, or the provider's [`Providers::coding_model`].
    /// Reads the Keychain, so it blocks.
    pub fn wiring(
        &self,
        agent: &str,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<Wiring, String> {
        let mut env = self.environment(agent);
        if let Some(model) = self
            .helper()
            .as_ref()
            .and_then(|helper| helper.subagent_model(agent, provider))
        {
            env.insert("CLAUDE_CODE_SUBAGENT_MODEL".into(), model.to_owned());
        }
        let Some(id) = provider else {
            return Ok(Wiring {
                env,
                args: Vec::new(),
            });
        };
        let provider =
            find(id).ok_or_else(|| format!("Sikemux does not know a provider called {id}"))?;
        let key = match provider.key {
            Some(_) => Some(self.key(id).ok_or_else(|| {
                format!("Connect {} in Settings to run chats on it", provider.label)
            })?),
            None => None,
        };
        let mut wiring = provider
            .wiring(agent, key.as_deref(), model)
            .ok_or_else(|| format!("{agent} cannot run on {}", provider.label))?;
        for (name, value) in env {
            wiring.env.entry(name).or_insert(value);
        }
        Ok(wiring)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_connected_list_survives_a_restart_and_drops_unknown_providers() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("model-providers.json");
        std::fs::write(&path, r#"["baseten","retired-provider"]"#).expect("write list");
        let store = Providers::new(directory.path());
        assert_eq!(store.connected(), BTreeSet::from(["baseten".to_string()]));
        store.set_connected("groq", true).expect("connect");
        assert_eq!(
            Providers::new(directory.path()).connected(),
            BTreeSet::from(["baseten".to_string(), "groq".to_string()])
        );
        store.set_connected("baseten", false).expect("disconnect");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read list"),
            r#"["groq"]"#
        );
    }

    #[test]
    fn every_provider_has_its_own_id_and_key_names() {
        let ids: BTreeSet<_> = catalogue().iter().map(|provider| &provider.id).collect();
        assert_eq!(ids.len(), catalogue().len());
        let envs: Vec<_> = catalogue()
            .iter()
            .filter_map(|provider| provider.key.as_ref())
            .flat_map(|spec| &spec.env)
            .collect();
        assert_eq!(envs.iter().collect::<BTreeSet<_>>().len(), envs.len());
        for provider in catalogue() {
            assert_eq!(
                provider.key.is_none(),
                provider.kind == Kind::Local,
                "{}",
                provider.id
            );
            assert!(
                provider.sign_up_url.starts_with("https://"),
                "{}",
                provider.id
            );
            if let Some(spec) = &provider.key {
                assert!(spec.check.starts_with("https://"), "{}", provider.id);
                assert!(!spec.env.is_empty(), "{}", provider.id);
            }
        }
    }
}
