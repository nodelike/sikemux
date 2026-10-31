//! Pre-picks how to start what the person types in the start box, at once
//! and without a model, from hard facts only: text naming a path of the
//! project that touches a server, a migration, a wire format or what users
//! see is planned first, and anything else is done at once. Judging size and
//! vagueness needs a model, which the core has no quick call to yet.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_wire::start::{Because, HardFact, StartMode, Triage};

use super::board::project_at;
use super::connection::blocking;
use super::Core;

/// The most of the text's words read for paths.
const MAX_WORDS: usize = 400;
/// The most paths looked up in the project.
const MAX_PATHS: usize = 32;

/// How a rule recognises a path.
enum Mark {
    /// One of the folders on its way, such as `server`.
    Folder(&'static str),
    /// What its name ends with, such as `.tsx`.
    Ending(&'static str),
}

/// What calls for planning first, by the paths it touches. The first rule a
/// path meets decides.
const RULES: [(Mark, HardFact); 17] = [
    (Mark::Folder("server"), HardFact::Server),
    (Mark::Folder("migrations"), HardFact::Migration),
    (Mark::Folder("migrate"), HardFact::Migration),
    (Mark::Folder("protocol"), HardFact::Protocol),
    (Mark::Folder("schema"), HardFact::Protocol),
    (Mark::Ending("-wire"), HardFact::Protocol),
    (Mark::Ending(".proto"), HardFact::Protocol),
    (Mark::Folder("app"), HardFact::SeenByUsers),
    (Mark::Folder("ui"), HardFact::SeenByUsers),
    (Mark::Folder("components"), HardFact::SeenByUsers),
    (Mark::Folder("pages"), HardFact::SeenByUsers),
    (Mark::Folder("screens"), HardFact::SeenByUsers),
    (Mark::Folder("mobile"), HardFact::SeenByUsers),
    (Mark::Ending(".tsx"), HardFact::SeenByUsers),
    (Mark::Ending(".css"), HardFact::SeenByUsers),
    (Mark::Ending(".html"), HardFact::SeenByUsers),
    (Mark::Ending(".swift"), HardFact::SeenByUsers),
];

fn fact_of(path: &str) -> Option<HardFact> {
    let path = path.to_lowercase();
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    RULES.iter().find_map(|(mark, fact)| {
        let met = match mark {
            Mark::Folder(folder) => parts.iter().any(|part| part == folder),
            Mark::Ending(ending) => parts.iter().any(|part| part.ends_with(ending)),
        };
        met.then_some(*fact)
    })
}

/// The words of `text` that name a file or folder of the project, in order.
fn paths_named(project: &Path, text: &str) -> Vec<String> {
    let trim: &[char] = &[
        '`', '"', '\'', '(', ')', '[', ']', '<', '>', ',', ';', ':', '!', '?',
    ];
    text.split_whitespace()
        .take(MAX_WORDS)
        .map(|word| word.trim_matches(trim).trim_end_matches('.'))
        .filter(|word| (word.contains('/') || word.contains('.')) && !word.contains("://"))
        .filter(|word| !word.starts_with('/') && !word.split('/').any(|part| part == ".."))
        .take(MAX_PATHS)
        .filter(|word| project.join(word.trim_start_matches("./")).exists())
        .map(str::to_owned)
        .collect()
}

fn triaged(project: &Path, text: &str) -> Triage {
    let because = paths_named(project, text).into_iter().find_map(|path| {
        Some(Because {
            fact: fact_of(&path)?,
            path,
        })
    });
    Triage {
        mode: if because.is_some() {
            StartMode::PlanFirst
        } else {
            StartMode::DoIt
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
