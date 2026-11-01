//! Pre-picks how to start what the person types in the start box, at once
//! and without a model, from hard facts only: text naming a path of the
//! project that touches what every project plans first, such as a migration
//! or a wire format, or a path the project's sikemux.json lists under
//! `planFirst`, is planned first, and anything else is done at once. Judging
//! size and vagueness needs a model, which the core has no quick call to yet.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_wire::start::{Because, HardFact, StartMode, Triage};

use super::board::project_at;
use super::connection::blocking;
use super::Core;
use crate::tasks::config::{self, ConfigLoad, PathGlobs};

/// The most of the text's words read for paths.
const MAX_WORDS: usize = 400;
/// The most paths looked up in the project.
const MAX_PATHS: usize = 32;

/// How a rule recognises a path.
enum Mark {
    /// Its first folder, such as `server`.
    Top(&'static str),
    /// One of the folders on its way, such as `migrations`.
    Folder(&'static str),
    /// What its name ends with, such as `.proto`.
    Ending(&'static str),
}

/// What every project plans first, by the paths it touches. The first rule a
/// path meets decides.
const RULES: [(Mark, HardFact); 16] = [
    (Mark::Folder("migrations"), HardFact::Migration),
    (Mark::Ending(".proto"), HardFact::Protocol),
    (Mark::Ending(".graphql"), HardFact::Protocol),
    (Mark::Ending("openapi.json"), HardFact::Protocol),
    (Mark::Ending("openapi.yaml"), HardFact::Protocol),
    (Mark::Ending("openapi.yml"), HardFact::Protocol),
    (Mark::Ending("swagger.json"), HardFact::Protocol),
    (Mark::Ending("swagger.yaml"), HardFact::Protocol),
    (Mark::Top("server"), HardFact::Server),
    (Mark::Top("api"), HardFact::Server),
    (Mark::Folder("ui"), HardFact::SeenByUsers),
    (Mark::Folder("components"), HardFact::SeenByUsers),
    (Mark::Folder("pages"), HardFact::SeenByUsers),
    (Mark::Folder("screens"), HardFact::SeenByUsers),
    (Mark::Folder("views"), HardFact::SeenByUsers),
    (Mark::Folder("templates"), HardFact::SeenByUsers),
];

fn fact_of(path: &str) -> Option<HardFact> {
    let path = path.to_lowercase();
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    RULES.iter().find_map(|(mark, fact)| {
        let met = match mark {
            Mark::Top(folder) => parts.first() == Some(folder),
            Mark::Folder(folder) => parts.iter().any(|part| part == folder),
            Mark::Ending(ending) => parts.last().is_some_and(|part| part.ends_with(ending)),
        };
        met.then_some(*fact)
    })
}

/// Whether the project's own list names `path`; a folder counts when a file
/// in it would.
fn listed(globs: &PathGlobs, project: &Path, path: &str) -> bool {
    let path = path.trim_end_matches('/');
    globs.set.is_match(path)
        || (project.join(path).is_dir() && globs.set.is_match(format!("{path}/-")))
}

/// The words of `text` that name a file or folder inside the project, in order.
fn paths_named(project: &Path, text: &str) -> Vec<String> {
    let trim: &[char] = &[
        '`', '"', '\'', '(', ')', '[', ']', '<', '>', ',', ';', ':', '!', '?',
    ];
    text.split_whitespace()
        .take(MAX_WORDS)
        .map(|word| word.trim_matches(trim).trim_end_matches('.'))
        .map(|word| word.trim_start_matches("./"))
        .filter(|word| (word.contains('/') || word.contains('.')) && !word.contains("://"))
        .filter(|word| !word.starts_with('/') && !word.split('/').any(|part| part == ".."))
        .take(MAX_PATHS)
        .filter(|word| project.join(word).exists())
        .map(str::to_owned)
        .collect()
}

fn triaged(project: &Path, text: &str) -> Triage {
    let globs = match config::load(project) {
        ConfigLoad::Valid { config, .. } => config.plan_first,
        _ => None,
    };
    let because = paths_named(project, text).into_iter().find_map(|path| {
        let fact = match &globs {
            Some(globs) if listed(globs, project, &path) => HardFact::Listed,
            _ => fact_of(&path)?,
        };
        Some(Because { fact, path })
    });
    Triage {
        mode: if because.is_some() {
            StartMode::PlanFirst
        } else {
            StartMode::default()
        },
        because,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageParams {
    pub project: String,
    /// What the person typed so far.
    pub text: String,
}

pub(crate) async fn triage(core: &Arc<Core>, params: TriageParams) -> Result<Triage, String> {
    let project = project_at(core, &params.project)?;
    blocking(move || Ok(triaged(Path::new(&project), &params.text)))
        .await
        .map_err(|error| error.to_string())
}
