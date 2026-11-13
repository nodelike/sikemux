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
    /// The origins its page may reach, such as `https://api.example.com`, once the person
    /// allows it; nothing else on the network.
    #[serde(default)]
    pub connect: Vec<String>,
    /// Why it cannot be shown, such as a `mod.json` that does not read.
    #[serde(default)]
    pub problem: Option<String>,
    /// Whether the person allowed the writes and origins it declares.
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
    /// The origins the person was shown and allowed with them.
    #[serde(default)]
    pub connect: Vec<String>,
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
    /// What the person has yet to allow before the page may reach the network.
    #[serde(default)]
    pub ask: Option<ModAsk>,
}

/// Everything a mod declares it may change or reach, as the person is asked to allow it.
#[derive(Clone, Serialize, Deserialize)]
pub struct ModAsk {
    pub name: String,
    pub writes: Vec<String>,
    pub connect: Vec<String>,
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
    Ask {
        name: String,
        writes: Vec<String>,
        #[serde(default)]
        connect: Vec<String>,
    },
    Done(Value),
}

/// What an agent is asked to make a mod from what the person wrote.
pub fn make_prompt(asked: &str, folder: &str) -> String {
    format!(
        "Make a Sikemux mod: {asked}\n\nFollow the sikemux-mods skill: make it in a new folder of {folder}, then open it with ui_open."
    )
}

/// What an agent is asked to change a mod from what the person wrote.
pub fn change_prompt(name: &str, folder: &str, id: &str, asked: &str) -> String {
    format!(
        "Change the Sikemux mod \"{name}\" in {folder}/{id}: {asked}\n\nFollow the sikemux-mods skill."
    )
}

/// What makes a mod's read of a name out of date: this host's view, or a topic's change.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    View,
    Topic(sikemux_view::changed::Topic),
}

/// What a mod's watch of `name` reads again on; a name with nothing to follow is read again on
/// a timer of the page's own while the pane shows.
pub fn follows(name: &str) -> Option<Moved> {
    use sikemux_view::changed::Topic;
    Some(match name {
        "agents" | "inbox" => Moved::View,
        "board" => Moved::Topic(Topic::Boards),
        "tasks" => Moved::Topic(Topic::Tasks),
        "runs" => Moved::Topic(Topic::Runs),
        "usage" => Moved::Topic(Topic::Usage),
        "questions" => Moved::Topic(Topic::Questions),
        "data" => Moved::Topic(Topic::ModData),
        name if name.starts_with("git.") => Moved::Topic(Topic::Git),
        _ => return None,
    })
}
