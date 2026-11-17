use serde::{Deserialize, Serialize};
use sikemux_agents::skills::Skill;
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnSkills {
    pub agent: String,
    pub folder: PathBuf,
    pub skills: Vec<Skill>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsOverview {
    pub library: PathBuf,
    pub skills: Vec<Skill>,
    pub own: Vec<OwnSkills>,
    /// Names in the library that one of the person's own skills also uses.
    pub clashes: Vec<String>,
}

/// A skill that appeared in an agent's own global folder while the core ran,
/// most likely written there by that agent's own skill maker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSkillAdded {
    pub agent: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub struct NewSkill {
    pub name: String,
    pub description: String,
    pub body: String,
}

#[derive(Serialize, Deserialize)]
pub struct Source {
    pub source: String,
    pub subfolder: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Named {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct Located {
    pub path: PathBuf,
}
