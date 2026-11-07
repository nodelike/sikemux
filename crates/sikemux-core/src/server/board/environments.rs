//! A project's environments: what each is called, how the core recognises a
//! deploy to it from hard facts, and who passes it. They give the project's
//! slices a step each, and the one table here decides whether a deploy goes or
//! holds. What it can't recognise counts as production. They live in the
//! board's settings because agents can write to the repository, and can only
//! propose them.

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_view::inbox::ShipKind;
use sikemux_view::slice::Gate;

use super::slice::{Later, Seen};
use super::{Board, By, CardStatus, Changes, Notice, State};
use crate::server::Core;
use crate::ship::{leads, Ship, Target};

const MAX_ENVIRONMENTS: usize = 8;
const MAX_RECOGNISED: usize = 8;
const MAX_NAME_CHARS: usize = 32;
const MAX_VALUE_CHARS: usize = 200;
const MAX_WHY_CHARS: usize = 500;
/// Words in a name that say it is production, which never goes unasked.
const PRODUCTION_WORDS: [&str; 4] = ["prod", "production", "prd", "live"];
/// Words in a Rundeck folder's name that say it is clearly not production.
const NOT_PRODUCTION_WORDS: [&str; 13] = [
    "dev",
    "develop",
    "development",
    "staging",
    "stage",
    "stg",
    "qa",
    "uat",
    "test",
    "testing",
    "sandbox",
    "preview",
    "demo",
];

/// Who a deploy to an environment waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Passes {
    /// Nobody: it just goes.
    Nobody,
    /// The project's check passes, then it goes.
    Check,
    /// It waits for the person to allow it once or pass its gate.
    Person,
    /// As a person, and no setting loosens it.
    Production,
}

/// A hard fact that says a deploy lands in an environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// One environment of a project, in the order its ladder reaches them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    /// What its step is called and named by, one per project.
    pub name: String,
    pub passes: Passes,
    pub recognise: Vec<Recognise>,
    /// An address that answers once a deploy there is healthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    /// It only lets deploys go, and gives the project's slices no step.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_step: bool,
}

/// Environments an agent proposed, for the person to accept or turn down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentsProposal {
    pub environments: Vec<Environment>,
    pub why: String,
    /// The agent that proposed them, by name.
    pub by: String,
    pub at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardProposalParams {
    pub project: String,
    pub accept: bool,
}

impl Environment {
    pub(crate) fn passes_words(&self) -> String {
        match self.passes {
            Passes::Nobody => format!("{} goes freely, marked safe", self.name),
            Passes::Check => format!("{} goes once the project's check passes", self.name),
            Passes::Person => format!("{} waits for a person", self.name),
            Passes::Production => format!("{} always waits for a person", self.name),
        }
    }

    /// Who passes it, short enough to sit under its step on a ladder.
    pub(crate) fn step_words(&self) -> &'static str {
        match self.passes {
            Passes::Nobody => "Goes freely",
            Passes::Check => "Once the check passes",
            Passes::Person => "A person",
            Passes::Production => "Always a person",
        }
    }

    fn recognises(&self, ship: &Ship) -> bool {
        self.recognise
            .iter()
            .any(|fact| match (fact, &ship.target) {
                (Recognise::RundeckJob { path }, Some(Target::RundeckJob { path: job, .. })) => {
                    matches_path(path, job)
                }
                (
                    Recognise::VercelTarget { production },
                    Some(Target::Vercel { production: at }),
                ) => production == at,
                (Recognise::AwsProfile { name }, Some(Target::AwsProfile { name: at }))
                | (Recognise::KubeContext { name }, Some(Target::KubeContext { name: at }))
                | (
                    Recognise::TerraformWorkspace { name },
                    Some(Target::TerraformWorkspace { name: at }),
                ) => name == at,
                (Recognise::Command { words }, _) => {
                    let said: Vec<String> =
                        ship.what.split_whitespace().map(str::to_owned).collect();
                    leads(words, &said)
                }
                _ => false,
            })
    }

    fn reads_as_production(&self) -> bool {
        says_production(&self.name)
            || self.recognise.iter().any(|fact| match fact {
                Recognise::VercelTarget { production } => *production,
                Recognise::RundeckJob { path } => {
                    (path.split('/').next()).is_some_and(says_production)
                }
                Recognise::AwsProfile { name }
                | Recognise::KubeContext { name }
                | Recognise::TerraformWorkspace { name } => says_production(name),
                Recognise::Command { .. } => false,
            })
    }
}

