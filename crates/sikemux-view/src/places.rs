//! A project's places: what its agents' actions can change outside this machine, which of
//! them are production, and how much the person lets agents do there. A change to a
//! production place at `mine` waits for the person's yes on a call; everything else runs.
//! What agents sent out that no place covers is listed once under `seen`.

use serde::{Deserialize, Serialize};

use crate::memory::Learned;

/// How much agents may do at a production place without the person.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// Agents change it and report after.
    Yours,
    /// Agents change it and the person is told at once.
    TellMe,
    /// A change waits for the person's yes on a call.
    #[default]
    Mine,
}

/// What kind of place it is, which a client draws with that service's mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaceKind {
    Vercel,
    Github,
    Database,
    Aws,
    Kubernetes,
    Terraform,
    Rundeck,
    Site,
    Connector,
    Command,
    /// A kind from a later release.
    #[serde(other)]
    Other,
}

/// A hard fact that says an action lands at a place.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Recognise {
    /// A Rundeck job's group/name path, where `*` stands for anything, such as `dev/*`.
    RundeckJob {
        path: String,
    },
    VercelTarget {
        production: bool,
    },
    AwsProfile {
        name: String,
    },
    KubeContext {
        name: String,
    },
    TerraformWorkspace {
        name: String,
    },
    /// A command of the project's own that lands there, by its leading words.
    Command {
        words: String,
    },
    /// A website, by its origin, such as `https://hooks.slack.com`.
    Site {
        origin: String,
    },
    /// A connector's service, such as Jira.
    Connector {
        name: String,
    },
    /// What is published for others to install, by where it goes, such as `npm` or
    /// `GitHub releases`.
    Package {
        registry: String,
    },
    /// A database the database connector marks as production, by its connection's name.
    Database {
        name: String,
    },
    /// Tags pushed to the project's remote, by a pattern where `*` stands for anything, such
    /// as `v*`, leaving out those `except` covers.
    Tag {
        pattern: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        except: Option<String>,
    },
    /// What `fact` names, as the tool's own config named it at `at`: a folder, a config file
    /// or an account. The same name read elsewhere is another target.
    Read {
        at: String,
        fact: Box<Recognise>,
    },
    /// A stack, stage or app a deploy tool names, such as `Heroku` and `acme-web`.
    Stage {
        tool: String,
        name: String,
    },
}

/// How the calls asked about a place went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaceStats {
    pub asked: u32,
    /// Answered as the call's pick.
    pub as_picked: u32,
    /// Answered against the pick.
    pub overruled: u32,
}

/// One place, as clients show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: String,
    /// In plain words, such as "acme.dev on Vercel".
    pub name: String,
    pub kind: PlaceKind,
    pub production: bool,
    /// Only a production place's level decides anything.
    pub level: Level,
    /// Where the core found it, such as "vercel.json", "the Vercel connector" or "@Ada's
    /// deploy"; none for one the person added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found_in: Option<String>,
    /// Who last set its level, and where; none while it has the level it was found with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said_by: Option<Learned>,
    /// Where the person set it, such as "Sikemux" or "a paired phone".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said_from: Option<String>,
    /// The level before that, which Undo puts back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<Level>,
    #[serde(default)]
    pub stats: PlaceStats,
}

/// Something agents sent out that no place covers, listed once however often it ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    pub id: String,
    /// Where it went, such as `hooks.slack.com`.
    pub what: String,
    /// What sent it, such as `scripts/notify.ts`, when that is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub times: u32,
    /// The agent that last sent it, by id, and what it was called then.
    pub last_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    pub last_at: u64,
}

/// A project's places, as the Playbook's Places page reads them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Places {
    pub places: Vec<Place>,
    pub seen: Vec<Seen>,
    /// When the person confirmed the list of production places; until then every one found
    /// is at `mine`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<u64>,
    /// How many more things agents sent out that no place covers, past what the list holds
    /// until the person counts some as production.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more_seen: u32,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}
