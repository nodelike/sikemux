//! Hosted model providers a person pays for directly, such as Baseten or
//! OpenRouter. OpenCode, Pi and OMP already know each one and turn it on
//! when its key is in their environment, so connecting a provider is saving
//! its key in the Keychain and handing it to those agents as they start.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::Serialize;

#[cfg(not(test))]
const KEY_SERVICE: &str = "sikemux-model-provider";
/// Tests keep to an entry of their own, so they never replace or delete a real key.
#[cfg(test)]
const KEY_SERVICE: &str = "sikemux-model-provider-test";
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// The agents that read these providers' keys from their environment.
const READERS: &[&str] = &["opencode", "pi", "omp"];

#[derive(Debug, PartialEq, Eq)]
struct Preset {
    id: &'static str,
    label: &'static str,
    env: &'static str,
    /// Answers 401 or 403 to a key the provider does not know.
    check_url: &'static str,
    keys_url: &'static str,
}

const PRESETS: &[Preset] = &[
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        env: "OPENROUTER_API_KEY",
        check_url: "https://openrouter.ai/api/v1/key",
        keys_url: "https://openrouter.ai/settings/keys",
    },
    Preset {
        id: "baseten",
        label: "Baseten",
        env: "BASETEN_API_KEY",
        check_url: "https://inference.baseten.co/v1/models",
        keys_url: "https://app.baseten.co/settings/api_keys",
    },
    Preset {
        id: "together",
        label: "Together AI",
        env: "TOGETHER_API_KEY",
        check_url: "https://api.together.xyz/v1/models",
        keys_url: "https://api.together.ai/settings/api-keys",
    },
    Preset {
        id: "fireworks",
        label: "Fireworks",
        env: "FIREWORKS_API_KEY",
        check_url: "https://api.fireworks.ai/inference/v1/models",
        keys_url: "https://fireworks.ai/account/api-keys",
    },
    Preset {
        id: "groq",
        label: "Groq",
        env: "GROQ_API_KEY",
        check_url: "https://api.groq.com/openai/v1/models",
        keys_url: "https://console.groq.com/keys",
    },
    Preset {
        id: "cerebras",
        label: "Cerebras",
        env: "CEREBRAS_API_KEY",
        check_url: "https://api.cerebras.ai/v1/models",
        keys_url: "https://cloud.cerebras.ai",
    },
    Preset {
        id: "deepseek",
        label: "DeepSeek",
        env: "DEEPSEEK_API_KEY",
        check_url: "https://api.deepseek.com/models",
        keys_url: "https://platform.deepseek.com/api_keys",
    },
    Preset {
        id: "moonshot",
        label: "Moonshot AI",
        env: "MOONSHOT_API_KEY",
        check_url: "https://api.moonshot.ai/v1/models",
        keys_url: "https://platform.moonshot.ai/console/api-keys",
    },
    Preset {
        id: "mistral",
        label: "Mistral",
        env: "MISTRAL_API_KEY",
        check_url: "https://api.mistral.ai/v1/models",
        keys_url: "https://console.mistral.ai/api-keys",
    },
    Preset {
        id: "vercel",
        label: "Vercel AI Gateway",
        env: "AI_GATEWAY_API_KEY",
        check_url: "https://ai-gateway.vercel.sh/v1/models",
        keys_url: "https://vercel.com/dashboard/ai-gateway/api-keys",
    },
];

fn preset(id: &str) -> Result<&'static Preset, String> {
    PRESETS
        .iter()
        .find(|preset| preset.id == id)
        .ok_or_else(|| format!("Sikemux does not know a provider called {id}"))
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelProvider {
    id: &'static str,
    label: &'static str,
    keys_url: &'static str,
    connected: bool,
}

/// Which providers have a key saved, and the keys themselves once read, so
/// a launch does not start the Keychain tool for every provider every time.
struct Store {
    path: PathBuf,
    connected: Mutex<BTreeSet<String>>,
    keys: Mutex<HashMap<String, String>>,
}

static STORE: OnceLock<Store> = OnceLock::new();

pub fn init(path: PathBuf) {
    let _ = STORE.set(Store::load(path));
}

fn store() -> Result<&'static Store, String> {
    STORE
        .get()
        .ok_or_else(|| "Model providers are not ready yet".to_string())
}