/// Whether `pattern`, where `*` stands for anything, covers all of `path`, in any case.
fn matches_path(pattern: &str, path: &str) -> bool {
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

fn says_production(name: &str) -> bool {
    has_word(name, &PRODUCTION_WORDS)
}

fn has_word(name: &str, words: &[&str]) -> bool {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| words.iter().any(|said| word.eq_ignore_ascii_case(said)))
}

/// What the person can always allow from a held deploy, in place of allowing it once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Always {
    /// What it covers, as the person reads it, such as `dev/*`.
    pub covers: String,
    pub fact: Recognise,
    /// The environment it joins or starts.
    pub name: String,
    /// The folder every project under which it holds for; none for the project alone.
    pub folder: Option<String>,
}

fn reads_not_production(name: &str) -> bool {
    !says_production(name) && has_word(name, &NOT_PRODUCTION_WORDS)
}

fn titled(word: &str) -> String {
    let mut letters = word.chars();
    let first = letters.next().map(|letter| letter.to_ascii_uppercase());
    first.into_iter().chain(letters).collect()
}

/// The narrowest fact that covers `ship` and reads as no production, with what it covers and
/// the environment it would be: a Rundeck folder such as `dev/*`, Vercel's previews, an AWS
/// profile, kube context or Terraform workspace, or one of the project's own `ships` commands.
fn freeing(ship: &Ship, ships: &[String]) -> Option<(String, Recognise, String)> {
    match &ship.target {
        Some(Target::RundeckJob {
            path,
            production: false,
        }) => {
            let (folder, _) = path.split_once('/')?;
            let pattern = format!("{folder}/*");
            (!says_production(path) && has_word(folder, &NOT_PRODUCTION_WORDS)).then(|| {
                let fact = Recognise::RundeckJob {
                    path: pattern.clone(),
                };
                (pattern, fact, titled(folder))
            })
        }
        Some(Target::Vercel { production: false }) => Some((
            "Vercel previews".to_owned(),
            Recognise::VercelTarget { production: false },
            "Preview".to_owned(),
        )),
        Some(Target::AwsProfile { name }) if reads_not_production(name) => Some((
            format!("AWS {name}"),
            Recognise::AwsProfile { name: name.clone() },
            name.clone(),
        )),
        Some(Target::KubeContext { name }) if reads_not_production(name) => Some((
            format!("context {name}"),
            Recognise::KubeContext { name: name.clone() },
            name.clone(),
        )),
        Some(Target::TerraformWorkspace { name }) if reads_not_production(name) => Some((
            format!("workspace {name}"),
            Recognise::TerraformWorkspace { name: name.clone() },
            name.clone(),
        )),
        None => {
            let words: Vec<String> = ship.what.split_whitespace().map(str::to_owned).collect();
            let listed = ships.iter().any(|listed| leads(listed, &words));
            let word = (ship.what.split(|c: char| !c.is_ascii_alphanumeric()))
                .find(|word| has_word(word, &NOT_PRODUCTION_WORDS))?;
            (listed && !says_production(&ship.what)).then(|| {
                let fact = Recognise::Command {
                    words: ship.what.clone(),
                };
                (format!("`{}`", ship.what), fact, titled(word))
            })
        }
        _ => None,
    }
}

