//! The one table of whether an action that leaves this machine runs. What it touches is read
//! from the facts of what runs. A production place decides by its level. When anything in the
//! action says production, that wins over a place or a name that says otherwise. A place
//! that isn't production, or a target whose name says it isn't, lets it run, and what no fact
//! places runs and is listed as seen.

use crate::server::places::place::PlaceExt;
use sikemux_view::inbox::ShipKind;
use sikemux_view::places::{Level, Recognise};

use super::place::{id_of, new_place, says_not_production, says_production, Place};
use crate::ship::{Ship, Target, SSH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goes {
    Freely,
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
    pub why: Option<String>,
}

/// Where `ship` lands among `places`, as the person `confirmed` the list or not. `agent` is
/// whose action it is, by name, for a place the core finds in it.
pub fn judged(places: &[Place], confirmed: bool, ship: &Ship, agent: &str) -> Judged {
    judged_reading(places, confirmed, ship, agent, &ship.what, None)
}

/// [`judged`] with `addresses`, what `ship` writes to with each address in full. A browser's
/// write is shown and allowed without its address's query, which can still say what it changes.
/// `plain` is the repository `ship` changes when it is a push or merge through git with nothing
/// else to it.
pub fn judged_reading(
    places: &[Place],
    confirmed: bool,
    ship: &Ship,
    agent: &str,
    addresses: &str,
    plain: Option<&str>,
) -> Judged {
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
    let fact = production_fact(ship, addresses, found_in.clone())
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
        .max_by_key(|place| strictness(level_for(place, confirmed, ship, plain)));
    if let Some(place) = strictest {
        let from_fact =
            (fact_place.as_ref()).is_some_and(|(found, _)| found.shown.id == place.shown.id);
        let keep = from_fact && fact_place.as_ref().is_some_and(|(_, keep)| *keep);
        return by_level(place, level_for(place, confirmed, ship, plain), keep);
    }
    if let Some(place) = recognising.first() {
        return Judged {
            goes: Goes::Freely,
            place: Some((*place).clone()),
            keep: false,
            why: Some(format!("{} isn't production.", place.name())),
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

fn level_for(place: &Place, confirmed: bool, ship: &Ship, plain: Option<&str>) -> Level {
    place.level_for(confirmed, plain.filter(|_| routine(place, ship)))
}

/// Whether `ship`, a plain push or merge, is the change `place` sees again and again, which the
/// person may say to just be told of: work that deploys from the project's own branch, or a tag
/// of a kind that recurs there. A stable release never is.
pub fn routine(place: &Place, ship: &Ship) -> bool {
    place.recognises(ship)
        && match (ship.kind, &ship.target) {
            (ShipKind::Deploy, Some(Target::Vercel { production: true })) => true,
            (ShipKind::Deploy, Some(Target::Workflow { rerun: None, .. })) => true,
            (ShipKind::Publish, Some(Target::Tag { name })) => place.recurs && !name.is_empty(),
            _ => false,
        }
}

fn by_level(place: &Place, level: Level, keep: bool) -> Judged {
    let (goes, why) = match level {
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
        why: Some(why),
    }
}

/// A place a deploy tool's stack, stage or app names, which is production when its name says
/// so.
pub fn stage_place(tool: &str, name: &str, found_in: Option<String>) -> Place {
    let fact = Recognise::Stage {
        tool: tool.to_owned(),
        name: name.to_owned(),
    };
    new_place(
        &format!("{name} on {tool}"),
        says_production(name),
        vec![fact],
        found_in,
    )
}

/// The words of `ship` that say where it lands: the origin of a site it writes to, the host
/// it changes over SSH, and every word of any other command.
fn selecting_words(ship: &Ship) -> &str {
    match ship.target.as_ref().map(Target::bare) {
        Some(Target::Site { origin }) => origin,
        Some(Target::Stage { tool, name }) if tool == SSH => name,
        _ => &ship.what,
    }
}

fn says_production_anywhere(ship: &Ship) -> bool {
    says_production(selecting_words(ship))
}

/// Whether a workflow run reaches production: one that couldn't be read, one that names a
/// production environment, or whose file name or inputs say production.
pub fn workflow_asks(target: &Target) -> bool {
    let Target::Workflow {
        file,
        environments,
        inputs,
        unread,
        ..
    } = target
    else {
        return false;
    };
    *unread
        || environments.iter().any(|name| says_production(name))
        || std::iter::once(file)
            .chain(inputs)
            .any(|word| says_production(word))
}

/// The production place a fact of `ship` names by itself, whatever the project's list says:
/// a target its service marks as production, a write to a production database, a write as the
/// person to a GitHub repository, a workflow that reaches production, or a stack, stage, app,
/// profile, context or workspace whose name says production. The gate lets a write to the
/// project's own repositories through before it asks here.
fn production_fact(ship: &Ship, addresses: &str, found_in: Option<String>) -> Option<Place> {
    let place =
        |name: String, fact: Recognise| new_place(&name, true, vec![fact], found_in.clone());
    let target = ship.target.as_ref().map(Target::bare)?;
    match target {
        Target::Vercel { production: true } => Some(place(
            "Production on Vercel".to_owned(),
            Recognise::VercelTarget { production: true },
        )),
        Target::CloudflareZone { zone } => Some(place(
            format!("{zone} on Cloudflare"),
            Recognise::CloudflareZone { zone: zone.clone() },
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
        Target::AwsProfile { name } if says_production(name) => Some(place(
            format!("AWS profile {name}"),
            Recognise::AwsProfile { name: name.clone() },
        )),
        Target::KubeContext { name } if says_production(name) => Some(place(
            format!("Kubernetes context {name}"),
            Recognise::KubeContext { name: name.clone() },
        )),
        Target::TerraformWorkspace { name } if says_production(name) => Some(place(
            format!("Terraform workspace {name}"),
            Recognise::TerraformWorkspace { name: name.clone() },
        )),
        Target::Stage { tool, name } if tool == "Netlify" => Some(new_place(
            &format!("{name} on Netlify"),
            true,
            vec![Recognise::Stage {
                tool: tool.clone(),
                name: name.clone(),
            }],
            found_in,
        )),
        Target::Stage { tool, name }
            if tool == "Railway" && !name.is_empty() && !says_not_production(name) =>
        {
            Some(stage_place(tool, name, found_in))
        }
        Target::Stage { tool, name } if says_production(name) => {
            Some(stage_place(tool, name, found_in))
        }
        Target::Package { registry } if registry == super::GITHUB_RELEASES => Some(place(
            "Releases on GitHub".to_owned(),
            Recognise::Package {
                registry: registry.clone(),
            },
        )),
        Target::GitHub { repo, lands, .. } if !repo.is_empty() || lands.is_some() => {
            let name = match repo.is_empty() {
                true => "GitHub".to_owned(),
                false => format!("{repo} on GitHub"),
            };
            Some(new_place(&name, true, Vec::new(), found_in))
        }
        Target::Database { name } => Some(place(
            format!("The {name} database"),
            Recognise::Database { name: name.clone() },
        )),
        Target::Site { origin } if manages_domains(origin, addresses) => {
            let host = origin
                .split_once("://")
                .map_or(origin.as_str(), |(_, host)| host);
            Some(place(
                format!("Domains at {host}"),
                Recognise::Site {
                    origin: origin.clone(),
                },
            ))
        }
        Target::Workflow {
            file,
            environments,
            unread,
            ..
        } if workflow_asks(target) => {
            let name = match environments.iter().find(|name| says_production(name)) {
                Some(environment) => format!("{environment} on GitHub"),
                None if file.is_empty() => "A workflow on GitHub".to_owned(),
                None => format!("The {file} workflow on GitHub"),
            };
            let facts = (!unread && !file.is_empty())
                .then(|| Recognise::Command {
                    words: format!("gh workflow run {file}"),
                })
                .into_iter()
                .collect();
            Some(new_place(&name, true, facts, found_in))
        }
        Target::Rerun { of, .. } => Some(place(
            format!("The workflow of {of} on GitHub"),
            Recognise::Command {
                words: format!("gh run rerun {of}"),
            },
        )),
        _ => None,
    }
}

/// Whether a write to the site at `origin` changes where a domain points: one to a registrar's
/// or a DNS host's panel, or one of whose `addresses` has, past its host, a word that starts
/// with nameserver, DNS or domain. What the request sends is not read.
fn manages_domains(origin: &str, addresses: &str) -> bool {
    const PANELS: &[&str] = &[
        "godaddy.com",
        "namecheap.com",
        "dash.cloudflare.com",
        "porkbun.com",
        "name.com",
        "namesilo.com",
        "gandi.net",
        "dynadot.com",
        "hover.com",
        "domains.squarespace.com",
        "ionos.com",
        "networksolutions.com",
        "enom.com",
        "hostinger.com",
        "route53.console.aws.amazon.com",
        "domains.google.com",
    ];
    const WORDS: &[&str] = &["nameserver", "dns", "domain", "registrar", "whois"];
    let host = origin.split("://").nth(1).unwrap_or(origin);
    let host = host
        .split([':', '/'])
        .next()
        .unwrap_or(host)
        .to_ascii_lowercase();
    let at_panel = PANELS
        .iter()
        .any(|panel| host == *panel || host.ends_with(&format!(".{panel}")));
    let addresses = addresses.to_ascii_lowercase();
    let named = (addresses.split_whitespace())
        .map(|word| word.trim_matches(['\'', '"']))
        .filter_map(|word| {
            ["https://", "http://"]
                .iter()
                .find_map(|at| word.strip_prefix(at))
        })
        .filter_map(|address| address.strip_prefix(host.as_str()))
        .filter(|past_host| past_host.is_empty() || past_host.starts_with(['/', ':', '?', '#']))
        .flat_map(|past_host| past_host.split(|letter: char| !letter.is_ascii_alphanumeric()))
        .any(|part| WORDS.iter().any(|word| part.starts_with(word)));
    at_panel || named
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
        Some(Target::CloudflareZone { .. }) => "Cloudflare write".to_owned(),
        Some(Target::GitHub { .. }) => "GitHub write".to_owned(),
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
        Some(Target::RundeckJob { path, .. }) => format!("Rundeck job {path}"),
        Some(Target::AwsProfile { name }) => format!("AWS profile {name}"),
        Some(Target::KubeContext { name }) => format!("Kubernetes context {name}"),
        Some(Target::TerraformWorkspace { name }) => format!("Terraform workspace {name}"),
        Some(Target::Workflow { file, .. }) => format!("the {file} workflow"),
        Some(Target::Stage { tool, name }) => format!("{name} on {tool}"),
        Some(Target::CloudflareZone { zone }) => format!("{zone} on Cloudflare"),
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
        Some(Target::CloudflareZone { zone }) => Recognise::CloudflareZone { zone: zone.clone() },
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

    /// An agent changed a live domain's nameservers in its registrar's panel with the desk
    /// browser on 2026-11-23; nothing placed the site, so the write ran unheld and took the
    /// live site down.
    #[test]
    fn a_registrar_panel_write_holds_and_ordinary_sites_do_not() {
        let submit = |what: &str, origin: &str| Ship {
            kind: ShipKind::Publish,
            what: what.to_owned(),
            target: Some(Target::Site {
                origin: origin.into(),
            }),
        };
        let nameservers = submit(
            "POST https://my.registrar.example/clientarea.php",
            "https://my.registrar.example",
        );
        let judged = judged_reading(
            &[],
            true,
            &nameservers,
            "Otter",
            "POST https://my.registrar.example/clientarea.php?action=domaindetails",
            None,
        );
        assert_eq!(judged.goes, Goes::Asks);
        assert!(judged.keep);
        let own_panel = submit(
            "POST https://hosting.example.com/clientarea.php",
            "https://hosting.example.com",
        );
        let judged = judged_reading(
            &[],
            true,
            &own_panel,
            "Otter",
            "POST https://hosting.example.com/clientarea.php?action=domaindns",
            None,
        );
        assert_eq!(judged.goes, Goes::Asks);
        let search = submit(
            "GET https://www.google.com/search",
            "https://www.google.com",
        );
        assert_eq!(judged_on(&search), Goes::Unplaced);
        let chat = submit(
            "curl -X POST https://hooks.chat.example/services/T0 -d {\"text\":\"fixed the domain model\"}",
            "https://hooks.chat.example",
        );
        assert_eq!(judged_on(&chat), Goes::Unplaced);
    }

    /// Cloudflare's dashboard saves a DNS record from its own script, which the browser holds
    /// and puts to the gate as the request it would have sent.
    #[test]
    fn a_script_save_on_a_registrar_panel_holds() {
        let save = Ship {
            kind: ShipKind::Publish,
            what: "PUT https://dash.cloudflare.com/api/v4/zones/023e105f/dns_records/372e6795"
                .to_owned(),
            target: Some(Target::Site {
                origin: "https://dash.cloudflare.com".into(),
            }),
        };
        let judged = judged(&[], true, &save, "Otter");
        assert_eq!(judged.goes, Goes::Asks);
        assert!(judged.keep);
    }

    fn judged_on(ship: &Ship) -> Goes {
        judged(&[], true, ship, "Otter").goes
    }

    #[test]
    fn production_writes_hold_whatever_is_set() {
        let ship = Ship {
            kind: ShipKind::ProductionWrite,
            what: "dev".to_owned(),
            target: Some(Target::Database {
                name: "main".into(),
            }),
        };
        let judged = judged(&project(), true, &ship, "Ada");
        assert_eq!(judged.goes, Goes::Asks);
        assert!(judged.keep);
    }

    #[test]
    fn cloudflare_zone_writes_hold_by_default_and_respect_per_zone_level() {
        let write = |zone: &str| Ship {
            kind: ShipKind::ProductionWrite,
            what: format!("cloudflare_dns_upsert {{\"zone\":\"{zone}\"}}"),
            target: Some(Target::CloudflareZone {
                zone: zone.to_owned(),
            }),
        };
        let prod = judged(&[], true, &write("sikemux.com"), "Ada");
        assert_eq!(prod.goes, Goes::Asks);
        assert!(prod.keep);
        assert_eq!(
            prod.place.as_ref().map(|place| place.shown.name.as_str()),
            Some("sikemux.com on Cloudflare")
        );
        assert_eq!(
            prod.place.as_ref().map(|place| place.shown.kind),
            Some(sikemux_view::places::PlaceKind::Cloudflare)
        );

        let mut relaxed = new_place(
            "sandbox.example on Cloudflare",
            true,
            vec![Recognise::CloudflareZone {
                zone: "sandbox.example".into(),
            }],
            None,
        );
        relaxed.shown.level = Level::Yours;
        assert_eq!(
            judged(&[relaxed.clone()], true, &write("sandbox.example"), "Ada").goes,
            Goes::Freely
        );
        assert_eq!(
            judged(&[relaxed], true, &write("sikemux.com"), "Ada").goes,
            Goes::Asks
        );
    }
}
