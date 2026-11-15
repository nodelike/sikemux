//! The models, efforts and modes OpenCode offers, read from its server, as the rows its ACP
//! session would show.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::acp::pickers;
use crate::sessions::opencode_server::{Client, Server};

/// What OpenCode runs a model at when no effort is picked: the provider's own default.
pub const DEFAULT_EFFORT: &str = "default";
pub const EFFORT_ID: &str = "effort";
pub const MODE_ID: &str = "mode";
const EFFORT_NAME: &str = "Effort";
const EFFORT_ABOUT: &str = "Available effort levels for this model";
/// The mode a session starts in, OpenCode's agent that does the work.
pub const BUILD: &str = "build";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    id: String,
    #[serde(rename = "providerID")]
    provider_id: String,
    name: Option<String>,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    variants: Vec<Variant>,
    #[serde(default)]
    limit: Limit,
}

#[derive(Clone, Serialize, Deserialize)]
struct Variant {
    id: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Limit {
    #[serde(default)]
    context: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mode {
    id: String,
    name: Option<String>,
    description: Option<String>,
    mode: String,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    permissions: Vec<Value>,
}

impl Listed {
    fn value(&self) -> String {
        format!("{}/{}", self.provider_id, self.id)
    }

    fn efforts(&self) -> Vec<&str> {
        let mut efforts: Vec<&str> = self
            .variants
            .iter()
            .map(|variant| variant.id.as_str())
            .collect();
        if !efforts.is_empty() {
            efforts.push(DEFAULT_EFFORT);
        }
        efforts
    }
}

/// A model as OpenCode names it in a session: its provider, its own id and the effort.
pub fn model_ref(value: &str, effort: Option<&str>) -> Option<Value> {
    let (provider, id) = value.split_once('/')?;
    let mut model = json!({ "providerID": provider, "id": id });
    if let Some(effort) = effort {
        model["variant"] = json!(effort);
    }
    Some(model)
}

/// A model reference OpenCode sent, as the model picker's value and the effort.
pub fn read_ref(model: &Value) -> Option<(String, Option<String>)> {
    let provider = model["providerID"].as_str()?;
    let id = model["id"].as_str()?;
    Some((
        format!("{provider}/{id}"),
        model["variant"].as_str().map(str::to_owned),
    ))
}

/// What a server offers to pick.
#[derive(Clone, Serialize, Deserialize)]
pub struct Catalog {
    listed: Vec<Listed>,
    default: Option<String>,
    modes: Vec<Mode>,
}

impl Catalog {
    pub async fn read(client: &Client) -> Result<Self, String> {
        let (listed, default, modes) = tokio::try_join!(
            client.get("/api/model"),
            client.get("/api/model/default"),
            client.get("/api/agent"),
        )
        .map_err(String::from)?;
        let mut listed: Vec<Listed> = serde_json::from_value(listed).map_err(|error| {
            format!("OpenCode listed its models in a shape Sikemux cannot read: {error}")
        })?;
        listed.retain(|model| model.enabled);
        if listed.is_empty() {
            return Err("OpenCode has no models it can run".to_owned());
        }
        listed.sort_by(|a, b| (&a.provider_id, &a.name).cmp(&(&b.provider_id, &b.name)));
        Ok(Self {
            listed,
            default: serde_json::from_value::<Listed>(default)
                .ok()
                .map(|model| model.value()),
            modes: serde_json::from_value(modes).unwrap_or_default(),
        })
    }

    fn find(&self, model: &str) -> Option<&Listed> {
        self.listed.iter().find(|entry| entry.value() == model)
    }

    /// `wanted` when OpenCode has it, else the model it picks itself.
    pub fn model(&self, wanted: Option<&str>) -> String {
        wanted
            .filter(|model| self.find(model).is_some())
            .map(str::to_owned)
            .or_else(|| self.default.clone())
            .unwrap_or_else(|| self.listed[0].value())
    }

    /// `wanted` when `model` takes it.
    pub fn effort(&self, model: &str, wanted: Option<&str>) -> Option<String> {
        let wanted = wanted?;
        self.find(model)?
            .efforts()
            .contains(&wanted)
            .then(|| wanted.to_owned())
    }

    /// The permission rules of the agent `mode` runs, as OpenCode built them from its own
    /// defaults and the person's config.
    pub fn permissions(&self, mode: &str) -> &[Value] {
        self.modes
            .iter()
            .find(|offered| offered.id == mode)
            .map_or(&[], |offered| &offered.permissions)
    }

    pub fn offers_mode(&self, mode: &str) -> bool {
        self.primary().any(|offered| offered.id == mode)
    }

    /// The context window of `model`, when OpenCode knows it.
    pub fn window(&self, model: &str) -> u64 {
        self.find(model).map_or(0, |entry| entry.limit.context)
    }

    fn primary(&self) -> impl Iterator<Item = &Mode> {
        self.modes
            .iter()
            .filter(|mode| mode.mode == "primary" && !mode.hidden)
    }

    /// The session's pickers, on `model` at `effort` in `mode`.
    pub fn rows(&self, model: &str, effort: Option<&str>, mode: Option<&str>) -> Value {
        let choices = self
            .listed
            .iter()
            .map(|entry| {
                let name = format!(
                    "{}/{}",
                    entry.provider_id,
                    entry.name.as_deref().unwrap_or(&entry.id)
                );
                pickers::with_efforts(
                    pickers::choice(&entry.value(), &name, None),
                    &entry.efforts(),
                    (!entry.variants.is_empty()).then_some(DEFAULT_EFFORT),
                )
            })
            .collect();
        let mut options = json!([pickers::with_effort_row(
            pickers::model_select(None, model, choices),
            EFFORT_ID,
            EFFORT_NAME,
            EFFORT_ABOUT,
        )]);
        pickers::fit_effort(&mut options);
        if let Some(effort) = effort {
            pickers::pick(&mut options, sikemux_fold::pickers::is_effort, effort);
        }
        let modes: Vec<Value> = self
            .primary()
            .map(|mode| {
                pickers::choice(
                    &mode.id,
                    mode.name.as_deref().unwrap_or(&mode.id),
                    mode.description.as_deref(),
                )
            })
            .collect();
        if let (Some(first), Some(rows)) = (modes.first(), options.as_array_mut()) {
            let offered = |id: &str| modes.iter().find(|choice| choice["value"] == id);
            let current = mode
                .and_then(offered)
                .or_else(|| offered(BUILD))
                .unwrap_or(first)["value"]
                .clone();
            rows.push(json!({
                "id": MODE_ID,
                "name": "Session Mode",
                "category": "mode",
                "type": "select",
                "currentValue": current,
                "options": modes,
            }));
        }
        options
    }
}

/// Asks a server started from `command` in `directory` what a chat there can run on, the
/// model picked first when OpenCode has it, at `effort` when that model takes it.
pub async fn read_options(
    command: tokio::process::Command,
    directory: &Path,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Value, String> {
    let server = Server::start(command, directory).await?;
    let catalog = Catalog::read(&server).await;
    server.close().await;
    let catalog = catalog?;
    let model = catalog.model(model);
    let effort = catalog.effort(&model, effort);
    Ok(catalog.rows(&model, effort.as_deref(), None))
}
