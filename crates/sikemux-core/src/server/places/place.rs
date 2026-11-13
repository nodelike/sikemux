//! One place as the core keeps it: what clients show, the hard facts that recognise an action
//! landing there, its health address, and the step it gives a slice's ladder.

use serde_json::Value;
pub use sikemux_calls::places::{Passes, Place, StepPass};
use sikemux_view::places::{Level, Place as Shown, PlaceKind, PlaceStats, Recognise};

use crate::ship::{leads, Ship, Target};
pub(crate) use crate::ship::{says_not_production, says_production, GITHUB_RELEASES};

pub(crate) const MAX_PLACES: usize = 32;
const MAX_RECOGNISED: usize = 128;
const MAX_NAME_CHARS: usize = 64;
const MAX_VALUE_CHARS: usize = 200;
/// What a step may not be called, in any case: the gates and the keys they go by.
const RESERVED: [&str; 4] = ["Built", "Looks right", "looksRight", "Landed"];

pub(crate) trait PlaceExt: Sized {
    fn recognises(&self, ship: &Ship) -> bool;
}

impl PlaceExt for Place {
    fn recognises(&self, ship: &Ship) -> bool {
        self.recognise.iter().any(|fact| covers(fact, ship))
    }
}

/// Whether `fact` says `ship` lands where it names.
pub(crate) fn covers(fact: &Recognise, ship: &Ship) -> bool {
    if let Recognise::Read { at, fact } = fact {
        return ship.target.as_ref().and_then(Target::read_at) == Some(at.as_str())
            && covers(fact, ship);
    }
    match (fact, ship.target.as_ref().map(Target::bare)) {
        (Recognise::RundeckJob { path }, Some(Target::RundeckJob { path: job, .. })) => {
            matches_path(path, job)
        }
        (Recognise::VercelTarget { production }, Some(Target::Vercel { production: at })) => {
            production == at
        }
        (Recognise::AwsProfile { name }, Some(Target::AwsProfile { name: at }))
        | (Recognise::KubeContext { name }, Some(Target::KubeContext { name: at }))
        | (Recognise::TerraformWorkspace { name }, Some(Target::TerraformWorkspace { name: at })) => {
            name == at
        }
        (Recognise::Site { origin }, Some(Target::Site { origin: at })) => origin == at,
        (Recognise::Connector { name }, Some(Target::Connector { name: at })) => {
            name.eq_ignore_ascii_case(at)
        }
        (Recognise::Database { name }, Some(Target::Database { name: at })) => name == at,
        (Recognise::Tag { pattern, except }, Some(Target::Tag { name })) => {
            name.is_empty()
                || (matches_path(pattern, name)
                    && !except
                        .as_ref()
                        .is_some_and(|except| matches_path(except, name)))
        }
        (Recognise::Stage { tool, name }, Some(Target::Stage { tool: by, name: at })) => {
            tool == by && name == at
        }
        (Recognise::Package { registry }, Some(Target::Package { registry: at })) => registry == at,
        (Recognise::Command { words }, _) => {
            let said: Vec<String> = ship.what.split_whitespace().map(str::to_owned).collect();
            leads(words, &said)
        }
        _ => false,
    }
}

/// Whether `pattern`, where `*` stands for anything, covers all of `path`, in any case.
pub(crate) fn matches_path(pattern: &str, path: &str) -> bool {
    let (pattern, path) = (pattern.to_lowercase(), path.to_lowercase());
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = pieces.collect();
    let Some((last, middle)) = pieces.split_last() else {
        return rest.is_empty();
    };
    for piece in middle {
        let Some(at) = rest.find(piece) else {
            return false;
        };
        rest = &rest[at + piece.len()..];
    }
    rest.ends_with(last)
}

/// A Heroku app is production unless its name says it isn't; another tool's stage is when
/// its name says so.
pub(crate) fn stage_is_production(tool: &str, name: &str) -> bool {
    match tool {
        "Heroku" => !says_not_production(name),
        _ => says_production(name),
    }
}

/// An id made of `name`'s letters and digits, for a place or a seen action.
pub(crate) fn id_of(name: &str) -> String {
    let mut id = String::new();
    for c in name.chars() {
        match c.is_ascii_alphanumeric() {
            true => id.push(c.to_ascii_lowercase()),
            false if !id.ends_with('-') && !id.is_empty() => id.push('-'),
            false => {}
        }
    }
    id.trim_end_matches('-').to_owned()
}

