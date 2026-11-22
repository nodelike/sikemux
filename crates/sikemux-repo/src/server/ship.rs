//! Actions agents take that leave this machine, held for the person. An agent
//! pushing, merging, deploying, publishing, writing to production, or writing to
//! a website or a connector is stopped
//! there, and the person is asked in the inbox. Allowing a Sikemux tool call runs
//! it and sends the agent what it answered; allowing a command lets exactly that
//! command run once, until the end of the turn in which the agent hears of it.
//! Where the person lets agents land work, pushes and merges go through. A
//! deploy to an environment the project marked safe goes without asking, and
//! one that goes is followed to its end and kept as evidence on the slice.

use crate::server::agent_now::AgentNowCore;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::ship::{HoldParams, WebWriteParams};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_plugin_api::vault::{as_caller, Caller};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::inbox::{ShipHold, ShipKind};
use sikemux_view::slice::Verdict;

use super::person_calls::Raise;
use super::places::push::{self, Reach};
use super::places::{self, work, Goes, Judged};
use super::plugged::Provided;
use super::{person_calls, plugged, Core, CoreResult};
use crate::cli::protocol::HarnessRequest;
use crate::git::Git;
use crate::ship::{Looks, Ship, Target};
use sikemux_calls::board::Landing;
use sikemux_view::calls::{
    CallAsker, CallKind, CallOption, CallOutcome, CallPlace, CallResult, CallState, PersonCall,
};
use sikemux_view::places::Recognise;

/// More than an agent waits on at once; past this its oldest hold goes.
const MAX_HOLDS_PER_AGENT: usize = 16;
/// The person's answers remembered, across every agent, while the core runs.
const MAX_ANSWERED: usize = 256;
const DATABASE: &str = "sikemux.database";
const RUNDECK: &str = "sikemux.rundeck";
const VERCEL: &str = "sikemux.vercel";
/// Deploys let go whose end the core is still to hear of.
const MAX_GOING: usize = 64;
/// How long after the hook lets a command go its shim may still check that same run.
const SAME_RUN_MS: u64 = 60_000;
/// How long a deploy the shim follows may go unheard before its end counts as unseen.
const GOING_AT_MOST: Duration = Duration::from_secs(60 * 60);
/// How often a long deploy is looked at, and how long a look can take, before
/// the core stops following it and says it did not see the end.
const FOLLOW_LOOKS: usize = 60;
const FOLLOW_EVERY: Duration = Duration::from_secs(15);
const HEALTH_TRIES: usize = 6;
const HEALTH_EVERY: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Ships {
    state: Mutex<State>,
    /// When this core started: a production yes given before then passes nothing.
    born: u64,
}

impl Default for Ships {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            born: epoch_ms(),
        }
    }
}

#[derive(Default)]
struct State {
    next: u64,
    holds: Vec<Hold>,
    allowed: Vec<Allowed>,
    answered: VecDeque<Answered>,
    going: VecDeque<(u64, Deploy)>,
    /// Production yeses already used, by call: one that covers an action once, or a deploy's
    /// whose turn ended.
    used: HashSet<String>,
    /// When a yes for one action was taken by a check that doesn't see the action end, such
    /// as the hook before a shell command, by call. The shim that then runs it may take it too.
    unfollowed: HashMap<String, u64>,
    /// A deploy's yes each agent heard, which covers that work's deploy until the end of the
    /// turn it heard it in: the agent, the call, and whether that turn began.
    covering: Vec<(String, String, bool)>,
    /// When each agent last heard from the person in its own chat or terminal, after which a
    /// no it was given may be asked again.
    spoke: HashMap<String, u64>,
    /// The settled call each deploy at a place at "tell me" was told by, by its key.
    told: HashMap<String, String>,
}

/// A change at a place at "tell me", told once its deploy ends.
#[derive(Clone)]
pub struct Telling {
    key: String,
    raise: Raise,
}

/// A deploy let go toward one of the project's environments.
#[derive(Clone)]
pub struct Deploy {
    agent_id: String,
    project: String,
    environment: String,
    /// The commit its checkout was at as it went, which is what it deployed.
    head: Option<String>,
    /// The call whose yes let it go, which hears how it ended.
    call: Option<String>,
    /// The person is told of it once it ends.
    telling: Option<Telling>,
}

#[derive(Clone)]
pub struct Answered {
    pub agent_id: String,
    pub what: String,
    pub allowed: bool,
    pub at: u64,
}

/// An agent's action waiting on the person.
struct Hold {
    /// What clients name it by, which can't be guessed from another hold's.
    token: String,
    agent_id: String,
    project: String,
    ship: Ship,
    cwd: PathBuf,
    at: u64,
    /// The environment it lands in and why it waits, in a project with environments.
    environment: Option<String>,
    why: Option<String>,
    /// The Sikemux tool call held, which the core runs itself once the person allows it.
    call: Option<ToolCall>,
    /// The deploy it is, to follow once the person lets it go.
    deploy: Option<Deploy>,
    /// The piece of work at a production place it waits for a yes on, whose call it is.
    key: Option<String>,
}

/// A Sikemux tool an agent called, as it called it.
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
    pub caller: Caller,
}

/// An action the person allowed the agent to take once.
struct Allowed {
    agent_id: String,
    permit: Permit,
    /// The call it was allowed on, which hears that it ran.
    call: Option<String>,
    /// The agent has heard it may, so the approval ends with its turn.
    heard: bool,
}

enum Permit {
    /// Exactly this, from there.
    Exactly { ship: Ship, cwd: PathBuf },
}

/// What waits on the person, as the inbox lists it.
pub struct Waiting {
    pub hold: ShipHold,
    /// The site or connector it writes to, when that is what it does.
    pub writes_to: Option<String>,
    pub agent_id: String,
    pub project: String,
    pub at: u64,
}

impl Ships {
    /// Whether an action of the agent's waits on the person.
    pub fn holds(&self, agent_id: &str) -> bool {
        (self.state.locked().holds.iter()).any(|hold| hold.agent_id == agent_id)
    }

    /// The actions waiting on the person, oldest first.
    pub fn waiting(&self) -> Vec<Waiting> {
        (self.state.locked().holds.iter())
            .map(|hold| Waiting {
                hold: ShipHold {
                    id: hold.token.clone(),
                    kind: hold.ship.kind,
                    what: hold.ship.what.clone(),
                    environment: hold.environment.clone(),
                    why: hold.why.clone(),
                    let_go: None,
                    let_go_in: None,
                },
                writes_to: hold.ship.writes_to().map(str::to_owned),
                agent_id: hold.agent_id.clone(),
                project: hold.project.clone(),
                at: hold.at,
            })
            .collect()
    }

    /// What the person answered of the actions held since the core started, oldest first.
    pub fn answered(&self) -> Vec<Answered> {
        self.state.locked().answered.iter().cloned().collect()
    }

    fn remember(&self, hold: &Hold, allowed: bool) {
        let mut state = self.state.locked();
        if state.answered.len() >= MAX_ANSWERED {
            state.answered.pop_front();
        }
        state.answered.push_back(Answered {
            agent_id: hold.agent_id.clone(),
            what: hold.ship.what.clone(),
            allowed,
            at: epoch_ms(),
        });
    }

    /// The agent began a turn, in which what it heard it may do holds.
    pub fn turn_started(&self, agent_id: &str) {
        let mut state = self.state.locked();
        for allowed in (state.allowed.iter_mut()).filter(|allowed| allowed.agent_id == agent_id) {
            allowed.heard = true;
        }
        for (agent, _, heard) in state.covering.iter_mut() {
            if agent == agent_id {
                *heard = true;
            }
        }
    }

    /// The agent's turn ended, and with it what it was allowed in it and the deploys its yes
    /// covered.
    pub fn turn_ended(&self, agent_id: &str) {
        let mut state = self.state.locked();
        (state.allowed).retain(|allowed| allowed.agent_id != agent_id || !allowed.heard);
        let ended: Vec<String> = (state.covering.iter())
            .filter(|(agent, _, heard)| agent == agent_id && *heard)
            .map(|(_, call, _)| call.clone())
            .collect();
        (state.covering).retain(|(agent, _, heard)| agent != agent_id || !heard);
        let done: Vec<String> = (ended.into_iter())
            .filter(|call| !state.covering.iter().any(|(_, other, _)| other == call))
            .collect();
        state.used.extend(done);
    }