/// What a held deploy could always be allowed by, where no environment of any of `levels`
/// has a say on it yet, as it would join `into`. Only what reads as no production offers it.
/// `ships` are the project's own commands that deploy.
pub(crate) fn always_offer(
    levels: &[&[Environment]],
    into: &[Environment],
    ship: &Ship,
    ships: &[String],
) -> Option<Always> {
    let unclaimed =
        (levels.iter()).all(|level| !level.iter().any(|environment| environment.recognises(ship)));
    if ship.kind != ShipKind::Deploy || !unclaimed {
        return None;
    }
    let (covers, fact, name) = freeing(ship, ships)?;
    let always = Always {
        covers,
        fact,
        name,
        folder: None,
    };
    allowing(into.to_vec(), &always).is_ok().then_some(always)
}

/// `environments` with what `always` covers going without asking, in an environment that
/// takes no step, so the project's slices climb what they climbed.
pub(super) fn allowing(
    mut environments: Vec<Environment>,
    always: &Always,
) -> Result<Vec<Environment>, String> {
    let named = (environments.iter_mut())
        .find(|environment| environment.name.eq_ignore_ascii_case(&always.name));
    match named {
        Some(environment) if environment.passes == Passes::Nobody && environment.no_step => {
            if !environment.recognise.contains(&always.fact) {
                environment.recognise.push(always.fact.clone());
            }
        }
        Some(environment) => {
            return Err(format!(
                "{} is already an environment that waits, so change it under Environments",
                environment.name
            ))
        }
        None => environments.push(Environment {
            name: always.name.clone(),
            passes: Passes::Nobody,
            recognise: vec![always.fact.clone()],
            health: None,
            no_step: true,
        }),
    }
    checked(environments)
}

/// What the person or an agent gave as a project's environments, tidied and
/// held to what no setting may do: a name or fact that reads as production
/// never goes without asking.
pub(super) fn checked(given: Vec<Environment>) -> Result<Vec<Environment>, String> {
    if given.len() > MAX_ENVIRONMENTS {
        return Err(format!(
            "a project has at most {MAX_ENVIRONMENTS} environments"
        ));
    }
    let mut kept: Vec<Environment> = Vec::with_capacity(given.len());
    for mut environment in given {
        environment.name = environment.name.trim().to_owned();
        let name = &environment.name;
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS || name.contains('\n') {
            return Err(format!(
                "an environment's name is one line of at most {MAX_NAME_CHARS} characters"
            ));
        }
        if RESERVED.iter().any(|word| name.eq_ignore_ascii_case(word)) {
            return Err(format!(
                "{name} is a gate every slice can have, so name it something else"
            ));
        }
        if (kept.iter()).any(|other| other.name.eq_ignore_ascii_case(name)) {
            return Err(format!("two environments are called {name}"));
        }
        if environment.recognise.is_empty() || environment.recognise.len() > MAX_RECOGNISED {
            return Err(format!(
                "{name} needs from 1 to {MAX_RECOGNISED} facts to recognise it by"
            ));
        }
        for fact in &mut environment.recognise {
            let (Recognise::RundeckJob { path: value }
            | Recognise::AwsProfile { name: value }
            | Recognise::KubeContext { name: value }
            | Recognise::TerraformWorkspace { name: value }
            | Recognise::Command { words: value }) = fact
            else {
                continue;
            };
            *value = value.trim().to_owned();
            if value.is_empty() || value.chars().count() > MAX_VALUE_CHARS {
                return Err(format!(
                    "{name}: what recognises it is at most {MAX_VALUE_CHARS} characters, and not empty"
                ));
            }
        }
        environment.health = (environment.health.take())
            .map(|health| health.trim().to_owned())
            .filter(|health| !health.is_empty());
        if let Some(health) = &environment.health {
            if !health.starts_with("http://") && !health.starts_with("https://") {
                return Err(format!(
                    "{name}: its health check is an http or https address"
                ));
            }
        }
        if matches!(environment.passes, Passes::Nobody | Passes::Check)
            && environment.reads_as_production()
        {
            return Err(format!(
                "{name} reads as production, which always waits for a person"
            ));
        }
        kept.push(environment);
    }
    Ok(kept)
}