impl Store {
    fn load(path: PathBuf) -> Self {
        let connected = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeSet<String>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|id| preset(id).is_ok())
            .collect();
        Self {
            path,
            connected: Mutex::new(connected),
            keys: Mutex::new(HashMap::new()),
        }
    }

    fn connected(&self) -> BTreeSet<String> {
        self.connected
            .lock()
            .map(|connected| connected.clone())
            .unwrap_or_default()
    }

    fn set_connected(&self, id: &str, connected: bool) -> Result<(), String> {
        let mut ids = self
            .connected
            .lock()
            .map_err(|_| "Model providers are unavailable".to_string())?;
        let changed = if connected {
            ids.insert(id.to_owned())
        } else {
            ids.remove(id)
        };
        if !changed {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = serde_json::to_vec(&*ids).map_err(|error| error.to_string())?;
        std::fs::write(&self.path, bytes).map_err(|error| error.to_string())
    }

    fn remember_key(&self, id: &str, key: Option<String>) {
        if let Ok(mut keys) = self.keys.lock() {
            match key {
                Some(key) => keys.insert(id.to_owned(), key),
                None => keys.remove(id),
            };
        }
    }

    fn key(&self, id: &str) -> Option<String> {
        if let Some(key) = self.keys.lock().ok()?.get(id).cloned() {
            return Some(key);
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
}

fn reads_provider_keys(agent: &str) -> bool {
    READERS.contains(&agent)
}

/// The keys `agent` should start with, under the names it looks for.
pub async fn environment(agent: &str) -> BTreeMap<String, String> {
    if !reads_provider_keys(agent) {
        return BTreeMap::new();
    }
    let Ok(store) = store() else {
        return BTreeMap::new();
    };
    let connected = store.connected();
    if connected.is_empty() {
        return BTreeMap::new();
    }
    tokio::task::spawn_blocking(move || {
        connected
            .iter()
            .filter_map(|id| {
                let preset = preset(id).ok()?;
                Some((preset.env.to_owned(), store.key(id)?))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(CHECK_TIMEOUT)
            .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

fn check_failure(label: &str, status: StatusCode) -> Option<String> {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Some(format!("{label} did not accept that key"))
        }
        status if status.is_success() => None,
        status => Some(format!(
            "{label} could not check the key right now ({status})"
        )),
    }
}

async fn check_key(preset: &Preset, key: &str) -> Result<(), String> {
    let response = client()
        .get(preset.check_url)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| format!("Could not reach {} to check the key", preset.label))?;
    match check_failure(preset.label, response.status()) {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}

#[tauri::command]
pub fn model_providers() -> Vec<ModelProvider> {
    let connected = store().map(Store::connected).unwrap_or_default();
    PRESETS
        .iter()
        .map(|preset| ModelProvider {
            id: preset.id,
            label: preset.label,
            keys_url: preset.keys_url,
            connected: connected.contains(preset.id),
        })
        .collect()
}

/// Saves the key once the provider accepts it. Agents already running keep
/// the environment they started with.
#[tauri::command]
pub async fn model_provider_connect(id: String, key: String) -> Result<(), String> {
    let preset = preset(&id)?;
    let store = store()?;
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err("Paste the key first".into());
    }
    check_key(preset, &key).await?;
    let saved = key.clone();
    tokio::task::spawn_blocking(move || sikemux_keychain::write(KEY_SERVICE, preset.id, &saved))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    store.remember_key(preset.id, Some(key));
    store.set_connected(preset.id, true)
}

#[tauri::command]
pub async fn model_provider_disconnect(id: String) -> Result<(), String> {
    let preset = preset(&id)?;
    let store = store()?;
    tokio::task::spawn_blocking(move || sikemux_keychain::delete(KEY_SERVICE, preset.id))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    store.remember_key(preset.id, None);
    store.set_connected(preset.id, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_has_its_own_id_and_key_name() {
        let ids: BTreeSet<_> = PRESETS.iter().map(|preset| preset.id).collect();
        let envs: BTreeSet<_> = PRESETS.iter().map(|preset| preset.env).collect();
        assert_eq!(ids.len(), PRESETS.len());
        assert_eq!(envs.len(), PRESETS.len());
        for preset in PRESETS {
            assert!(preset.check_url.starts_with("https://"), "{}", preset.id);
            assert!(preset.keys_url.starts_with("https://"), "{}", preset.id);
        }
    }

    #[test]
    fn only_agents_that_read_provider_keys_are_given_them() {
        for agent in ["opencode", "pi", "omp"] {
            assert!(reads_provider_keys(agent), "{agent}");
        }
        for agent in ["claude", "codex", "hermes", "grok"] {
            assert!(!reads_provider_keys(agent), "{agent}");
        }
    }

    #[test]
    fn a_rejected_key_is_told_apart_from_a_provider_that_did_not_answer() {
        assert_eq!(check_failure("Baseten", StatusCode::OK), None);
        assert_eq!(
            check_failure("Baseten", StatusCode::FORBIDDEN).as_deref(),
            Some("Baseten did not accept that key")
        );
        assert_eq!(
            check_failure("Groq", StatusCode::UNAUTHORIZED).as_deref(),
            Some("Groq did not accept that key")
        );
        assert!(check_failure("Groq", StatusCode::BAD_GATEWAY)
            .is_some_and(|failure| failure.contains("could not check")));
    }

    #[test]
    fn the_connected_list_survives_a_restart_and_drops_unknown_providers() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("model-providers.json");
        std::fs::write(&path, r#"["baseten","retired-provider"]"#).expect("write list");
        let store = Store::load(path.clone());
        assert_eq!(store.connected(), BTreeSet::from(["baseten".to_string()]));
        store.set_connected("groq", true).expect("connect");
        assert_eq!(
            Store::load(path.clone()).connected(),
            BTreeSet::from(["baseten".to_string(), "groq".to_string()])
        );
        store.set_connected("baseten", false).expect("disconnect");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read list"),
            r#"["groq"]"#
        );
    }
}