    /// The person wrote to the agent in its own chat or terminal.
    pub fn person_spoke(&self, agent_id: &str) {
        (self.state.locked().spoke).insert(agent_id.to_owned(), epoch_ms());
    }

    /// Takes the yes of `call` for one action: once only for an action it covers once, so two
    /// tries at the same moment can't both pass. A check that `follows` the action to its end
    /// shortly after one that doesn't is the same run checked twice, by the hook and the shim.
    fn claim(&self, call: &str, once: bool, follows: bool) -> bool {
        let mut state = self.state.locked();
        if state.used.contains(call) {
            let taken = state.unfollowed.remove(call);
            return follows && taken.is_some_and(|at| epoch_ms() < at + SAME_RUN_MS);
        }
        if once {
            state.used.insert(call.to_owned());
            if !follows {
                state.unfollowed.insert(call.to_owned(), epoch_ms());
            }
        }
        true
    }

    /// The action a yes covered once failed, so the yes stands for its retry.
    fn unspend(&self, call: &str) {
        let mut state = self.state.locked();
        state.used.remove(call);
        state.unfollowed.remove(call);
    }
}

/// Whether the agent `agent_id` may take `ship` from `cwd` now: as the
/// project's environments say for a deploy; once the person allowed exactly
/// that; or where they let agents land work, for a push or a merge. Otherwise
/// it is held, and the agent is told why. What goes toward an environment comes
/// back to be followed to its end.
pub fn check(
    core: &Core,
    agent_id: &str,
    ship: Ship,
    cwd: &Path,
) -> Result<Option<Deploy>, String> {
    check_call(core, agent_id, ship, cwd, None, false)
}

/// [`check`] for a Sikemux tool call, which a hold keeps to run once the person allows it.
fn check_call(
    core: &Core,
    agent_id: &str,
    ship: Ship,
    cwd: &Path,
    call: Option<ToolCall>,
    follows: bool,
) -> Result<Option<Deploy>, String> {
    let mut project = (core.names().get(agent_id).map(|named| named.project))
        .filter(|project| !project.is_empty())
        .unwrap_or_else(|| core.workspaces().project_of(cwd));
    let mut ship = ship;
    if let Some(Target::GitHub { repo }) = ship.target.as_ref().map(Target::bare) {
        if owned_here(repo, cwd, &project) {
            return Ok(None);
        }
    }
    let pushed_in = push::run_in(&ship.what, cwd);
    if ship.kind == ShipKind::Push && !same_repository(&pushed_in, &project) {
        let Some(top) = sikemux_process::files::repo_root(&pushed_in) else {
            return Ok(None);
        };
        project = core.workspaces().project_of(top);
    }
    if ship.kind == ShipKind::Push {
        match push::reach(&ship.what, cwd) {
            Reach::Branches => return Ok(None),
            Reach::Project if places::found::vercel_deploys_pushes(Path::new(&project)) => {
                ship = Ship {
                    kind: ShipKind::Deploy,
                    target: Some(Target::Vercel { production: true }),
                    what: ship.what,
                };
            }
            Reach::Project => {}
            Reach::Tags(names) => {
                if (core.repo_above().board_landing)(core, &project) != Landing::Core {
                    return hold(core, agent_id, &project, ship, cwd, call, None);
                }
                let name = match names.as_slice() {
                    [name] => name.clone(),
                    _ => String::new(),
                };
                let target = match (tag_places(core, &project), releases_on_tags(core, &project)) {
                    (true, _) => Some(Target::Tag { name }),
                    (false, true) => Some(Target::Package {
                        registry: places::GITHUB_RELEASES.to_owned(),
                    }),
                    (false, false) => None,
                };
                if let Some(target) = target {
                    ship = Ship {
                        kind: ShipKind::Publish,
                        target: Some(target),
                        what: ship.what,
                    };
                }
            }
            Reach::Deploy { tool, app } => {
                ship = Ship {
                    kind: ShipKind::Deploy,
                    target: Some(Target::Stage { tool, name: app }),
                    what: ship.what,
                };
            }
        }
    }
    let cwd = pushed_in.as_path();
    if matches!(ship.kind, ShipKind::Push | ShipKind::Merge) {
        if (core.repo_above().board_landing)(core, &project) == Landing::Core {
            return Ok(None);
        }
        return hold(core, agent_id, &project, ship, cwd, call, None);
    }
    let name = core.names().name_or_id(agent_id);
    let judged = places::judge(core, &project, &ship, &name);
    let place = judged.place.as_ref().map(|place| place.name().to_owned());
    let deploy = |call: Option<String>, telling: Option<Telling>| {
        let followed = ship.kind == ShipKind::Deploy || call.is_some() || telling.is_some();
        place.as_ref().filter(|_| followed).map(|name| Deploy {
            head: Git::quick(cwd).trimmed(&["rev-parse", "HEAD"]).ok(),
            agent_id: agent_id.to_owned(),
            project: project.clone(),
            environment: name.clone(),
            call,
            telling,
        })
    };
    match judged.goes {
        Goes::Freely => Ok(deploy(None, None)),
        Goes::Unplaced => {
            places::saw(core, &project, &ship, agent_id);
            Ok(None)
        }
        Goes::Tells => {
            #[expect(
                clippy::expect_used,
                reason = "a judgement that tells always names its place"
            )]
            let told = judged.place.as_ref().expect("a place tells");
            let (key, _) = key_of(told, &ship, work::of(cwd).as_deref(), call.is_some());
            let raise = call_for(core, agent_id, &project, &ship, cwd, told, None);
            Ok(deploy(None, Some(Telling { key, raise })))
        }
        Goes::Asks => {
            #[expect(
                clippy::expect_used,
                reason = "a judgement that asks always names its place"
            )]
            let gated = judged.place.clone().expect("a place asks");
            let work = work::of(cwd);
            let (key, once) = key_of(&gated, &ship, work.as_deref(), call.is_some());
            let yes = person_calls::find_key(core, &project, &key)
                .filter(|call| passes(core, call, agent_id))
                .filter(|call| core.ship().claim(&call.id, once, follows));
            if let Some(yes) = yes {
                if let Err(why) = person_calls::started(core, &yes.id) {
                    if once {
                        core.ship().unspend(&yes.id);
                    }
                    return Err(format!(
                        "Not yet: Sikemux couldn't record that `{}` started ({why}), so it doesn't run. Try again in a moment.",
                        ship.what
                    ));
                }
                if !once {
                    let mut state = core.ship().state.locked();
                    let covered = (state.covering.iter())
                        .any(|(agent, call, _)| agent == agent_id && *call == yes.id);
                    if !covered {
                        state
                            .covering
                            .push((agent_id.to_owned(), yes.id.clone(), true));
                    }
                }
                return Ok(deploy(Some(yes.id), None));
            }
            let asking = Asking {
                place: &gated,
                key,
                once,
            };
            gate(core, agent_id, &project, ship, cwd, asking, judged.why)
        }
    }
}

/// What a yes at `place` is kept under for `ship`, and whether it covers the action once. A
/// deploy of work with a fingerprint is covered with its retries, however spelled, at the same
/// target; anything else, or work git can't fingerprint, only as the exact action, from any
/// checkout of the project.
pub fn key_of(
    place: &places::Place,
    ship: &Ship,
    work: Option<&str>,
    tool: bool,
) -> (String, bool) {
    let target = (ship.target.as_ref())
        .and_then(|target| serde_json::to_string(target).ok())
        .unwrap_or_default();
    let digest = |text: &str| {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(text.as_bytes());
        hex::encode(hash).get(..16).unwrap_or_default().to_owned()
    };
    let id = &place.shown.id;
    match (ship.kind, work) {
        (ShipKind::Deploy, Some(work)) if !tool => (
            format!("place:{id}:work:{work}:to:{}", digest(&target)),
            false,
        ),
        _ => {
            let action = digest(&format!("{target}\n{}", ship.what));
            (format!("place:{id}:work::action:{action}"), true)
        }
    }
}

/// Whether an answered call is a yes that still stands: picked as its allow option since this
/// core started, not yet used, and its deploy not yet ended well.
fn passes(core: &Core, call: &PersonCall, asking: &str) -> bool {
    settle_covering(core, &call.id, asking);
    let yes = (call.answer.as_ref())
        .is_some_and(|answer| said_yes(answer.option.as_deref()) && answer.at >= core.ship().born);
    if core.ship().state.locked().used.contains(&call.id) {
        return false;
    }
    let used = (call.outcome.as_ref()).is_some_and(|outcome| {
        matches!(
            outcome.result,
            CallResult::NotShipped | CallResult::Withdrawn
        )
    });
    yes && !used
}

