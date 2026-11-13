//! The one table of whether an action that leaves this machine runs. What it touches is read
//! from the facts of what runs. A production place decides by its level. When anything in the
//! action says production, that wins over a place or a name that says otherwise. A place
//! that isn't production, or a target whose name says it isn't, lets it run, and what no fact
//! places runs and is listed as seen.

use crate::server::places::place::PlaceExt;
use sikemux_view::inbox::ShipKind;
use sikemux_view::places::{Level, Recognise};

use super::place::{
    id_of, new_place, says_not_production, says_production, Passes, Place, GITHUB_RELEASES,
};
use crate::ship::{Ship, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goes {
    Freely,
    /// Once the project's check passes, at a place that isn't production whose step says so.
    OnceChecked,
    /// It runs and the person is told at once.
    Tells,
    /// It waits for the person's yes on a call.
    Asks,
    /// Nothing places it: it runs and is listed as seen.
    Unplaced,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judged {
    pub goes: Goes,
    /// The place that decided.
    pub place: Option<Place>,
    /// That place, when the core found it in this action and is to keep it.
    pub keep: bool,
    /// That place was read from the tool's own config, as the command named none, so the
    /// person's answer keeps it: production on a yes, not production when they say so.
    pub offered: bool,
    pub why: Option<String>,
}

/// Programs whose change, with nothing naming where it lands, counts as production.
const INFRA: &[&str] = &[
    "kubectl",
    "helm",
    "terraform",
    "aws",
    "pulumi",
    "cdk",
    "sst",
    "sam",
    "fly",
    "flyctl",
    "wrangler",
    "firebase",
    "railway",
    "netlify",
    "heroku",
    "dokku",
    "eas",
    "serverless",
    "sls",
];

/// Where `ship` lands among `places`, as the person `confirmed` the list or not. `agent` is
/// whose action it is, by name, for a place the core finds in it.
pub fn judged(places: &[Place], confirmed: bool, ship: &Ship, agent: &str) -> Judged {
    let recognising: Vec<&Place> = places
        .iter()
        .filter(|place| place.recognises(ship))
        .collect();
    let found_in = Some(format!("@{agent}'s {}", program_of(ship)));
    let freed_by_person = |found: &Place| {
        (recognising.iter()).any(|place| {
            !place.shown.production
                && place.shown.said_by.is_some()
                && found
                    .recognise
                    .iter()
                    .any(|fact| place.recognise.contains(fact))
        })
    };
    let read_at = ship.target.as_ref().and_then(Target::read_at);
    let fact = production_fact(ship, found_in.clone())
        .map(|found| match read_at {
            Some(at) if !found.recognise.is_empty() => read_from(found, at),
            _ => found,
        })
        .filter(|found| !freed_by_person(found));
    let named = fact.or_else(|| {
        says_production_anywhere(ship).then(|| {
            let (_, what) = seen_key(ship);
            let fact = Recognise::Command {
                words: ship.what.clone(),
            };
            new_place(&what, true, vec![fact], found_in)
        })
    });
    let resolved = (ship.target.as_ref()).is_some_and(|target| target.resolved_from().is_some());
    let fact_place = named.map(|found| match found.recognise.is_empty() {
        true => (found, false),
        false => (merged(places, found), !resolved),
    });
    let production: Vec<&Place> = (recognising.iter().copied())
        .filter(|place| place.shown.production)
        .collect();
    let covered = |found: &Place| {
        (production.iter()).any(|place| {
            found
                .recognise
                .iter()
                .any(|fact| place.recognise.contains(fact))
        })
    };
    let fact_place = fact_place.filter(|(found, _)| !covered(found));
    let strictest = (fact_place.as_ref().map(|(place, _)| place))
        .into_iter()
        .chain(production.iter().copied())
        .max_by_key(|place| strictness(place.level(confirmed)));
    if let Some(place) = strictest {
        let from_fact =
            (fact_place.as_ref()).is_some_and(|(found, _)| found.shown.id == place.shown.id);
        let keep = from_fact && fact_place.as_ref().is_some_and(|(_, keep)| *keep);
        let kept = places.iter().any(|kept| kept.shown.id == place.shown.id);
        let mut judged = by_level(place, confirmed, keep);
        judged.offered = from_fact
            && resolved
            && !kept
            && !place.recognise.is_empty()
            && !says_production(place.name());
        return judged;
    }
    if let Some(place) = recognising.first() {
        let goes = match place.passes(confirmed) {
            Passes::Check => Goes::OnceChecked,
            Passes::Nobody | Passes::Person => Goes::Freely,
        };
        let why = match goes {
            Goes::OnceChecked => format!("{} goes once the project's check passes.", place.name()),
            _ => format!("{} isn't production.", place.name()),
        };
        return Judged {
            goes,
            place: Some((*place).clone()),
            keep: false,
            offered: false,
            why: Some(why),
        };
    }
    let goes = match not_production_fact(ship) {
        true => Goes::Freely,
        false => Goes::Unplaced,
    };
    Judged {
        goes,
        place: None,
        keep: false,
        offered: false,
        why: None,
    }
}

/// `found` as read from the tool's own config at `at`, so the same name read elsewhere is
/// another place.
fn read_from(mut found: Place, at: &str) -> Place {
    found.recognise = (found.recognise.into_iter())
        .map(|fact| Recognise::Read {
            at: at.to_owned(),
            fact: Box::new(fact),
        })
        .collect();
    let name = format!("{}, read from {at}", found.shown.name);
    found.shown.id = super::place::id_of(&name);
    found.shown.name = name;
    found
}

/// The place the project keeps with `found`'s id, made production with `found`'s facts
/// added, or `found` itself.
fn merged(places: &[Place], found: Place) -> Place {
    let Some(kept) = places.iter().find(|place| place.shown.id == found.shown.id) else {
        return found;
    };
    let mut place = kept.clone();
    if !kept.shown.production {
        place.shown.production = true;
        place.shown.level = Level::Mine;
    }
    for fact in found.recognise {
        if !place.recognise.contains(&fact) {
            place.recognise.push(fact);
        }
    }
    place
}

fn strictness(level: Level) -> u8 {
    match level {
        Level::Yours => 0,
        Level::TellMe => 1,
        Level::Mine => 2,
    }
}

fn by_level(place: &Place, confirmed: bool, keep: bool) -> Judged {
    let (goes, why) = match place.level(confirmed) {
        Level::Yours => (
            Goes::Freely,
            format!("{} is the agents' to change.", place.name()),
        ),
        Level::TellMe => (
            Goes::Tells,
            format!("{} runs, and the person is told at once.", place.name()),
        ),
        Level::Mine => (
            Goes::Asks,
            format!(
                "{} is production, so a change there waits for the person's yes.",
                place.name()
            ),
        ),
    };
    Judged {
        goes,
        place: Some(place.clone()),
        keep,
        offered: false,
        why: Some(why),
    }
}

/// A place a deploy tool's stack, stage or app names, which is production unless its name
/// says otherwise.
pub fn stage_place(tool: &str, name: &str, found_in: Option<String>) -> Place {
    let fact = Recognise::Stage {
        tool: tool.to_owned(),
        name: name.to_owned(),
    };
    new_place(
        &format!("{name} on {tool}"),
        !says_not_production(name),
        vec![fact],
        found_in,
    )
}

/// The words of `ship` that say where it lands: the origin of a site it writes to, nothing of
/// what it writes to a connector, and every word of a command.
fn selecting_words(ship: &Ship) -> &str {
    match ship.target.as_ref().map(Target::bare) {
        Some(Target::Site { origin }) => origin,
        Some(Target::Connector { .. }) => "",
        _ => &ship.what,
    }
}

fn says_production_anywhere(ship: &Ship) -> bool {
    says_production(selecting_words(ship))
}

/// Whether a workflow run reaches production: one that publishes, deploys by a known step or a
/// command the gate would hold, or names a production environment, or one whose file name or
/// inputs say production, or that it releases, deploys or publishes, read or not.
pub fn workflow_asks(target: &Target) -> bool {
    let Target::Workflow {
        file,
        environments,
        publishes,
        deploys,
        read: _,
        inputs,
    } = target
    else {
        return false;
    };
    let named = std::iter::once(file).chain(inputs);
    let says_it = |word: &str| {
        word.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|part| {
                [
                    "release",
                    "releases",
                    "deploy",
                    "deploys",
                    "publish",
                    "prod",
                    "production",
                ]
                .iter()
                .any(|said| part.eq_ignore_ascii_case(said))
            })
    };
    *publishes
        || *deploys
        || environments.iter().any(|name| says_production(name))
        || named.clone().any(|word| says_production(word))
        || named.clone().any(|word| says_it(word))
}

/// The production place a fact of `ship` names by itself, whatever the project's list says:
/// a target its service marks as production, a package published for others, a write to a
/// production database, a workflow that reaches production, a stack, stage, app, profile,
/// context or workspace whose name doesn't say it isn't production, or an infrastructure
/// program with nothing naming where it lands.
fn production_fact(ship: &Ship, found_in: Option<String>) -> Option<Place> {
    let place =
        |name: String, fact: Recognise| new_place(&name, true, vec![fact], found_in.clone());
    let Some(target) = ship.target.as_ref().map(Target::bare) else {
        let program = ship.what.split_whitespace().next()?;
        let program = program.rsplit('/').next().unwrap_or(program);
        return (ship.kind == ShipKind::Deploy && INFRA.contains(&program)).then(|| {
            new_place(
                &format!("{program}, where nothing names it"),
                true,
                Vec::new(),
                found_in,
            )
        });
    };
    match target {
        Target::Vercel { production: true } => Some(place(
            "Production on Vercel".to_owned(),
            Recognise::VercelTarget { production: true },
        )),
        Target::RundeckJob {
            path,
            production: true,
        } => {
            let folder = path.split('/').next().unwrap_or(path);
            Some(place(
                format!("{folder} jobs on Rundeck"),
                Recognise::RundeckJob {
                    path: format!("{folder}/*"),
                },
            ))
        }
        Target::RundeckJob { path, .. } if says_production(path) => Some(place(
            format!("Rundeck job {path}"),
            Recognise::RundeckJob { path: path.clone() },
        )),
        Target::AwsProfile { name } if !says_not_production(name) => Some(place(
            format!("AWS profile {name}"),
            Recognise::AwsProfile { name: name.clone() },
        )),
        Target::KubeContext { name } if !says_not_production(name) => Some(place(
            format!("Kubernetes context {name}"),
            Recognise::KubeContext { name: name.clone() },
        )),
        Target::TerraformWorkspace { name } if !says_not_production(name) => Some(place(
            format!("Terraform workspace {name}"),
            Recognise::TerraformWorkspace { name: name.clone() },
        )),
        Target::Stage { tool, name } if name.is_empty() => Some(new_place(
            &format!("{tool}, where nothing names it"),
            true,
            Vec::new(),
            found_in,
        )),
        Target::Stage { tool, name } if !says_not_production(name) => {
            let mut place = stage_place(tool, name, found_in);
            place.shown.production = true;
            Some(place)
        }
        Target::Database { name } => Some(place(
            format!("The {name} database"),
            Recognise::Database { name: name.clone() },
        )),
        Target::Package { registry } if registry == GITHUB_RELEASES => Some(place(
            "Releases on GitHub".to_owned(),
            Recognise::Package {
                registry: registry.clone(),
            },
        )),
        Target::Package { registry } => Some(place(
            format!("Packages on {registry}"),
            Recognise::Package {
                registry: registry.clone(),
            },
        )),
        Target::Workflow {
            file,
            environments,
            publishes,
            ..
        } if workflow_asks(target) => {
            let words = Recognise::Command {
                words: format!("gh workflow run {file}"),
            };
            let environment = environments.iter().find(|name| says_production(name));
            let name = match (*publishes, environment) {
                (true, _) => "Releases on GitHub".to_owned(),
                (false, Some(environment)) => format!("{environment} on GitHub"),
                (false, None) => format!("The {file} workflow on GitHub"),
            };
            Some(place(name, words))
        }
        _ => None,
    }
}

/// Whether a fact of `ship` says it lands somewhere that isn't production. Only asked once
/// nothing in the action says production.
fn not_production_fact(ship: &Ship) -> bool {
    match ship.target.as_ref().map(Target::bare) {
        Some(Target::Vercel { production: false }) => true,
        Some(Target::RundeckJob { path, .. }) => {
            (path.split('/').next()).is_some_and(says_not_production)
        }
        Some(Target::AwsProfile { name })
        | Some(Target::KubeContext { name })
        | Some(Target::TerraformWorkspace { name })
        | Some(Target::Stage { name, .. }) => says_not_production(name),
        _ => false,
    }
}

fn program_of(ship: &Ship) -> String {
    match ship.target.as_ref().map(Target::bare) {
        Some(Target::Database { .. }) => "database write".to_owned(),
        _ => match ship.kind {
            ShipKind::Publish => "publish".to_owned(),
            _ => "deploy".to_owned(),
        },
    }
}

/// What an action nothing places is listed under once, however often it runs: where it
/// went, and an id for that.
pub fn seen_key(ship: &Ship) -> (String, String) {
    let what = match ship.target.as_ref().map(Target::bare) {
        Some(Target::Site { origin }) => origin
            .split_once("://")
            .map_or(origin.as_str(), |(_, host)| host)
            .to_owned(),
        Some(Target::Connector { name }) => name.clone(),
        Some(Target::RundeckJob { path, .. }) => format!("Rundeck job {path}"),
        Some(Target::AwsProfile { name }) => format!("AWS profile {name}"),
        Some(Target::KubeContext { name }) => format!("Kubernetes context {name}"),
        Some(Target::TerraformWorkspace { name }) => format!("Terraform workspace {name}"),
        Some(Target::Workflow { file, .. }) => format!("the {file} workflow"),
        Some(Target::Stage { tool, name }) => format!("{name} on {tool}"),
        _ => {
            let words: Vec<&str> = (ship.what.split_whitespace())
                .filter(|word| !word.starts_with('-') && !word.contains('='))
                .take(2)
                .collect();
            words.join(" ")
        }
    };
    (id_of(&what), what)
}

/// The fact that makes a seen action a production place, when the person counts it as one.
pub fn fact_of_seen(ship: &Ship) -> Recognise {
    match ship.target.as_ref().map(Target::bare) {
        Some(Target::Site { origin }) => Recognise::Site {
            origin: origin.clone(),
        },
        Some(Target::Connector { name }) => Recognise::Connector { name: name.clone() },
        Some(Target::RundeckJob { path, .. }) => Recognise::RundeckJob { path: path.clone() },
        Some(Target::AwsProfile { name }) => Recognise::AwsProfile { name: name.clone() },
        Some(Target::KubeContext { name }) => Recognise::KubeContext { name: name.clone() },
        Some(Target::TerraformWorkspace { name }) => {
            Recognise::TerraformWorkspace { name: name.clone() }
        }
        Some(Target::Workflow { file, .. }) => Recognise::Command {
            words: format!("gh workflow run {file}"),
        },
        Some(Target::Stage { tool, name }) => Recognise::Stage {
            tool: tool.clone(),
            name: name.clone(),
        },
        _ => Recognise::Command {
            words: seen_key(ship).1,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::place::StepPass;
    use super::*;

    fn job(name: &str, production: bool, path: &str, step: StepPass) -> Place {
        let mut place = new_place(
            name,
            production,
            vec![Recognise::RundeckJob {
                path: path.to_owned(),
            }],
            None,
        );
        place.step = Some(step);
        place
    }

    fn deploy(target: Option<Target>) -> Ship {
        Ship {
            kind: ShipKind::Deploy,
            what: "rundeck_run {}".to_owned(),
            target,
        }
    }

    fn at(path: &str, production: bool) -> Option<Target> {
        Some(Target::RundeckJob {
            path: path.to_owned(),
            production,
        })
    }

    fn project() -> Vec<Place> {
        vec![
            job("Dev", false, "dev/*", StepPass::Deploy),
            job("Staging", false, "staging/*", StepPass::Check),
            job("Production", true, "prod/*", StepPass::Deploy),
        ]
    }

    #[test]
    fn production_holds() {
        let judged = judged(&project(), true, &deploy(at("prod/deploy", false)), "Ada");
        assert_eq!(judged.goes, Goes::Asks);
        assert_eq!(
            judged.place.map(|place| place.shown.name),
            Some("Production".into())
        );
    }

    /// Unknown counted as production until 2026-11-12, when the chief PRD reversed it on
    /// purpose: what nothing places runs and is listed as seen.
    #[test]
    fn what_cannot_be_placed_runs_as_unplaced() {
        for ship in [deploy(None), deploy(at("backend/deploy", false))] {
            let judged = judged(&project(), true, &ship, "Ada");
            assert_eq!(judged.goes, Goes::Unplaced);
            assert_eq!(judged.place, None);
        }
        assert_eq!(
            judged(&project(), true, &deploy(at("qa/deploy", false)), "Ada").goes,
            Goes::Freely
        );
    }

    #[test]
    fn a_fact_that_says_production_holds_whatever_the_project_marked_it() {
        let judged = judged(&project(), true, &deploy(at("dev/deploy", true)), "Ada");
        assert_eq!(judged.goes, Goes::Asks);
    }

    #[test]
    fn the_strictest_place_that_matches_decides() {
        let mut both = vec![
            job("Shared", true, "shared/*", StepPass::Deploy),
            job("Payments", true, "shared/pay*", StepPass::Deploy),
        ];
        both[0].shown.level = Level::Yours;
        let judged = judged(&both, true, &deploy(at("shared/payments", false)), "Ada");
        assert_eq!(judged.goes, Goes::Asks);
        assert_eq!(
            judged.place.map(|place| place.shown.name),
            Some("Payments".into())
        );
    }

    #[test]
    fn publishes_and_production_writes_hold_whatever_is_set() {
        let targets = [
            Target::Package {
                registry: "npm".into(),
            },
            Target::Database {
                name: "main".into(),
            },
        ];
        for target in targets {
            let ship = Ship {
                kind: ShipKind::Publish,
                what: "dev".to_owned(),
                target: Some(target),
            };
            let judged = judged(&project(), true, &ship, "Ada");
            assert_eq!(judged.goes, Goes::Asks);
            assert!(judged.keep);
        }
    }
}