/// What a step may not be called, in any case: the gates and the keys they go by.
const RESERVED: [&str; 4] = ["Built", "Looks right", "looksRight", "Landed"];

/// Whether a deploy goes now, as the table in [`judged`] says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Goes {
    Freely,
    OnceChecked,
    Holds,
}

/// Where an action lands and whether it waits, as the core judged it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Judged {
    pub environment: Option<String>,
    pub goes: Goes,
    /// Why, for the agent and the person; none in a project with no environments.
    pub why: Option<String>,
}

/// The one table of whether an action goes: production and what can't be
/// recognised always hold, and only an environment marked safe goes unasked.
/// Pushes and merges are `landing`'s, and publishes always hold.
pub(crate) fn judged(environments: &[Environment], ship: &Ship) -> Judged {
    let holds = |environment: Option<&Environment>, why: Option<String>| Judged {
        environment: environment.map(|environment| environment.name.clone()),
        goes: Goes::Holds,
        why,
    };
    if ship.kind != ShipKind::Deploy || environments.is_empty() {
        return holds(None, None);
    }
    let strictest = (environments.iter())
        .filter(|environment| environment.recognises(ship))
        .max_by_key(|environment| match environment.passes {
            Passes::Nobody => 0,
            Passes::Check => 1,
            Passes::Person => 2,
            Passes::Production => 3,
        });
    let Some(environment) = strictest else {
        return holds(
            None,
            Some("Sikemux doesn't recognise where this lands, so it counts as production.".into()),
        );
    };
    let words = environment.passes_words();
    let lands_in_production = match &ship.target {
        Some(target) => {
            target.production()
                || matches!(target, Target::RundeckJob { path, .. } if says_production(path))
        }
        None => says_production(&ship.what),
    };
    if lands_in_production {
        let why = format!(
            "{} lands in production, which always waits for a person.",
            environment.name
        );
        return holds(Some(environment), Some(why));
    }
    let goes = match environment.passes {
        Passes::Nobody => Goes::Freely,
        Passes::Check => Goes::OnceChecked,
        Passes::Person | Passes::Production => Goes::Holds,
    };
    Judged {
        environment: Some(environment.name.clone()),
        goes,
        why: Some(format!("{words}.")),
    }
}

/// [`judged`] for a project that inherits environments: `levels` are its own, then each
/// folder's above it, nearest first. The nearest level that recognises the action decides.
pub(crate) fn judged_within(levels: &[&[Environment]], ship: &Ship) -> Judged {
    let recognising =
        (levels.iter()).find(|level| level.iter().any(|environment| environment.recognises(ship)));
    let deciding = recognising.or_else(|| levels.iter().find(|level| !level.is_empty()));
    judged(deciding.copied().unwrap_or_default(), ship)
}

/// `board_decide` with environments: keeps what an agent proposed for the person to
/// answer, in place of any it proposed before. It changes nothing else.
pub(super) fn propose(
    board: &mut Board,
    environments: Vec<Environment>,
    why: &str,
    by: String,
) -> Result<(), String> {
    let why = why.trim();
    if why.is_empty() || why.chars().count() > MAX_WHY_CHARS {
        return Err(format!(
            "why says in at most {MAX_WHY_CHARS} characters why the project should have these"
        ));
    }
    let environments = checked(environments)?;
    if environments.is_empty() {
        return Err("name at least one environment".into());
    }
    board.environments_proposal = Some(EnvironmentsProposal {
        environments,
        why: why.to_owned(),
        by,
        at: epoch_ms(),
    });
    Ok(())
}

/// The ladder a project's environments and landing give a slice. Where the
/// person lands slices, Landed stays first, as nothing counts as landed before they say.
pub(super) fn ladder_steps(person_lands: bool, environments: &[Environment]) -> Vec<Step> {
    let mut steps = vec![Step::Gate(Gate::Built)];
    if person_lands {
        steps.push(Step::Gate(Gate::Landed));
    }
    steps.extend(
        environments
            .iter()
            .filter(|environment| !environment.no_step)
            .map(|environment| Step::Environment(environment.name.clone())),
    );
    steps
}