/// A deploy's yes covers those on its call until the last of their turns ends; an agent that
/// closed takes no more turns, so it counts as ended.
fn settle_covering(core: &Core, call: &str, asking: &str) {
    let on_call: Vec<String> = (core.ship().state.locked().covering.iter())
        .filter(|(_, covered, _)| covered == call)
        .map(|(agent, _, _)| agent.clone())
        .collect();
    if on_call.is_empty() {
        return;
    }
    let gone: Vec<String> = (on_call.into_iter())
        .filter(|agent| agent != asking)
        .filter(|agent| !core.agent_now(agent).running() && core.workspaces().closed(agent))
        .collect();
    if gone.is_empty() {
        return;
    }
    let mut state = core.ship().state.locked();
    state
        .covering
        .retain(|(agent, covered, _)| covered != call || !gone.contains(agent));
    if !state.covering.iter().any(|(_, covered, _)| covered == call) {
        state.used.insert(call.to_owned());
    }
}

fn refused(call: &PersonCall) -> bool {
    (call.answer.as_ref()).is_some_and(|answer| !said_yes(answer.option.as_deref()))
}

/// The option of a call at a place whose changes recur that says yes, and stops asking.
pub const FROM_NOW_ON: &str = "allowFromNowOn";

fn said_yes(option: Option<&str>) -> bool {
    matches!(option, Some(person_calls::HOLD_ALLOW | FROM_NOW_ON))
}

/// What a gate asks the person about: the place, the key its answer is kept under, and whether
/// a yes covers the action once.
struct Asking<'a> {
    place: &'a places::Place,
    key: String,
    once: bool,
}

/// The call that puts a change of `ship` to the production place `place` to the person.
fn call_for(
    core: &Core,
    agent_id: &str,
    project: &str,
    ship: &Ship,
    cwd: &Path,
    place: &places::Place,
    key: Option<String>,
) -> Raise {
    let name = core.names().name_or_id(agent_id);
    let subject = (ship.kind == ShipKind::Deploy)
        .then(|| Git::quick(cwd).trimmed(&["log", "-1", "--format=%s"]).ok())
        .flatten()
        .filter(|subject| !subject.is_empty());
    let here = || {
        let remote = Git::quick(cwd)
            .trimmed(&["remote", "get-url", "origin"])
            .ok()?;
        let (owner, name) = super::credential::owner_and_name(&remote)?;
        Some(format!("{owner}/{name}"))
    };
    let tag = match (&ship.target, push::deletes(&ship.what)) {
        (Some(Target::Tag { .. }), Some(deleted)) => Some(format!(
            "Delete {} on {}",
            deleted.join(", "),
            here().unwrap_or_else(|| "GitHub".to_owned())
        )),
        (Some(Target::Tag { name }), None) if !name.is_empty() => {
            Some(format!("Publish {name} to {}", place.name()))
        }
        (Some(Target::Tag { .. }), None) => Some(format!(
            "Run this push, which may send tags to {}",
            place.name()
        )),
        _ => None,
    };
    let github = tag.or_else(|| github_write(&ship.what, here));
    let option = |id: &str, label: String| CallOption {
        id: id.to_owned(),
        label,
        detail: None,
        picture: None,
    };
    Raise {
        project: project.to_owned(),
        kind: CallKind::Production,
        asker: CallAsker {
            agent_id: agent_id.to_owned(),
            cli: (core.repo_above().questions_cli_of)(core, agent_id),
            name,
            chief: false,
        },
        question: (github.as_ref().map(|write| format!("{write}?")))
            .unwrap_or_else(|| format!("Change {} with this work?", place.name())),
        context: subject.map(|subject| format!("The work's latest commit: {subject}")),
        options: [
            Some(option(
                person_calls::HOLD_ALLOW,
                github.unwrap_or_else(|| format!("Change {}", place.name())),
            )),
            place.recurs.then(|| {
                let label = format!("Change {}, and from now on just tell me", place.name());
                option(FROM_NOW_ON, label)
            }),
            Some(option(person_calls::HOLD_REFUSE, "Not now".to_owned())),
        ]
        .into_iter()
        .flatten()
        .collect(),
        pick: None,
        against: None,
        evidence: Vec::new(),
        action: Some(ship.what.clone()),
        brief: None,
        place: Some(CallPlace {
            id: place.shown.id.clone(),
            name: place.name().to_owned(),
        }),
        key,
        yes: Some(person_calls::HOLD_ALLOW.to_owned()),
    }
}

/// What a `gh api` command writes on GitHub, in the words the person would use for it, such
/// as "Comment on owner/repo#12", and never an endpoint or an id. `here` names the checkout's
/// repository, which gh puts in for `{owner}/{repo}`.
fn github_write(what: &str, here: impl FnOnce() -> Option<String>) -> Option<String> {
    let command = what.rsplit(" && ").next()?;
    let words: Vec<&str> = command.split_whitespace().collect();
    let ["gh", "api", args @ ..] = words.as_slice() else {
        return None;
    };
    let call = crate::ship::github_api(args)?;
    let Some(named) = call.repo() else {
        return Some("Write to GitHub as you".to_owned());
    };
    let repo = match named.split_once('/') {
        Some(("{owner}", _) | (_, "{repo}")) => {
            here().unwrap_or_else(|| "this repository".to_owned())
        }
        _ => named,
    };
    let path = call.endpoint.split(['?', '#']).next().unwrap_or_default();
    let rest: Vec<&str> = path.split('/').skip(3).collect();
    let said = match (call.method.as_str(), rest.as_slice()) {
        ("POST", ["issues" | "pulls", number, "comments", ..]) => {
            format!("Comment on {repo}#{number}")
        }
        ("POST", ["pulls", number, "reviews", ..]) => format!("Review {repo}#{number}"),
        ("PATCH", ["issues" | "pulls", number]) => format!("Edit {repo}#{number}"),
        ("PUT", ["pulls", number, "merge"]) => format!("Merge {repo}#{number}"),
        ("POST", ["issues"]) => format!("Open an issue on {repo}"),
        ("POST", ["pulls"]) => format!("Open a pull request on {repo}"),
        ("POST", ["forks"]) => format!("Fork {repo}"),
        ("PUT", ["contents", path @ ..]) => format!("Update {} in {repo}", path.join("/")),
        ("DELETE", ["contents", path @ ..]) => format!("Delete {} in {repo}", path.join("/")),
        ("POST", ["git", "refs"]) => format!("Create a branch in {repo}"),
        ("PATCH", ["git", "refs", "heads", branch @ ..]) => {
            format!("Move branch {} in {repo}", branch.join("/"))
        }
        ("DELETE", ["git", "refs", "heads", branch @ ..]) => {
            format!("Delete branch {} in {repo}", branch.join("/"))
        }
        ("POST", ["releases"]) => format!("Publish a release of {repo}"),
        ("DELETE", []) => format!("Delete {repo}"),
        ("PATCH", []) => format!("Change the settings of {repo}"),
        ("POST", ["pages"]) => format!("Turn on GitHub Pages for {repo}"),
        ("PUT", ["pages"]) => format!("Change GitHub Pages for {repo}"),
        ("DELETE", ["pages"]) => format!("Turn off GitHub Pages for {repo}"),
        ("POST", ["pages", "builds"]) => format!("Rebuild GitHub Pages for {repo}"),
        (_, ["rulesets", ..]) => format!("Change who may push what to {repo}"),
        (_, ["branches", _, "protection", ..]) => format!("Change branch protection on {repo}"),
        (_, ["labels", ..]) => format!("Change the labels of {repo}"),
        (_, ["collaborators", ..]) => format!("Change who can work on {repo}"),
        (_, ["hooks", ..]) => format!("Change the webhooks of {repo}"),
        (_, ["actions", "secrets", ..]) => format!("Change the secrets of {repo}"),
        (_, ["actions", ..] | ["environments", ..]) => {
            format!("Change how GitHub Actions runs for {repo}")
        }
        _ => format!("Change {repo} on GitHub as you"),
    };
    Some(said)
}

/// Whether the project keeps a place for tags it pushes.
fn tag_places(core: &Core, project: &str) -> bool {
    let (places, _) = places::current(core, project);
    (places.iter())
        .flat_map(|place| place.recognise.iter())
        .any(|fact| matches!(fact, Recognise::Tag { .. }))
}

