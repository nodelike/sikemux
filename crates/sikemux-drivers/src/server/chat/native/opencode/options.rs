//! The models, efforts and modes OpenCode offers, read from a server of its own with no session,
//! as the rows its ACP session would show.

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::acp::pickers;
use crate::sessions::opencode_server::Server;

/// What OpenCode runs a model at when no effort is picked: the provider's own default.
const DEFAULT_EFFORT: &str = "default";
const EFFORT_ID: &str = "effort";
const EFFORT_NAME: &str = "Effort";
const EFFORT_ABOUT: &str = "Available effort levels for this model";

#[derive(Deserialize)]
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
}

#[derive(Deserialize)]
struct Variant {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mode {
    id: String,
    name: Option<String>,
    description: Option<String>,
    mode: String,
    #[serde(default)]
    hidden: bool,
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

/// Asks a server started from `command` in `directory` what a chat there can run on, the
/// model picked first when OpenCode has it, at `effort` when that model takes it.
pub async fn read_options(
    command: tokio::process::Command,
    directory: &Path,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Value, String> {
    let server = Server::start(command, directory).await?;
    let read = tokio::try_join!(
        server.get("/api/model"),
        server.get("/api/model/default"),
        server.get("/api/agent"),
    );
    server.close().await;
    let (listed, default, modes) = read?;
    let mut listed: Vec<Listed> = serde_json::from_value(listed).map_err(|error| {
        format!("OpenCode listed its models in a shape Sikemux cannot read: {error}")
    })?;
    listed.retain(|model| model.enabled);
    if listed.is_empty() {
        return Err("OpenCode has no models it can run".to_owned());
    }
    listed.sort_by(|a, b| (&a.provider_id, &a.name).cmp(&(&b.provider_id, &b.name)));
    let default = serde_json::from_value::<Listed>(default)
        .ok()
        .map(|model| model.value());
    let current = model
        .map(str::to_owned)
        .filter(|model| listed.iter().any(|entry| &entry.value() == model))
        .or(default)
        .unwrap_or_else(|| listed[0].value());
    let modes: Vec<Mode> = serde_json::from_value(modes).unwrap_or_default();
    Ok(rows(&listed, &current, effort, &modes))
}

fn rows(listed: &[Listed], current: &str, effort: Option<&str>, modes: &[Mode]) -> Value {
    let choices = listed
        .iter()
        .map(|model| {
            let value = model.value();
            let name = format!(
                "{}/{}",
                model.provider_id,
                model.name.as_deref().unwrap_or(&model.id)
            );
            pickers::with_efforts(
                pickers::choice(&value, &name, None),
                &model.efforts(),
                (!model.variants.is_empty()).then_some(DEFAULT_EFFORT),
            )
        })
        .collect();
    let mut options = json!([pickers::with_effort_row(
        pickers::model_select(None, current, choices),
        EFFORT_ID,
        EFFORT_NAME,
        EFFORT_ABOUT,
    )]);
    pickers::fit_effort(&mut options);
    if let Some(effort) = effort {
        pickers::pick(&mut options, sikemux_fold::pickers::is_effort, effort);
    }
    let modes: Vec<Value> = modes
        .iter()
        .filter(|mode| mode.mode == "primary" && !mode.hidden)
        .map(|mode| {
            pickers::choice(
                &mode.id,
                mode.name.as_deref().unwrap_or(&mode.id),
                mode.description.as_deref(),
            )
        })
        .collect();
    if let (Some(first), Some(rows)) = (modes.first(), options.as_array_mut()) {
        let current = modes
            .iter()
            .find(|mode| mode["value"] == "build")
            .unwrap_or(first)["value"]
            .clone();
        rows.push(json!({
            "id": "mode",
            "name": "Session Mode",
            "category": "mode",
            "type": "select",
            "currentValue": current,
            "options": modes,
        }));
    }
    options
}