/// A step of a slice's ladder: a gate every project knows, or one of its
/// environments, by name. In the board's file a gate keeps its old key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Step {
    Gate(Gate),
    Environment(String),
}

impl Step {
    /// What a released client reads for it, which knows only the three gates:
    /// an environment comes after what lands, so it reads as Landed.
    pub(crate) fn gate(&self) -> Gate {
        match self {
            Self::Gate(gate) => *gate,
            Self::Environment(_) => Gate::Landed,
        }
    }

    pub(crate) fn environment(&self) -> Option<&str> {
        match self {
            Self::Gate(_) => None,
            Self::Environment(name) => Some(name),
        }
    }

    /// The step a client's `gate` and `environment` name.
    pub(crate) fn named(gate: Gate, environment: Option<&str>) -> Self {
        match environment {
            Some(name) => Self::Environment(name.to_owned()),
            None => Self::Gate(gate),
        }
    }
}

/// What is left to do once the board's lock is let go of, after ladders moved.
#[derive(Default)]
pub(super) struct Settled {
    pub notices: Vec<Notice>,
    pub later: Later,
}

pub(super) fn answer_proposal(
    core: &Core,
    state: &mut State,
    project: &str,
    accept: bool,
) -> Result<Settled, String> {
    let board = state
        .boards
        .get_mut(project)
        .ok_or("this project's board has no proposal")?;
    let proposal = board
        .environments_proposal
        .take()
        .ok_or("that proposal is gone")?;
    if !accept {
        return Ok(Settled::default());
    }
    board.settings.environments = checked(proposal.environments)?;
    super::slice::settle_ladders(board);
    Ok(settle_cards(core, state, project))
}

/// Moves on each open slice of `project` whose ladder just changed, as a step
/// that went may leave it with nothing to wait at.
pub(super) fn settle_cards(core: &Core, state: &mut State, project: &str) -> Settled {
    let open: Vec<u64> = (state.boards.get(project).into_iter())
        .flat_map(|board| board.cards.iter())
        .filter(|card| card.slice.is_some() && card.status != CardStatus::Done)
        .map(|card| card.id)
        .collect();
    let mut settled = Settled::default();
    for id in open {
        let moved = super::change_in(
            core,
            state,
            project,
            &By::Auto("Board"),
            id,
            Changes::default(),
            &Seen::unread(),
        );
        match moved {
            Ok((_, notices, later)) => {
                settled.notices.extend(notices);
                settled.later = std::mem::take(&mut settled.later).and(later);
            }
            Err(error) => {
                eprintln!("sikemux core: #{id} did not move with its new ladder: {error}")
            }
        }
    }
    settled
}

#[cfg(test)]
mod tests {
    use sikemux_process::sync::Locked;

    use super::*;

    fn environment(name: &str, passes: Passes, recognise: Vec<Recognise>) -> Environment {
        Environment {
            name: name.to_owned(),
            passes,
            recognise,
            health: None,
            no_step: false,
        }
    }