/// Whether a workflow the project keeps publishes a release, so a pushed tag can start one.
fn releases_on_tags(core: &Core, project: &str) -> bool {
    let releases = Recognise::Package {
        registry: places::GITHUB_RELEASES.to_owned(),
    };
    let (places, _) = places::current(core, project);
    places
        .iter()
        .any(|place| place.recognise.contains(&releases))
}

/// A change to the production place `place` waits for the person's yes on one call for this
/// piece of work there, which every agent waiting on the same work there joins. A no holds for
/// that work there until the work changes or the person writes to the agent again, with no
/// second call. An agent waits on one call at a place: a new action there, such as a retry
/// spelled another way, withdraws the call it waited on.
fn gate(
    core: &Core,
    agent_id: &str,
    project: &str,
    ship: Ship,
    cwd: &Path,
    asking: Asking<'_>,
    why: Option<String>,
) -> Result<Option<Deploy>, String> {
    let Asking { place, key, once } = asking;
    if let Some(no) = person_calls::find_key(core, project, &key).filter(refused) {
        let answered = no.answer.as_ref().map_or(0, |answer| answer.at);
        let spoke = core.ship().state.locked().spoke.get(agent_id).copied();
        if spoke.is_none_or(|spoke| spoke <= answered) {
            return Err(format!(
                "Not yet: the person said no to changing {} with this work, until the work changes or they tell you otherwise.",
                place.name()
            ));
        }
    }
    let stale: Vec<String> = (core.ship().state.locked().holds.iter())
        .filter(|hold| hold.agent_id == agent_id)
        .filter_map(|hold| hold.key.as_deref().map(|held| (hold, held)))
        .filter(|(_, held)| *held != key && same_place(held, &key))
        .map(|(hold, _)| hold.token.clone())
        .collect();
    for token in stale {
        withdraw(core, agent_id, &token, "its agent asked again there");
    }
    let oldest = {
        let state = core.ship().state.locked();
        let mine: Vec<&Hold> = (state.holds.iter())
            .filter(|hold| hold.agent_id == agent_id && hold.key.is_some())
            .collect();
        (mine.first())
            .filter(|_| mine.len() >= MAX_HOLDS_PER_AGENT)
            .map(|oldest| (oldest.token.clone(), oldest.ship.what.clone()))
    };
    if let Some((token, what)) = oldest {
        withdraw(
            core,
            agent_id,
            &token,
            "it waited on more calls than Sikemux keeps",
        );
        let text = format!(
            "Sikemux withdrew its call for `{what}`, the oldest of the calls you wait on, as you wait on more than it keeps. Run it again when you need it."
        );
        tell_soon(core, agent_id, text);
    }
    let raise = call_for(
        core,
        agent_id,
        project,
        &ship,
        cwd,
        place,
        Some(key.clone()),
    );
    let id = match person_calls::raise(core, raise) {
        Ok(id) => id,
        Err(why) => {
            return Err(format!(
                "Waits for the person: `{}` changes {}, a production place, but Sikemux couldn't put the call to them ({why}). Don't run it; try again in a moment.",
                ship.what,
                place.name()
            ))
        }
    };
    {
        let mut state = core.ship().state.locked();

        let waiting = (state.holds.iter())
            .any(|hold| hold.agent_id == agent_id && hold.key.as_deref() == Some(key.as_str()));
        if !waiting {
            state.holds.push(Hold {
                token: id,
                agent_id: agent_id.to_owned(),
                project: project.to_owned(),
                cwd: cwd.to_owned(),
                at: epoch_ms(),
                environment: Some(place.name().to_owned()),
                why: why.clone(),
                deploy: None,
                call: None,
                key: Some(key),
                ship: ship.clone(),
            });
        }
    }
    core.changed(What::Agents);
    let covers = match once {
        true => "this action once",
        false => "this work's deploy there until the end of that turn",
    };
    let mut said = format!(
        "Waits for the person's yes: `{}` changes {}, a production place. You will hear their answer; a yes covers {covers}. Running anything else there withdraws this call, so put every change you need there in one command. Go on with other work meanwhile.",
        ship.what,
        place.name()
    );
    if let Some(why) = why {
        said = format!("{said} {why}");
    }
    Err(said)
}

/// Sends the agent `text` as from Sikemux, without waiting on it.
fn tell_soon(core: &Core, agent_id: &str, text: String) {
    let (Some(core), agent_id) = (core.arc(), agent_id.to_owned()) else {
        return;
    };
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            let _ = (core.repo_above().messages_for_person)(&core, agent_id, text).await;
        });
    }
}