pub(crate) fn kind_of(fact: &Recognise) -> PlaceKind {
    match fact {
        Recognise::RundeckJob { .. } => PlaceKind::Rundeck,
        Recognise::VercelTarget { .. } => PlaceKind::Vercel,
        Recognise::AwsProfile { .. } => PlaceKind::Aws,
        Recognise::KubeContext { .. } => PlaceKind::Kubernetes,
        Recognise::TerraformWorkspace { .. } => PlaceKind::Terraform,
        Recognise::Command { .. } => PlaceKind::Command,
        Recognise::Site { .. } => PlaceKind::Site,
        Recognise::Connector { .. } => PlaceKind::Connector,
        Recognise::Package { registry } if registry == GITHUB_RELEASES => PlaceKind::Github,
        Recognise::Package { .. } => PlaceKind::Other,
        Recognise::Database { .. } => PlaceKind::Database,
        Recognise::Stage { tool, .. }
            if matches!(tool.as_str(), "Pulumi" | "Serverless" | "CDK") =>
        {
            PlaceKind::Aws
        }
        Recognise::Stage { .. } => PlaceKind::Other,
        Recognise::Tag { .. } => PlaceKind::Github,
        Recognise::Read { fact, .. } => kind_of(fact),
    }
}

/// Whether a fact by itself says production, whatever the place holding it is marked.
fn fact_says_production(fact: &Recognise) -> bool {
    match fact {
        Recognise::VercelTarget { production } => *production,
        Recognise::RundeckJob { path } => (path.split('/').next()).is_some_and(says_production),
        Recognise::AwsProfile { name }
        | Recognise::KubeContext { name }
        | Recognise::TerraformWorkspace { name } => says_production(name),
        Recognise::Package { .. } | Recognise::Database { .. } => true,
        Recognise::Stage { tool, name } => stage_is_production(tool, name),
        Recognise::Tag { .. } => true,
        Recognise::Read { fact, .. } => fact_says_production(fact),
        Recognise::Command { .. } | Recognise::Site { .. } | Recognise::Connector { .. } => false,
    }
}

pub(crate) fn new_place(
    name: &str,
    production: bool,
    recognise: Vec<Recognise>,
    found_in: Option<String>,
) -> Place {
    let kind = recognise.first().map_or(PlaceKind::Other, kind_of);
    Place {
        shown: Shown {
            id: id_of(name),
            name: name.to_owned(),
            kind,
            production,
            level: match production {
                true => Level::Mine,
                false => Level::Yours,
            },
            found_in,
            said_by: None,
            said_from: None,
            was: None,
            stats: PlaceStats::default(),
        },
        recognise,
        health: None,
        step: None,
        recurs: false,
    }
}

/// A place from an environment as an earlier core kept it: one the person or an agent's
/// accepted proposal set to wait for a person, or whose name or facts read as production, is
/// a production place at `mine`; the rest are not production and keep their step's rule.
pub(crate) fn from_environment(environment: &Value) -> Option<Place> {
    let name = environment["name"].as_str()?.trim();
    let recognise: Vec<Recognise> = (environment["recognise"].as_array()?.iter())
        .filter_map(from_old_fact)
        .collect();
    let passes = environment["passes"].as_str().unwrap_or("person");
    let production = matches!(passes, "person" | "production")
        || says_production(name)
        || recognise.iter().any(fact_says_production);
    let mut place = new_place(name, production, recognise, None);
    place.health = environment["health"].as_str().map(str::to_owned);
    let no_step = environment["noStep"].as_bool().unwrap_or(false);
    place.step = (!no_step).then_some(match (production, passes) {
        (false, "check") => StepPass::Check,
        _ => StepPass::Deploy,
    });
    Some(place)
}

/// An earlier core kept what the person always allowed sites and connectors by as command
/// words, `site <origin>` and `connector <name>`.
fn from_old_fact(fact: &Value) -> Option<Recognise> {
    if fact["kind"] == "command" {
        let words = fact["words"].as_str()?;
        if let Some(origin) = words.strip_prefix("site ") {
            return Some(Recognise::Site {
                origin: origin.to_owned(),
            });
        }
        if let Some(name) = words.strip_prefix("connector ") {
            return Some(Recognise::Connector {
                name: name.to_owned(),
            });
        }
    }
    serde_json::from_value(fact.clone()).ok()
}