    fn job(path: &str) -> Recognise {
        Recognise::RundeckJob {
            path: path.to_owned(),
        }
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

    fn project() -> Vec<Environment> {
        vec![
            environment("Dev", Passes::Nobody, vec![job("dev/*")]),
            environment("Staging", Passes::Check, vec![job("staging/*")]),
            environment("Production", Passes::Production, vec![job("prod/*")]),
        ]
    }

    #[test]
    fn production_holds() {
        let judged = judged(&project(), &deploy(at("prod/deploy", false)));
        assert_eq!(judged.goes, Goes::Holds);
        assert_eq!(judged.environment.as_deref(), Some("Production"));
    }

    #[test]
    fn what_cannot_be_recognised_holds_as_production() {
        for ship in [deploy(None), deploy(at("qa/deploy", false))] {
            let judged = judged(&project(), &ship);
            assert_eq!(judged.goes, Goes::Holds);
            assert_eq!(judged.environment, None);
            assert!(judged.why.is_some_and(|why| why.contains("production")));
        }
    }

    #[test]
    fn a_fact_that_says_production_holds_whatever_the_project_marked_it() {
        let judged = judged(&project(), &deploy(at("dev/deploy", true)));
        assert_eq!(judged.goes, Goes::Holds);
    }

    #[test]
    fn the_strictest_environment_that_matches_decides() {
        let both = vec![
            environment("Dev", Passes::Nobody, vec![job("shared/*")]),
            environment("Payments", Passes::Person, vec![job("shared/pay*")]),
        ];
        let judged = judged(&both, &deploy(at("shared/payments", false)));
        assert_eq!(judged.goes, Goes::Holds);
        assert_eq!(judged.environment.as_deref(), Some("Payments"));
    }

    #[test]
    fn publishes_and_production_writes_hold_whatever_is_set() {
        for kind in [ShipKind::Publish, ShipKind::ProductionWrite] {
            let ship = Ship {
                kind,
                what: "dev".to_owned(),
                target: at("dev/deploy", false),
            };
            assert_eq!(judged(&project(), &ship).goes, Goes::Holds);
        }
    }

    #[test]
    fn nothing_that_reads_as_production_may_be_loosened() {
        for passes in [Passes::Nobody, Passes::Check] {
            let refused = [
                environment("Production", passes, vec![job("a/*")]),
                environment("prod-eu", passes, vec![job("a/*")]),
                environment("Safe", passes, vec![job("prod/*")]),
                environment(
                    "Safe",
                    passes,
                    vec![Recognise::VercelTarget { production: true }],
                ),
                environment(
                    "Safe",
                    passes,
                    vec![Recognise::AwsProfile {
                        name: "acme-prod".to_owned(),
                    }],
                ),
            ];
            for one in refused {
                assert!(checked(vec![one.clone()]).is_err(), "{one:?}");
            }
        }
        assert!(checked(vec![environment(
            "Production",
            Passes::Production,
            vec![job("a/*")]
        )])
        .is_ok());
    }

    #[test]
    fn a_path_pattern_covers_the_whole_path() {
        assert!(matches_path("dev/*", "dev/backend/deploy"));
        assert!(matches_path("DEV/*", "dev/x"));
        assert!(matches_path("*/deploy", "dev/deploy"));
        assert!(!matches_path("dev/*", "develop/x"));
        assert!(!matches_path("dev/x", "dev/x/y"));
    }

    #[test]
    fn a_name_that_is_a_gate_is_refused() {
        for name in ["Built", "landed", "LOOKS RIGHT", "looksright"] {
            assert!(
                checked(vec![environment(name, Passes::Person, vec![job("a/*")])]).is_err(),
                "{name}"
            );
        }
    }

    const PROJECT: &str = "/tmp/sikemux-environments-test";

    fn core_set(environments: Vec<Environment>) -> std::sync::Arc<Core> {
        let core = Core::new(Default::default(), None).unwrap();
        let mut state = core.boards.state.locked();
        let board = state
            .boards
            .entry(PROJECT.to_owned())
            .or_insert_with(|| Board::empty(PROJECT));
        board.settings.environments = environments;
        drop(state);
        core
    }

    fn try_ship(core: &Core, target: Option<Target>) -> Result<bool, String> {
        let ship = Ship {
            kind: ShipKind::Deploy,
            what: format!("rundeck_run {target:?}"),
            target,
        };
        crate::server::ship::check(core, "agent", ship, std::path::Path::new(PROJECT))
            .map(|deploy| deploy.is_some())
    }

    /// `rundeck_run` on a job under `dev/backend/` waited for Allow once just as production would.
    #[test]
    fn a_dev_job_is_not_held_once_dev_is_marked_safe() {
        let core = core_set(project());
        assert_eq!(
            try_ship(&core, at("dev/backend/deploy-api", false)),
            Ok(true)
        );
        assert!(core.ship.waiting().is_empty());
    }

    #[test]
    fn a_staging_job_waits_on_the_check_and_asks_nobody() {
        let core = core_set(project());
        let said = try_ship(&core, at("staging/deploy", false)).unwrap_err();
        assert!(said.contains("once the project's check passes"), "{said}");
        assert!(core.ship.waiting().is_empty());
    }

    #[test]
    fn production_and_what_is_unrecognised_hold_for_the_person() {
        let core = core_set(project());
        assert!(try_ship(&core, at("prod/deploy", false)).is_err());
        assert!(try_ship(&core, at("qa/deploy", false)).is_err());
        let held: Vec<Option<String>> = (core.ship.waiting().into_iter())
            .map(|waiting| waiting.hold.environment)
            .collect();
        assert_eq!(held, [Some("Production".to_owned()), None]);
    }

    #[test]
    fn with_nothing_set_a_dev_job_holds_as_it_always_did() {
        let core = core_set(Vec::new());
        assert!(try_ship(&core, at("dev/backend/deploy-api", false)).is_err());
        let waiting = core.ship.waiting();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].hold.environment, None);
        assert_eq!(waiting[0].hold.why, None);
    }

    /// 47 `rundeck_run` holds on `dev/backend/*` in a project with no environments since 2026-11-03,
    /// one after the person said "deploy on dev, I'm out", each answerable only by Allow once.
    #[test]
    fn a_dev_folder_can_be_let_go_from_its_hold_and_production_cannot() {
        let core = core_set(Vec::new());
        let dev = at("dev/backend/deploy-api", false);
        assert!(try_ship(&core, dev.clone()).is_err());
        let waiting = core.ship.waiting();
        assert_eq!(waiting[0].hold.let_go.as_deref(), Some("dev/*"));
        assert_eq!(waiting[0].hold.let_go_in, None);
        let always = crate::server::board::always_offer(&core, PROJECT, &deploy(dev.clone()));
        crate::server::board::allow_always(&core, PROJECT, &always.unwrap()).unwrap();

        assert_eq!(try_ship(&core, dev), Ok(true));
        assert_eq!(
            try_ship(&core, at("dev/backend/reprocess", false)),
            Ok(true)
        );
        assert_eq!(
            try_ship(&core, at("dev/backend/product-api", false)),
            Ok(true)
        );
        let state = core.boards.state.locked();
        let board = state.boards.get(PROJECT).unwrap();
        let ladder = super::super::slice::ladder_of(&board.settings, false);
        assert!(ladder.iter().all(|step| step.environment().is_none()));
        drop(state);

        for still_waits in [
            at("dev/backend/promote-to-prod", false),
            at("dev/backend/deploy-api", true),
            at("prod/deploy", false),
            at("backend/deploy-api", false),
            None,
        ] {
            assert!(
                try_ship(&core, still_waits.clone()).is_err(),
                "{still_waits:?}"
            );
            let offered = always_offer(&[], &[], &deploy(still_waits), &[]);
            assert_eq!(offered, None);
        }
    }

    /// An agent's proposal changes nothing until the person accepts it.
    #[test]
    fn a_proposal_changes_no_setting() {
        let core = core_set(Vec::new());
        let mut state = core.boards.state.locked();
        let board = state.boards.get_mut(PROJECT).unwrap();
        propose(
            board,
            project(),
            "We deploy three places.",
            "Otter".to_owned(),
        )
        .unwrap();
        assert!(board.settings.environments.is_empty());
        assert!(board.environments_proposal.is_some());
        answer_proposal(&core, &mut state, PROJECT, false).unwrap();
        let board = state.boards.get_mut(PROJECT).unwrap();
        assert!(board.settings.environments.is_empty());
        assert!(board.environments_proposal.is_none());
    }

    #[test]
    fn a_proposal_cannot_loosen_production_either() {
        let mut board = Board::empty(PROJECT);
        let loosened = vec![environment(
            "Production",
            Passes::Nobody,
            vec![job("prod/*")],
        )];
        assert!(propose(&mut board, loosened, "Faster.", "Otter".to_owned()).is_err());
        assert!(board.environments_proposal.is_none());
    }
}