/// Whether `dir` is in the repository of `project`, one of its worktrees or a clone that
/// pushes to one of its remotes. A push from another repository, such as a clone of someone
/// else's, reaches none of the project's places.
fn same_repository(dir: &Path, project: &str) -> bool {
    let read = |at: &Path| {
        let git = || Git::quick(at);
        let common = git()
            .trimmed(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .ok()?;
        let remotes = git().trimmed(&["remote", "-v"]).unwrap_or_default();
        let urls: HashSet<String> = (remotes.lines())
            .filter_map(|line| line.split_whitespace().nth(1).map(str::to_owned))
            .collect();
        Some((
            sikemux_process::files::real_path(PathBuf::from(common)).ok()?,
            urls,
        ))
    };
    match (read(dir), read(Path::new(project))) {
        (Some((pushed, theirs)), Some((own, ours))) => pushed == own || !theirs.is_disjoint(&ours),
        _ => true,
    }
}

/// Whether the GitHub repository `repo` belongs to whoever owns the remotes of the checkout at
/// `cwd` or of `project`, as gh's `{owner}` does, so a write there is the project's own work.
fn owned_here(repo: &str, cwd: &Path, project: &str) -> bool {
    let owner = repo.split('/').next().unwrap_or_default();
    if owner.is_empty() || owner == "{owner}" {
        return !repo.is_empty();
    }
    [cwd, Path::new(project)].into_iter().any(|dir| {
        let remotes = Git::quick(dir)
            .trimmed(&["remote", "-v"])
            .unwrap_or_default();
        (remotes.lines())
            .filter_map(|line| line.split_whitespace().nth(1))
            .filter_map(super::credential::owner_and_name)
            .any(|(theirs, _)| theirs.eq_ignore_ascii_case(owner))
    })
}

/// Whether two keys name the same place, whatever the work or the action.
fn same_place(one: &str, other: &str) -> bool {
    let place = |key: &str| key.split_once(":work:").map(|(place, _)| place.to_owned());
    place(one).is_some_and(|one| Some(one) == place(other))
}

/// Drops `agent_id`'s wait on the gate call `token`, and withdraws the call when nobody else
/// waits on it, telling the others that joined it why.
fn withdraw(core: &Core, agent_id: &str, token: &str, why: &str) {
    let others: Vec<String> = {
        let mut state = core.ship().state.locked();
        state
            .holds
            .retain(|hold| !(hold.token == token && hold.agent_id == agent_id));
        (state.holds.iter())
            .filter(|hold| hold.token == token)
            .map(|hold| hold.agent_id.clone())
            .collect()
    };
    if !others.is_empty() {
        return;
    }
    let outcome = CallOutcome {
        result: CallResult::Withdrawn,
        healthy: None,
        roll_back: false,
        why: None,
        at: epoch_ms(),
    };
    person_calls::set_outcome(core, token, outcome);
    eprintln!("sikemux core: call {token} was withdrawn, as {why}");
}

/// Holds `ship` for the person, unless they allowed exactly it or a deploy to its place
/// already, and tells the agent why.
fn hold(
    core: &Core,
    agent_id: &str,
    project: &str,
    ship: Ship,
    cwd: &Path,
    call: Option<ToolCall>,
    judged: Option<Judged>,
) -> Result<Option<Deploy>, String> {
    let then = match call {
        Some(_) => "Do not call it again: once they allow it, Sikemux runs it and sends you what it answered.",
        None => "You will hear when they allow it, and then run exactly this again.",
    };
    let why = judged.as_ref().and_then(|judged| judged.why.clone());
    let place = (judged.as_ref())
        .and_then(|judged| judged.place.as_ref())
        .map(|place| place.name().to_owned());
    let mut said = format!(
        "Held for the person: `{}` {}, so Sikemux asked them first: they answer it on its card in your chat in Sikemux, on the notch, or in your chat on their phone. {then} Go on with other work meanwhile.",
        ship.what,
        leaves(&ship)
    );
    if let Some(why) = &why {
        said = format!("{said} {why}");
    }
    let allowed_deploy = (place.as_ref())
        .filter(|_| ship.kind == ShipKind::Deploy)
        .map(|name| Deploy {
            head: Git::quick(cwd).trimmed(&["rev-parse", "HEAD"]).ok(),
            agent_id: agent_id.to_owned(),
            project: project.to_owned(),
            environment: name.clone(),
            call: None,
            telling: None,
        });
    {
        let mut state = core.ship().state.locked();
        let same =
            |agent: &str, held: &Ship, at: &Path| agent == agent_id && *held == ship && at == cwd;
        if let Some(at) = (state.allowed.iter()).position(|allowed| {
            let Permit::Exactly {
                ship: held,
                cwd: at,
            } = &allowed.permit;
            same(&allowed.agent_id, held, at)
        }) {
            let ran = state.allowed.remove(at).call;
            drop(state);
            if let Some(call) = ran {
                person_calls::set_outcome(core, &call, went(CallResult::Shipped));
            }
            return Ok(allowed_deploy);
        }
        if (state.holds.iter()).any(|hold| same(&hold.agent_id, &hold.ship, &hold.cwd)) {
            return Err(said);
        }
        let held = (state.holds.iter())
            .filter(|hold| hold.agent_id == agent_id)
            .count();
        if held >= MAX_HOLDS_PER_AGENT {
            if let Some(oldest) = (state.holds.iter()).position(|hold| hold.agent_id == agent_id) {
                state.holds.remove(oldest);
            }
        }
        state.holds.push(Hold {
            token: uuid::Uuid::new_v4().simple().to_string(),
            agent_id: agent_id.to_owned(),
            project: project.to_owned(),
            cwd: cwd.to_owned(),
            at: epoch_ms(),
            environment: place,
            why,
            deploy: call.as_ref().and(allowed_deploy),
            call,
            key: None,
            ship,
        });
    }
    core.changed(What::Agents);
    tell_calls(core);
    Err(said)
}

fn leaves(ship: &Ship) -> String {
    if let Some(place) = ship.writes_to() {
        return format!("writes to {place} as the person");
    }
    let words = match ship.kind {
        ShipKind::Push => "pushes commits off this machine",
        ShipKind::Merge => "merges a pull request",
        ShipKind::Deploy => "deploys",
        ShipKind::Publish => "publishes a release",
        ShipKind::ProductionWrite => "can change a production database",
    };
    words.to_owned()
}

/// The databases marked production, as the host each is reached at and its name.
async fn production_databases(core: &Arc<Core>) -> Vec<(String, String)> {
    let listed = plugged::call(core, DATABASE, "agentDatabases", json!({}))
        .await
        .unwrap_or_default();
    (listed.as_array().into_iter().flatten())
        .filter(|database| database["production"] == true)
        .filter_map(|database| {
            let address = database["address"].as_str()?;
            let host = address.rsplit_once('@').map_or(address, |(_, at)| at);
            let host = host.split([':', '/']).next()?;
            Some((host.to_owned(), database["name"].as_str()?.to_owned()))
        })
        .collect()
}

/// `ship` as the check sees it: a database client's write as a write to the production
/// database it reaches, and none when the database it reaches isn't marked production.
fn on_databases(ship: Ship, databases: &[(String, String)]) -> Option<Ship> {
    let Some(Target::Site { origin }) = &ship.target else {
        return Some(ship);
    };
    let Some(host) = (origin.strip_prefix("psql://")).or_else(|| origin.strip_prefix("mysql://"))
    else {
        return Some(ship);
    };
    let (_, name) = databases.iter().find(|(at, _)| at == host)?;
    Some(Ship {
        kind: ShipKind::ProductionWrite,
        target: Some(Target::Database { name: name.clone() }),
        what: ship.what,
    })
}

async fn at_production_database(core: &Arc<Core>, ship: Ship) -> Option<Ship> {
    let client = matches!(&ship.target, Some(Target::Site { origin })
        if origin.starts_with("psql://") || origin.starts_with("mysql://"));
    match client {
        true => on_databases(ship, &production_databases(core).await),
        false => Some(ship),
    }
}

/// What the shim and the hook ask before an action leaving this machine runs:
/// whether it may, and if not, what the agent is told. A program no agent of
/// this core runs, as the person's own shell does, is never held. Where the asker
/// `follows` the program to its end it hears a `went` to answer with; otherwise
/// what went is kept at once, as not seen to end.
pub async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let Some(agent_id) = request.agent_id.clone() else {
        return Ok(json!({ "allowed": true }));
    };
    let kind: ShipKind =
        serde_json::from_value(request.params.get("kind").cloned().unwrap_or_default())
            .map_err(|_| "kind must be what the action does")?;
    let what = (request.params.get("what").and_then(Value::as_str))
        .ok_or("what must be the action's command")?
        .to_owned();
    let target = serde_json::from_value::<Option<Target>>(
        request.params.get("target").cloned().unwrap_or_default(),
    )
    .ok()
    .flatten();
    let follows = request
        .params
        .get("follows")
        .is_some_and(|follows| follows == true);
    let cwd = PathBuf::from(&request.project);
    let Some(ship) = at_production_database(core, Ship { kind, what, target }).await else {
        return Ok(json!({ "allowed": true }));
    };
    let checking = core.clone();
    let checked = tokio::task::spawn_blocking(move || {
        check_call(&checking, &agent_id, ship, &cwd, None, follows)
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(match checked {
        Ok(None) => json!({ "allowed": true }),
        Ok(Some(deploy)) if follows => {
            let went = {
                let mut state = core.ship().state.locked();
                state.next += 1;
                let went = state.next;
                if state.going.len() >= MAX_GOING {
                    if let Some((_, lost)) = state.going.pop_front() {
                        let core = core.clone();
                        tokio::spawn(async move { ended(core, lost, None).await });
                    }
                }
                state.going.push_back((went, deploy));
                went
            };
            let waiting = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(GOING_AT_MOST).await;
                let lost = {
                    let mut state = waiting.ship().state.locked();
                    let at = state.going.iter().position(|(id, _)| *id == went);
                    at.and_then(|at| state.going.remove(at))
                };
                if let Some((_, deploy)) = lost {
                    ended(waiting, deploy, None).await;
                }
            });
            json!({ "allowed": true, "went": went.to_string() })
        }
        Ok(Some(deploy)) => {
            let core = core.clone();
            tokio::spawn(async move { ended(core, deploy, None).await });
            json!({ "allowed": true })
        }
        Err(said) => json!({ "allowed": false, "said": said }),
    })
}

/// Whether the shell command line `command`, which agent `agent_id` runs in `cwd`, may run:
/// each of its commands that `looks` sees leave this machine, as [`check`] says. `set` reads
/// the variables the agent was given beside the person's own.
async fn check_command(
    core: &Arc<Core>,
    agent_id: &str,
    command: &str,
    set: impl Fn(&str) -> Option<String> + Send + 'static,
    cwd: &Path,
    looks: Looks,
) -> Result<(), String> {
    let (checking, agent_id, command, cwd) = (
        core.clone(),
        agent_id.to_owned(),
        command.to_owned(),
        cwd.to_owned(),
    );
    let databases = match command.contains("psql") || command.contains("mysql") {
        true => production_databases(core).await,
        false => Vec::new(),
    };
    let going = tokio::task::spawn_blocking(move || {
        let env = |name: &str| set(name).or_else(|| sikemux_process::user_environment::var(name));
        let ships = crate::git_guard::shim::ships_of(&command, looks, &cwd, &env);
        let mut going = Vec::new();
        for ship in ships
            .into_iter()
            .filter_map(|ship| on_databases(ship, &databases))
        {
            going.extend(check(&checking, &agent_id, ship, &cwd)?);
        }
        Ok::<_, String>(going)
    })
    .await
    .map_err(|error| error.to_string())??;
    for deploy in going {
        let core = core.clone();
        tokio::spawn(async move { ended(core, deploy, None).await });
    }
    Ok(())
}

/// Whether the process `command`, which agent `agent_id` starts in `cwd` with `env` set,
/// may run: each of its commands that leaves this machine, as [`check`] says. The core
/// starts the process outside the agent's own, so the shim inside it can't tell it is the
/// agent's.
pub async fn check_process(
    core: &Arc<Core>,
    agent_id: &str,
    command: &str,
    env: &[(String, String)],
    cwd: &Path,
) -> Result<(), String> {
    let env = env.to_vec();
    let set = move |name: &str| {
        (env.iter().rev())
            .find(|(set, _)| set == name)
            .map(|(_, value)| value.clone())
    };
    check_command(core, agent_id, command, set, cwd, Looks::Every)
        .await
        .map_err(|said| format!("{said} Start the process with a new idempotencyKey."))
}