/// A place the person adds, tidied and held to what no setting may do: a fact that reads
/// as production is never on a place marked as not production.
pub(crate) fn checked(mut place: Place, others: &[Place]) -> Result<Place, String> {
    place.shown.name = place.shown.name.trim().to_owned();
    let name = &place.shown.name;
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS || name.contains('\n') {
        return Err(format!(
            "a place's name is one line of at most {MAX_NAME_CHARS} characters"
        ));
    }
    if RESERVED.iter().any(|word| name.eq_ignore_ascii_case(word)) {
        return Err(format!(
            "{name} is a gate every slice can have, so name it something else"
        ));
    }
    if others.len() >= MAX_PLACES {
        return Err(format!("a project has at most {MAX_PLACES} places"));
    }
    if (others.iter()).any(|other| {
        other.shown.id == place.shown.id || other.shown.name.eq_ignore_ascii_case(name)
    }) {
        return Err(format!("the project already has a place called {name}"));
    }
    if place.recognise.is_empty() || place.recognise.len() > MAX_RECOGNISED {
        return Err(format!(
            "{name} needs from 1 to {MAX_RECOGNISED} facts to recognise it by"
        ));
    }
    for fact in &mut place.recognise {
        let (Recognise::RundeckJob { path: value }
        | Recognise::AwsProfile { name: value }
        | Recognise::KubeContext { name: value }
        | Recognise::TerraformWorkspace { name: value }
        | Recognise::Command { words: value }
        | Recognise::Site { origin: value }
        | Recognise::Connector { name: value }
        | Recognise::Database { name: value }
        | Recognise::Package { registry: value }
        | Recognise::Stage { name: value, .. }
        | Recognise::Tag { pattern: value, .. }
        | Recognise::Read { at: value, .. }) = fact
        else {
            continue;
        };
        *value = value.trim().to_owned();
        if value.is_empty() || value.chars().count() > MAX_VALUE_CHARS {
            return Err(format!(
                "{}: what recognises it is at most {MAX_VALUE_CHARS} characters, and not empty",
                place.shown.name
            ));
        }
    }
    if !place.shown.production
        && (says_production(&place.shown.name) || place.recognise.iter().any(fact_says_production))
    {
        return Err(format!(
            "{} reads as production, so it is a production place",
            place.shown.name
        ));
    }
    if place.shown.production && place.step == Some(StepPass::Check) {
        place.step = Some(StepPass::Deploy);
    }
    Ok(place)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_pattern_covers_the_whole_path() {
        assert!(matches_path("dev/*", "dev/backend/deploy"));
        assert!(matches_path("DEV/*", "dev/x"));
        assert!(matches_path("*/deploy", "dev/deploy"));
        assert!(!matches_path("dev/*", "develop/x"));
        assert!(!matches_path("dev/x", "dev/x/y"));
    }

    #[test]
    fn nothing_that_reads_as_production_may_be_added_as_not_production() {
        let refused = [
            new_place(
                "Production",
                false,
                vec![Recognise::Command { words: "x".into() }],
                None,
            ),
            new_place(
                "prod-eu",
                false,
                vec![Recognise::Command { words: "x".into() }],
                None,
            ),
            new_place(
                "Safe",
                false,
                vec![Recognise::RundeckJob {
                    path: "prod/*".into(),
                }],
                None,
            ),
            new_place(
                "Safe",
                false,
                vec![Recognise::VercelTarget { production: true }],
                None,
            ),
            new_place(
                "Safe",
                false,
                vec![Recognise::AwsProfile {
                    name: "acme-prod".into(),
                }],
                None,
            ),
        ];
        for one in refused {
            assert!(checked(one.clone(), &[]).is_err(), "{one:?}");
        }
    }

    #[test]
    fn a_name_that_is_a_gate_is_refused() {
        for name in ["Built", "landed", "LOOKS RIGHT", "looksright"] {
            let place = new_place(
                name,
                true,
                vec![Recognise::Command { words: "x".into() }],
                None,
            );
            assert!(checked(place, &[]).is_err(), "{name}");
        }
    }
}
