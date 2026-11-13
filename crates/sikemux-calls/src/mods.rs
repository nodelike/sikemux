use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// A mod as clients list it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInfo {
    /// Its folder's name.
    pub id: String,
    pub name: String,
    /// One of the app's icon names, such as `git-pull-request`.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub description: String,
    /// The data and connectors it reads, such as `agents` or `github`.
    #[serde(default)]
    pub reads: Vec<String>,
    /// What it may change once the person allows it, such as `agents.send`.
    #[serde(default)]
    pub writes: Vec<String>,
    /// Why it cannot be shown, such as a `mod.json` that does not read.
    #[serde(default)]
    pub problem: Option<String>,
    /// Whether the person allowed the writes it declares.
    #[serde(default)]
    pub allowed: bool,
    /// A few words the mod's page shows beside its name, such as "3 failing".
    #[serde(default)]
    pub badge: Option<String>,
    /// The folder of the project it is pinned to, where alone it shows.
    #[serde(default)]
    pub project: Option<String>,
    /// Turned off by the person: it neither shows nor reads.
    #[serde(default)]
    pub off: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModExample {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Already in the mods folder.
    pub added: bool,
}

/// The mods folder, every mod in it, and the examples there are to copy.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModList {
    pub folder: PathBuf,
    pub mods: Vec<ModInfo>,
    #[serde(default)]
    pub examples: Vec<ModExample>,
}

#[derive(Deserialize, Serialize)]
pub struct AllowParams {
    pub id: String,
    /// The writes the person was shown and allowed, or none to take the approval back.
    pub writes: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize)]
pub struct BadgeParams {
    pub id: String,
    pub badge: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct ModParams {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct ModPage {
    pub html: String,
}

#[derive(Deserialize, Serialize)]
pub struct PinModParams {
    pub id: String,
    /// The project folder it shows in alone, or none to show it in every project.
    pub project: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct OffParams {
    pub id: String,
    pub off: bool,
}

/// What a mod's page asks through its bridge. `name` and `params` are the page's own, so the
/// core works out what they mean.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeParams {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub params: Value,
    /// The project of the strip the mod's pane is in.
    #[serde(default)]
    pub project: Option<String>,
}

/// How a mod's write went: done, or waiting on the person to allow what its manifest declares.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BridgeWrite {
    Ask { name: String, writes: Vec<String> },
    Done(Value),
}