/// Whether the shell command `command`, which the chat of agent `agent_id` is about to run
/// in `cwd`, may run. Its agent was started with `env` set and the shim first on its PATH,
/// which checks the programs it stands in for as they run, so this checks the rest: programs
/// run by a path, and the project's own commands.
pub async fn check_hooked(
    core: &Arc<Core>,
    agent_id: &str,
    command: &str,
    env: BTreeMap<String, String>,
    cwd: &Path,
) -> Result<(), String> {
    if let Some(said) = waits_by_sleeping(command) {
        return Err(said);
    }
    let set = move |name: &str| env.get(name).cloned();
    check_command(core, agent_id, command, set, cwd, Looks::PastTheShim).await
}

/// What the agent is told instead of running `command` when it waits by sleeping: a sleep in a
/// loop of five seconds or more, or one of a minute or more.
fn waits_by_sleeping(command: &str) -> Option<String> {
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
        .filter(|word| !word.is_empty())
        .collect();
    let seconds = |word: &str| {
        let (number, unit) = word.split_at(
            word.find(|c: char| c.is_ascii_alphabetic())
                .unwrap_or(word.len()),
        );
        let number: f64 = number.parse().ok()?;
        Some(
            number
                * match unit {
                    "" | "s" => 1.0,
                    "m" => 60.0,
                    "h" => 3600.0,
                    _ => return None,
                },
        )
    };
    let sleeps: Vec<f64> = (words.iter().zip(words.iter().skip(1)))
        .filter(|(word, _)| **word == "sleep")
        .filter_map(|(_, after)| seconds(after))
        .collect();
    let looped = words
        .iter()
        .any(|word| matches!(*word, "while" | "until" | "for"));
    let waits = sleeps
        .iter()
        .any(|seconds| *seconds >= 60.0 || (looped && *seconds >= 5.0));
    waits.then(|| "Sleeping to wait spends your context and holds the turn. Set trigger_add for what you wait on: process, ci or agent, else every with a check that prints once it is ready; then end your turn.".to_owned())
}

/// How a deploy the shim followed ended, by the `went` the core gave it.
pub fn ran_call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let went: u64 = (request.params.get("went").and_then(Value::as_str))
        .and_then(|went| went.parse().ok())
        .ok_or("went must be what the check answered with")?;
    let deploy = {
        let mut state = core.ship().state.locked();
        let at = (state.going.iter())
            .position(|(id, deploy)| {
                *id == went && Some(&deploy.agent_id) == request.agent_id.as_ref()
            })
            .ok_or("no such deploy is going")?;
        state.going.remove(at).map(|(_, deploy)| deploy)
    };
    let exit = request.params.get("exit").and_then(Value::as_i64);
    let verdict = match exit {
        Some(0) => Verdict::Passed,
        _ => Verdict::Failed,
    };
    if let Some(deploy) = deploy {
        let core = core.clone();
        tokio::spawn(async move { ended(core, deploy, Some(verdict)).await });
    }
    Ok(json!({}))
}

/// What a call to the Sikemux tool `name` changes and where it lands, for the tools that can
/// reach a production place; the places on the list decide whether it waits.
async fn tool_ship(
    core: &Arc<Core>,
    name: &str,
    arguments: &Value,
) -> Result<Option<(ShipKind, Option<Target>)>, String> {
    Ok(Some(match name {
        "db_query" => {
            let asked = plugged::call(core, DATABASE, "agentWritesProduction", arguments.clone())
                .await
                .map_err(|failure| plugged::explain(&failure))?;
            if asked != true {
                return Ok(None);
            }
            let name = arguments["database"].as_str().unwrap_or("production");
            let target = Target::Database {
                name: name.to_owned(),
            };
            (ShipKind::ProductionWrite, Some(target))
        }
        "rundeck_run" | "rundeck_import" => {
            let place = plugged::call(core, RUNDECK, "agentJobPlace", arguments.clone())
                .await
                .ok();
            let target = place.and_then(|place| {
                Some(Target::RundeckJob {
                    path: place.get("path")?.as_str()?.to_owned(),
                    production: place.get("production")?.as_bool()?,
                })
            });
            (ShipKind::Deploy, target)
        }
        "vercel_promote" => (ShipKind::Deploy, Some(Target::Vercel { production: true })),
        "vercel_redeploy" => {
            let target = match arguments["target"].as_str() {
                Some(target) => Some(target.to_owned()),
                None => plugged::call(core, VERCEL, "deployment", arguments.clone())
                    .await
                    .ok()
                    .and_then(|read| read.get("target")?.as_str().map(str::to_owned)),
            };
            let target = target.map(|target| Target::Vercel {
                production: target == "production",
            });
            (ShipKind::Deploy, target)
        }
        _ => return Ok(None),
    }))
}

/// Whether an agent's call to the Sikemux tool `name` may run now, as
/// [`check`] says for what it does.
pub async fn check_tool(
    core: &Arc<Core>,
    request: &HarnessRequest,
    name: &str,
    arguments: &Value,
) -> Result<Option<Deploy>, String> {
    let Some(agent_id) = &request.agent_id else {
        return Ok(None);
    };
    let Some((kind, target)) = tool_ship(core, name, arguments).await? else {
        return Ok(None);
    };
    let what = format!("{name} {arguments}");
    let (checking, agent_id, cwd) = (core.clone(), agent_id.clone(), request.project.clone());
    let call = ToolCall {
        name: name.to_owned(),
        arguments: arguments.clone(),
        caller: (core.repo_above().plugins_caller_of)(core, request),
    };
    tokio::task::spawn_blocking(move || {
        check_call(
            &checking,
            &agent_id,
            Ship { kind, what, target },
            Path::new(&cwd),
            Some(call),
            true,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Whether an agent's browser may send the write it is about to make to `origin`,
/// as [`check`] says: none when it may, otherwise what the agent is told.
pub async fn check_web_write(
    core: &Arc<Core>,
    params: WebWriteParams,
) -> CoreResult<Option<String>> {
    let ship = Ship {
        kind: ShipKind::Publish,
        what: params.what,
        target: Some(Target::Site {
            origin: params.origin,
        }),
    };
    let checking = core.clone();
    let checked = tokio::task::spawn_blocking(move || {
        check(&checking, &params.agent, ship, Path::new(&params.project))
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(match checked {
        Ok(Some(deploy)) => {
            let core = core.clone();
            tokio::spawn(async move { ended(core, deploy, None).await });
            None
        }
        Ok(None) => None,
        Err(said) => Some(said),
    })
}

/// How far a deploy tool's answer says it got.
enum Reached {
    Ended(Verdict),
    Going,
    /// The answer doesn't say.
    Unknown,
}

/// What a look at a deploy tool's run says of how far it got. Rundeck's status
/// can be the job's own word, so only one the core knows settles a verdict.
fn reached(tool: &str, answer: &Value) -> Reached {
    if tool.starts_with("rundeck_") {
        if answer["ended"].is_null() {
            return Reached::Going;
        }
        return match answer["status"].as_str() {
            Some("succeeded") => Reached::Ended(Verdict::Passed),
            Some("failed" | "aborted" | "timedout" | "failed-with-retry") => {
                Reached::Ended(Verdict::Failed)
            }
            _ => Reached::Unknown,
        };
    }
    match answer["state"]
        .as_str()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ready") => Reached::Ended(Verdict::Passed),
        Some("error" | "canceled") => Reached::Ended(Verdict::Failed),
        Some(_) => Reached::Going,
        None => Reached::Unknown,
    }
}

/// What to read again while a deploy tool's run is going.
fn read_again(tool: &str, answer: &Value) -> Option<(&'static str, Value)> {
    if tool.starts_with("rundeck_") {
        return Some((
            "rundeck_execution",
            json!({ "executionId": answer["id"].as_u64()?, "wait": true }),
        ));
    }
    Some((
        "vercel_deployment",
        json!({ "id": answer["id"].as_str()?, "team": answer["team"] }),
    ))
}

/// What a deploy tool's answer says happened, in a few lines.
fn answer_words(answer: &Value) -> String {
    let said = |field: &str| answer[field].as_str().map(str::to_owned);
    let mut lines: Vec<String> = [said("status"), said("state"), said("url"), said("error")]
        .into_iter()
        .flatten()
        .collect();
    if let Some(log) = answer["log"].as_array() {
        lines.extend(log.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    lines.join("\n")
}

/// A deploy tool let go toward an environment answered: it is followed to its
/// end and kept as evidence, without holding up the agent.
pub fn tool_ran(core: &Arc<Core>, deploy: Deploy, tool: &str, answer: Result<&Value, &str>) {
    let (core, tool) = (core.clone(), tool.to_owned());
    let answer = answer.cloned().map_err(str::to_owned);
    tokio::spawn(async move {
        let verdict = match answer {
            Err(_) => Some(Verdict::Failed),
            Ok(answer) => followed(&core, &tool, answer).await.0,
        };
        ended(core, deploy, verdict).await;
    });
}

/// Reads a deploy tool's run until it ends, within `FOLLOW_LOOKS`. No verdict
/// when the core did not see the end.
async fn followed(core: &Arc<Core>, tool: &str, mut answer: Value) -> (Option<Verdict>, String) {
    let Some(Provided(plugins)) = plugged::provided(core) else {
        return (None, answer_words(&answer));
    };
    let mut reading = tool.to_owned();
    for _ in 0..FOLLOW_LOOKS {
        match reached(&reading, &answer) {
            Reached::Ended(verdict) => return (Some(verdict), answer_words(&answer)),
            Reached::Unknown => return (None, answer_words(&answer)),
            Reached::Going => {}
        }
        let Some((next, arguments)) = read_again(&reading, &answer) else {
            return (None, answer_words(&answer));
        };
        tokio::time::sleep(FOLLOW_EVERY).await;
        if let Ok(read) = plugins.call_agent_tool(next, arguments).await {
            answer = read;
            reading = next.to_owned();
        }
    }
    (None, answer_words(&answer))
}

/// Whether the project's health check for the environment answers, in a line.
async fn health_of(address: &str) -> (Verdict, String) {
    let client = crate::http::client();
    let mut said = format!("{address} did not answer");
    for attempt in 0..HEALTH_TRIES {
        if attempt > 0 {
            tokio::time::sleep(HEALTH_EVERY).await;
        }
        let Ok(client) = client else {
            break;
        };
        match client.get(address).timeout(HEALTH_TIMEOUT).send().await {
            Ok(response) if response.status().is_success() => {
                return (
                    Verdict::Passed,
                    format!("{address} answered {}", response.status()),
                );
            }
            Ok(response) => said = format!("{address} answered {}", response.status()),
            Err(error) => said = format!("{address} did not answer: {error}"),
        }
    }
    (Verdict::Failed, said)
}

/// A deploy reached its end: its result, with the project's health check on one that went
/// well, goes on the person's call for it.
pub async fn ended(core: Arc<Core>, deploy: Deploy, verdict: Option<Verdict>) {
    let mut healthy = None;
    if verdict == Some(Verdict::Passed) {
        if let Some(address) =
            (core.repo_above().board_health_of)(&core, &deploy.project, &deploy.environment)
        {
            let (health, _) = health_of(&address).await;
            healthy = Some(health == Verdict::Passed);
        }
        let (kept, _) = places::current(&core, &deploy.project);
        let production = (kept.iter())
            .any(|place| place.shown.production && place.shown.name == deploy.environment);
        if let (false, false, Some(head)) =
            (production, healthy == Some(false), deploy.head.clone())
        {
            (core.repo_above().inbox_staged)(&core, &deploy.agent_id, head);
        }
    }
    let result = match verdict {
        Some(Verdict::Failed) => CallResult::Failed,
        Some(_) => CallResult::Shipped,
        None => CallResult::MayHaveRun,
    };
    let outcome = CallOutcome {
        result,
        healthy,
        roll_back: result != CallResult::Failed,
        why: None,
        at: epoch_ms(),
    };
    if let Some(call) = &deploy.call {
        if result == CallResult::Failed {
            core.ship().unspend(call);
        }
        person_calls::set_outcome(&core, call, outcome.clone());
    }
    if let Some(telling) = deploy.telling.clone() {
        let told = core.ship().state.locked().told.get(&telling.key).cloned();
        match told {
            Some(id) => person_calls::set_outcome(&core, &id, outcome),
            None => {
                let id = person_calls::told(&core, telling.raise, outcome);
                core.ship().state.locked().told.insert(telling.key, id);
            }
        }
    }
}

/// The person answered the call a production gate raised: each agent waiting on it hears
/// so, and what waits for it is no longer held. A no holds for those agents until the end of
/// the turn in which they hear it.
pub async fn gate_answered(core: &Arc<Core>, call: &PersonCall, typed: Option<String>) {
    let option = call
        .answer
        .as_ref()
        .and_then(|answer| answer.option.clone());
    let once = person_calls::key_of(core, &call.id).is_some_and(|key| key.contains(":action:"));
    {
        let mut state = core.ship().state.locked();
        let mut waiting: Vec<String> = (state.holds.iter())
            .filter(|hold| hold.token == call.id)
            .map(|hold| hold.agent_id.clone())
            .collect();
        waiting.extend(
            std::iter::once(&call.asker)
                .chain(&call.joined)
                .map(|asker| asker.agent_id.clone()),
        );
        waiting.sort();
        waiting.dedup();
        state.holds.retain(|hold| hold.token != call.id);
        if said_yes(option.as_deref()) && !once {
            for agent_id in waiting {
                state.covering.push((agent_id, call.id.clone(), false));
            }
        }
    }
    core.changed(What::Agents);
    let from = call.answer.as_ref().map(|answer| match answer.by {
        sikemux_view::calls::CallClient::Device { .. } => "a paired phone".to_owned(),
        _ => "Sikemux".to_owned(),
    });
    let keep =
        |place: places::Place, production: bool, level: Option<sikemux_view::places::Level>| {
            let project = call.project.clone();
            places::kept_by_tap(core, &project, place, production, level, from.clone())
        };
    if option.as_deref() == Some(FROM_NOW_ON) {
        if let Some(place) = place_of(core, call) {
            keep(place, true, Some(sikemux_view::places::Level::TellMe));
        }
    }
    let place = (call.place.as_ref()).map_or("the place", |place| place.name.as_str());
    let action = call.action.as_deref().unwrap_or("it");
    let mut text = match option.as_deref() {
        Some(FROM_NOW_ON) => format!(
            "The person said yes on the call \"{}\", and to just tell them from now on. Run `{action}` again now.",
            call.question
        ),
        _ if said_yes(option.as_deref()) && once => format!(
            "The person said yes on the call \"{}\". Run exactly `{action}` again now. The yes lets it run once, and stands until it runs without failing.",
            call.question
        ),
        _ if said_yes(option.as_deref()) => format!(
            "The person said yes on the call \"{}\". Run `{action}` again now; the yes covers this work's deploy to {place} until the end of this turn.",
            call.question
        ),
        _ => format!(
            "The person said no on the call \"{}\": {place} stays as it is with this work.",
            call.question
        ),
    };
    if let Some(typed) = typed {
        text = format!("{text}\nThey added: {typed}");
    }
    let askers = std::iter::once(&call.asker).chain(&call.joined);
    let mut told: Vec<&str> = Vec::new();
    for asker in askers {
        if told.contains(&asker.agent_id.as_str()) {
            continue;
        }
        told.push(&asker.agent_id);
        let delivered =
            (core.repo_above().messages_for_person)(core, asker.agent_id.clone(), text.clone())
                .await;
        let heard = delivered.is_ok_and(|told| {
            matches!(
                told.get("delivery").and_then(Value::as_str),
                Some("steered" | "newTurn")
            )
        });
        if heard {
            core.ship().turn_started(&asker.agent_id);
        }
    }
}

/// The place a call was raised at, as the project keeps it.
fn place_of(core: &Core, call: &PersonCall) -> Option<places::Place> {
    let shown = call.place.as_ref()?;
    let (kept, _) = places::current(core, &call.project);
    kept.into_iter().find(|place| place.shown.id == shown.id)
}

/// The person allows the held action once: a tool call runs and its agent hears
/// what it answered; a command its agent is told it may run again, until the end
/// of the turn in which it hears this.
pub async fn allow(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    allow_hold(core, hold).await;
    Ok(())
}

async fn allow_hold(core: &Arc<Core>, mut hold: Hold) {
    core.ship().remember(&hold, true);
    if let Some(call) = hold.call.take() {
        let allowed = format!("The person allowed `{}` once", hold.ship.what);
        run_held(core, hold, call, allowed);
        return;
    }
    let text = format!(
        "The person allowed `{}` once. Run exactly that again now; it may run once, until this turn ends.",
        hold.ship.what
    );
    let permit = Permit::Exactly {
        ship: hold.ship,
        cwd: hold.cwd,
    };
    permitted(core, hold.agent_id, permit, text, Some(hold.token)).await;
}

/// Runs a held tool call the person let go, without making them wait on it, and
/// sends its agent what it answered.
fn run_held(core: &Arc<Core>, hold: Hold, call: ToolCall, allowed: String) {
    if let Err(error) = person_calls::started(core, &hold.token) {
        person_calls::set_outcome(core, &hold.token, went(CallResult::NotShipped));
        let text = format!("{allowed}, but {error}. Call it again to be asked again.");
        let core = core.clone();
        tokio::spawn(async move {
            (core.repo_above().messages_for_person_waiting)(&core, hold.agent_id, text).await
        });
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let ran = as_caller(
            call.caller,
            (core.repo_above().plugins_run_agent_tool)(
                &core,
                &call.name,
                call.arguments,
                hold.deploy,
            ),
        )
        .await;
        let result = match ran {
            Ok(_) => CallResult::Shipped,
            Err(_) => CallResult::Failed,
        };
        person_calls::set_outcome(&core, &hold.token, went(result));
        let text = match ran {
            Ok(Value::String(answer)) => {
                format!("{allowed}, and Sikemux ran it for you. It answered:\n{answer}")
            }
            Ok(answer) => {
                format!("{allowed}, and Sikemux ran it for you. It answered:\n{answer:#}")
            }
            Err(error) => format!("{allowed}, and Sikemux ran it for you, but it failed: {error}"),
        };
        let _ = (core.repo_above().messages_for_person)(&core, hold.agent_id, text).await;
    });
}

/// Keeps what the person allowed and tells the agent, whose hearing of it in a
/// turn starts the time it holds for.
async fn permitted(
    core: &Arc<Core>,
    agent_id: String,
    permit: Permit,
    text: String,
    call: Option<String>,
) {
    core.ship().state.locked().allowed.push(Allowed {
        agent_id: agent_id.clone(),
        permit,
        call,
        heard: false,
    });
    let told = (core.repo_above().messages_for_person)(core, agent_id.clone(), text).await;
    let heard = told.is_ok_and(|told| {
        matches!(
            told.get("delivery").and_then(Value::as_str),
            Some("steered" | "newTurn")
        )
    });
    if heard {
        core.ship().turn_started(&agent_id);
    }
}

/// A released client's "always allow". No hold offers it: a place that isn't production
/// runs without asking, and a production place's level is set on the Playbook.
pub async fn let_go(_core: &Arc<Core>, _params: HoldParams) -> CoreResult<()> {
    Err("this action can't be always allowed; its place's level is set on the Playbook".into())
}

/// The person refuses the held action, and the agent is told not to take it.
pub async fn refuse(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    core.ship().remember(&hold, false);
    let text = format!("The person did not allow `{}`.", hold.ship.what);
    let _ = (core.repo_above().messages_for_person)(core, hold.agent_id, text).await;
    Ok(())
}

fn take(core: &Core, id: &str) -> CoreResult<Hold> {
    let hold = {
        let mut state = core.ship().state.locked();
        let at = (state.holds.iter())
            .position(|hold| hold.token == id)
            .ok_or("that action no longer waits on you")?;
        state.holds.remove(at)
    };
    core.changed(What::Agents);
    tell_calls(core);
    Ok(hold)
}

/// Puts each held action to the person as a production call, and closes the calls of those no
/// longer held.
fn tell_calls(core: &Core) {
    let state = core.ship().state.locked();
    let wanted = (state.holds.iter())
        .filter(|hold| hold.key.is_none())
        .map(|hold| (hold.token.clone(), held_call(core, hold)))
        .collect();
    person_calls::sync_holds(core, wanted);
    drop(state);
}

/// What came of a held action the person allowed: it ran, and how it ended where Sikemux saw.
fn went(result: CallResult) -> CallOutcome {
    CallOutcome {
        result,
        healthy: None,
        roll_back: false,
        why: None,
        at: epoch_ms(),
    }
}

/// A place's id made from its name: lowercase letters and digits, words joined by `-`.
fn slug(name: &str) -> String {
    let lower = name.to_lowercase();
    let words = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty());
    words.collect::<Vec<_>>().join("-")
}

fn held_call(core: &Core, hold: &Hold) -> PersonCall {
    let name = core.names().name_or_id(&hold.agent_id);
    let question = match (hold.ship.kind, hold.ship.writes_to(), &hold.environment) {
        (_, Some(place), _) => format!("Let @{name} write to {place} as you?"),
        (ShipKind::Push, ..) => format!("Let @{name} push its commits?"),
        (ShipKind::Merge, ..) => format!("Let @{name} merge its pull request?"),
        (ShipKind::Deploy, _, Some(environment)) => format!("Let @{name} deploy to {environment}?"),
        (ShipKind::Deploy, ..) => format!("Let @{name} deploy?"),
        (ShipKind::Publish, ..) => format!("Let @{name} publish a release?"),
        (ShipKind::ProductionWrite, ..) => {
            format!("Let @{name} change the production database?")
        }
    };
    let option = |id: &str, label: String, detail: Option<String>| CallOption {
        id: id.to_owned(),
        label,
        detail,
        picture: None,
    };
    let options = vec![
        option(person_calls::HOLD_ALLOW, "Allow it once".to_owned(), None),
        option(person_calls::HOLD_REFUSE, "Not now".to_owned(), None),
    ];
    PersonCall {
        id: hold.token.clone(),
        project: hold.project.clone(),
        kind: CallKind::Production,
        title: None,
        asker: CallAsker {
            agent_id: hold.agent_id.clone(),
            cli: (core.repo_above().questions_cli_of)(core, &hold.agent_id),
            name,
            chief: false,
        },
        joined: Vec::new(),
        about: None,
        page: None,
        more_pages: Vec::new(),
        shown_in: Vec::new(),
        link: None,
        question,
        context: hold.why.clone(),
        assumption: None,
        options,
        pick: None,
        against: None,
        evidence: Vec::new(),
        opened: Vec::new(),
        action: Some(hold.ship.what.clone()),
        brief: None,
        place: (hold.environment.clone()).map(|name| CallPlace {
            id: slug(&name),
            name,
        }),
        can_hand_back: false,
        multi: false,
        secret: false,
        raised_at: hold.at,
        state: CallState::Open,
        answer: None,
        outcome: None,
    }
}

#[cfg(test)]
mod tests {
    /// @Yanagi's writes to GitHub each asked "Change writes on GitHub's API with this work?",
    /// five cards the person couldn't tell apart.
    #[test]
    fn a_github_write_says_what_it_writes() {
        for (what, said) in [
            (
                "gh api repos/anomalyco/opencode/pulls/63648/comments/4558641882/replies -f body=Done: shortened it --jq .html_url",
                "Comment on anomalyco/opencode#63648",
            ),
            (
                "gh api -X PUT repos/nodelike/awesome-hermes-agents/contents/README.md -f message=Add Sikemux -f branch=main",
                "Update README.md in nodelike/awesome-hermes-agents",
            ),
            (
                "gh api repos/nodelike/awesome-opencode/git/refs -f ref=refs/heads/add-sikemux -f sha=c125252",
                "Create a branch in nodelike/awesome-opencode",
            ),
            (
                "gh api -X PATCH repos/nodelike/awesome-opencode/git/refs/heads/add-sikemux -f sha=c125252 -F force=true",
                "Move branch add-sikemux in nodelike/awesome-opencode",
            ),
            ("gh api -X POST repos/owner/repo/forks", "Fork owner/repo"),
            (
                "gh api -X POST repos/{owner}/{repo}/pages -f build_type=workflow",
                "Turn on GitHub Pages for nodelike/sikemux",
            ),
            (
                "gh api -X PUT repos/acme/web/rulesets/24037079 --input - --jq .conditions",
                "Change who may push what to acme/web",
            ),
            (
                "gh api -X POST repos/acme/web/deployments/77/statuses -f state=success",
                "Change acme/web on GitHub as you",
            ),
        ] {
            let here = || Some("nodelike/sikemux".to_owned());
            assert_eq!(super::github_write(what, here).as_deref(), Some(said), "{what}");
        }
    }
}
