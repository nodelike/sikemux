//! Actions agents take that leave this machine, held for the person. An agent
//! pushing, merging, deploying, publishing, writing to production, or writing to
//! a website or a connector is stopped
//! there, and the person is asked in the inbox. Allowing a Sikemux tool call runs
//! it and sends the agent what it answered; allowing a command lets exactly that
//! command run once, until the end of the turn in which the agent hears of it.
//! Where the person lets agents land work, pushes and merges go through. A
//! deploy to an environment the project marked safe goes without asking, and
//! one that goes is followed to its end and kept as evidence on the slice.

use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::ship::{HoldParams, WebWriteParams};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::vault::Caller;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::inbox::{ShipHold, ShipKind};
use sikemux_view::slice::Verdict;

use super::person_calls::{self, Raise};
use super::places::push::{self, Reach};
use super::places::{self, work, Goes, Judged};
use super::{Core, CoreResult};

mod gate;
mod hold;
#[cfg(test)]
mod tests;
mod tools;

pub(crate) use gate::with_reason;
pub use gate::{explain_held, gate_answered, key_of, take_back, FROM_NOW_ON};
pub use hold::{allow, refuse};
pub use tools::{check_tool, ended, tool_ran};

use crate::cli::protocol::HarnessRequest;
use crate::git::Git;
use crate::ship::{Lands, Looks, Ship, Target};
use gate::{
    call_for, checkout_of, gate, names_its_own, owned_here, passes, releases_on_tags,
    same_repository, tag_places, Asking,
};
use hold::hold;
use sikemux_calls::board::Landing;
use tools::{at_production_database, on_databases, production_databases};

/// More than an agent waits on at once; past this its oldest hold goes.
pub(super) const MAX_HOLDS_PER_AGENT: usize = 16;
/// The person's answers remembered, across every agent, while the core runs.
const MAX_ANSWERED: usize = 256;
const MAX_SPOKE: usize = 512;
pub(super) const MAX_TOLD: usize = 256;
const PRUNE_USED_AT: usize = 128;
/// Deploys let go whose end the core is still to hear of.
const MAX_GOING: usize = 64;
/// How long after the hook lets a command go its shim may still check that same run.
const SAME_RUN_MS: u64 = 60_000;
/// How long a deploy the shim follows may go unheard before its end counts as unseen.
const GOING_AT_MOST: Duration = Duration::from_secs(60 * 60);

pub struct Ships {
    pub(super) state: Mutex<State>,
    /// When this core started: a production yes given before then passes nothing.
    pub(super) born: u64,
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
pub(super) struct State {
    next: u64,
    pub(super) holds: Vec<Hold>,
    pub(super) allowed: Vec<Allowed>,
    answered: VecDeque<Answered>,
    going: VecDeque<(u64, Deploy)>,
    /// Production yeses already used, by call: one that covers an action once, or a deploy's
    /// whose turn ended.
    pub(super) used: HashSet<String>,
    /// When a yes for one action was taken by a check that doesn't see the action end, such
    /// as the hook before a shell command, by call. The shim that then runs it may take it too.
    unfollowed: HashMap<String, u64>,
    /// A deploy's yes each agent heard, which covers that work's deploy until the end of the
    /// turn it heard it in: the agent, the call, and whether that turn began.
    pub(super) covering: Vec<(String, String, bool)>,
    /// When each agent last heard from the person in its own chat or terminal, after which a
    /// no it was given may be asked again.
    spoke: HashMap<String, u64>,
    /// The settled call each deploy at a place at "tell me" was told by, by its key.
    pub(super) told: HashMap<String, String>,
    told_order: VecDeque<String>,
}

/// A change at a place at "tell me", told once its deploy ends.
#[derive(Clone)]
pub struct Telling {
    pub(super) key: String,
    pub(super) raise: Raise,
}

/// A deploy let go toward one of the project's environments.
#[derive(Clone)]
pub struct Deploy {
    pub(super) agent_id: String,
    pub(super) project: String,
    pub(super) environment: String,
    /// The commit its checkout was at as it went, which is what it deployed.
    pub(super) head: Option<String>,
    /// The call whose yes let it go, which hears how it ended.
    pub(super) call: Option<String>,
    /// The person is told of it once it ends.
    pub(super) telling: Option<Telling>,
}

#[derive(Clone)]
pub struct Answered {
    pub agent_id: String,
    pub what: String,
    pub allowed: bool,
    pub at: u64,
}

/// An agent's action waiting on the person.
pub(super) struct Hold {
    /// What clients name it by, which can't be guessed from another hold's.
    pub(super) token: String,
    pub(super) agent_id: String,
    pub(super) project: String,
    pub(super) ship: Ship,
    pub(super) cwd: PathBuf,
    pub(super) at: u64,
    /// The environment it lands in and why it waits, in a project with environments.
    pub(super) environment: Option<String>,
    pub(super) why: Option<String>,
    /// Why its agent needs it and what is lost, in the agent's words.
    pub(super) because: Option<String>,
    /// The Sikemux tool call held, which the core runs itself once the person allows it.
    pub(super) call: Option<ToolCall>,
    /// The deploy it is, to follow once the person lets it go.
    pub(super) deploy: Option<Deploy>,
    /// The piece of work at a production place it waits for a yes on, whose call it is.
    pub(super) key: Option<String>,
}

/// A Sikemux tool an agent called, as it called it.
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
    pub caller: Caller,
}

/// An action the person allowed the agent to take once.
pub(super) struct Allowed {
    pub(super) agent_id: String,
    pub(super) permit: Permit,
    /// The call it was allowed on, which hears that it ran.
    pub(super) call: Option<String>,
    /// The agent has heard it may, so the approval ends with its turn.
    pub(super) heard: bool,
}

pub(super) enum Permit {
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

    pub(super) fn remember(&self, hold: &Hold, allowed: bool) {
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
        let mut state = self.state.locked();
        if !state.spoke.contains_key(agent_id) && state.spoke.len() >= MAX_SPOKE {
            if let Some(oldest) = state
                .spoke
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(id, _)| id.clone())
            {
                state.spoke.remove(&oldest);
            }
        }
        state.spoke.insert(agent_id.to_owned(), epoch_ms());
    }

    pub fn spoke_at(&self, agent_id: &str) -> Option<u64> {
        self.state.locked().spoke.get(agent_id).copied()
    }

    pub fn agent_closed(&self, agent_id: &str) {
        let mut state = self.state.locked();
        state.spoke.remove(agent_id);
        state.allowed.retain(|allowed| allowed.agent_id != agent_id);
        let ended: Vec<String> = (state.covering.iter())
            .filter(|(agent, _, _)| agent == agent_id)
            .map(|(_, call, _)| call.clone())
            .collect();
        state.covering.retain(|(agent, _, _)| agent != agent_id);
        for call in ended {
            if !state.covering.iter().any(|(_, other, _)| *other == call) {
                state.used.insert(call);
            }
        }
    }

    pub fn prune_settled(&self, live: &HashSet<String>) {
        let now = epoch_ms();
        let mut state = self.state.locked();
        state.used.retain(|id| live.contains(id));
        state
            .unfollowed
            .retain(|id, at| live.contains(id) && now < at.saturating_add(SAME_RUN_MS));
        state.told.retain(|_, id| live.contains(id));
        let mut order = std::mem::take(&mut state.told_order);
        order.retain(|key| state.told.contains_key(key));
        state.told_order = order;
    }

    pub(super) fn record_told(&self, key: String, id: String) {
        let mut state = self.state.locked();
        if state.told.insert(key.clone(), id).is_none() {
            state.told_order.push_back(key);
            while state.told.len() > MAX_TOLD {
                let Some(oldest) = state.told_order.pop_front() else {
                    break;
                };
                state.told.remove(&oldest);
            }
        }
    }

    /// Takes the yes of `call` for one action: once only for an action it covers once, so two
    /// tries at the same moment can't both pass. A check that `follows` the action to its end
    /// shortly after one that doesn't is the same run checked twice, by the hook and the shim.
    fn claim(&self, call: &str, once: bool, follows: bool) -> bool {
        let now = epoch_ms();
        let mut state = self.state.locked();
        state
            .unfollowed
            .retain(|_, at| now < at.saturating_add(SAME_RUN_MS));
        if state.used.contains(call) {
            let taken = state.unfollowed.remove(call);
            return follows && taken.is_some_and(|at| now < at.saturating_add(SAME_RUN_MS));
        }
        if once {
            state.used.insert(call.to_owned());
            if !follows {
                state.unfollowed.insert(call.to_owned(), now);
            }
        }
        true
    }

    /// The action a yes covered once failed, so the yes stands for its retry.
    pub(super) fn unspend(&self, call: &str) {
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
    check_call(core, agent_id, ship, cwd, None, false, None)
}

/// What an agent tried to run when the gate put it to the person, kept with the call so that a
/// core started later can tell whether a yes to it still counts. A browser's write is not kept:
/// what decides it, its address in full, may hold secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    ship: Ship,
    cwd: PathBuf,
    /// A Sikemux tool call rather than a command.
    tool: bool,
}

/// Whether a yes to the gate's open call `id` would let its agent's action run, were it run again
/// now. Runs git and may ask GitHub, so it blocks.
pub fn yes_counts(core: &Core, id: &str) -> bool {
    let Some(call) = person_calls::open_call(core, id) else {
        return false;
    };
    let Some(Attempt { ship, cwd, tool }) = person_calls::attempt_of(core, id) else {
        return false;
    };
    match read(core, &call.asker.agent_id, ship, &cwd, tool, None) {
        Ok(Reading::Judged(judged)) if matches!(judged.judged.goes, Goes::Asks) => {
            let unread = judged.merging && judged.sends.is_none();
            let key = judged.key().map(|(key, _)| key);
            unread || (key.is_some() && key == person_calls::key_of(core, id))
        }
        _ => false,
    }
}

/// What the gate makes of an action before anyone is asked.
enum Reading {
    /// Nothing for the gate to ask.
    Passes,
    /// A push or merge that lands as the project says, run from `cwd`.
    Lands {
        project: String,
        ship: Ship,
        cwd: PathBuf,
    },
    Judged(Box<Reads>),
}

/// An action as the gate reads it, and what its project's places make of it.
struct Reads {
    project: String,
    ship: Ship,
    cwd: PathBuf,
    merging: bool,
    /// A push that may do more than send the checkout's commits to its own branch.
    exact: bool,
    tool: bool,
    sends: Option<String>,
    /// The repository it changes when it is a push or merge through git with nothing else to it.
    plain: Option<String>,
    judged: Judged,
}

impl Reads {
    /// What its yes is kept under at the place that decided, and whether that covers it once.
    fn key(&self) -> Option<(String, bool)> {
        // A pull request's commits needn't be the checkout's, so a yes to a merge is kept for
        // that merge rather than for the work in the checkout.
        let work = (!self.merging && !self.exact).then(|| work::of(&self.cwd));
        Some(key_of(
            self.judged.place.as_ref()?,
            &self.ship,
            work.flatten().as_deref(),
            self.sends.as_deref(),
            self.tool,
        ))
    }
}

/// Reads what `ship`, run by `agent_id` from `cwd`, does and where it lands, as [`check`] says,
/// or why it may not run at all.
fn read(
    core: &Core,
    agent_id: &str,
    ship: Ship,
    cwd: &Path,
    tool: bool,
    addresses: Option<&str>,
) -> Result<Reading, String> {
    let mut project = (core.names().get(agent_id).map(|named| named.project))
        .filter(|project| !project.is_empty())
        .unwrap_or_else(|| core.workspaces().project_of(cwd));
    let mut ship = ship;
    let mut api_lands = None;
    let mut api_merging = None;
    if let Some(Target::GitHub { repo, lands }) = ship.target.as_ref().map(Target::bare) {
        let ours = [cwd, Path::new(&project)]
            .into_iter()
            .any(|root| push::is_remote_of(repo, root));
        match lands.clone().filter(|_| ours) {
            Some(Lands::Merge { pull }) => {
                api_merging = Some(push::api_merging(&pull, repo));
                ship.kind = ShipKind::Merge;
            }
            Some(lands) => {
                api_lands = Some(lands);
                ship.kind = ShipKind::Push;
            }
            None if owned_here(repo, cwd, &project) => return Ok(Reading::Passes),
            None => {}
        }
    }
    let pushed_in = push::run_in(&ship.what, cwd);
    let merging = (ship.kind == ShipKind::Merge)
        .then(|| api_merging.unwrap_or_else(|| push::merging(&ship.what)));
    let mut plain = match ship.kind {
        _ if tool => None,
        ShipKind::Push => push::plain(&ship.what, cwd),
        ShipKind::Publish => push::one_tag(&ship.what, cwd),
        _ => None,
    };
    let pushed = ship.kind == ShipKind::Push;
    let exact = pushed && plain.is_none();
    let sent_from_here = match &merging {
        _ if api_lands.is_some() => false,
        Some(merging) => merging.repo.is_none(),
        None => ship.kind == ShipKind::Push,
    };
    if sent_from_here && !same_repository(&pushed_in, &project) {
        let Some(top) = sikemux_process::files::repo_root(&pushed_in) else {
            return Ok(Reading::Passes);
        };
        project = core.workspaces().project_of(top);
    }
    let mut sends = None;
    let from = [project.clone(), checkout_of(&pushed_in, &project)];
    let from_one = |deploys: &dyn Fn(&str) -> bool| from.iter().find(|root| deploys(root)).cloned();
    let on_vercel = |root: &str| places::found::vercel_deploys_pushes(Path::new(root));
    let to_vercel = |ship: Ship| Ship {
        kind: ShipKind::Deploy,
        target: Some(Target::Vercel { production: true }),
        what: ship.what,
    };
    if let Some(merging) = &merging {
        let deployed = from_one(&|root| on_vercel(root) && merging.is_into(Path::new(root)));
        if let Some(deployed) = deployed {
            let (reach, head, repository) = merging.reach(Path::new(&deployed), &pushed_in);
            if reach == Reach::Project {
                plain = repository.filter(|_| !tool);
                project = deployed;
                sends = head;
                ship = to_vercel(ship);
            }
        }
    }
    if ship.kind == ShipKind::Push {
        let (reach, may_reach_project) = match &api_lands {
            Some(lands) => {
                let reach = push::api_reach(lands, Path::new(&project));
                let any = reach == Reach::Tags(Vec::new());
                (reach, any)
            }
            None => (
                push::reach(&ship.what, cwd),
                push::may_reach_project(&ship.what, cwd),
            ),
        };
        let published = |root: &str| tag_places(core, root) || releases_on_tags(core, root);
        if api_lands == Some(Lands::Unread)
            && from_one(&|root| published(root) || on_vercel(root)).is_some()
        {
            return Err(
                "Not run: Sikemux can't read which branch or tag this changes. Put it in the command, or in a file given with --input or @, and run it again."
                    .to_owned(),
            );
        }
        match reach {
            Reach::Branches => return Ok(Reading::Passes),
            Reach::Project => {
                if let Some(deployed) = from_one(&on_vercel) {
                    project = deployed;
                    ship = to_vercel(ship);
                }
            }
            Reach::Tags(names) => {
                if (core.repo_above().board_landing)(core, &project) != Landing::Core {
                    return Ok(Reading::Lands {
                        project,
                        ship,
                        cwd: cwd.to_owned(),
                    });
                }
                sends = push::tag_commits(&names, &pushed_in);
                let deployed = from_one(&on_vercel);
                plain = (!tool && !names.is_empty())
                    .then(|| push::one_tag(&ship.what, cwd))
                    .flatten();
                let name = match names.as_slice() {
                    [name] if !may_reach_project || deployed.is_none() => name.clone(),
                    _ => String::new(),
                };
                project = from_one(&published).unwrap_or(project);
                let target = match (tag_places(core, &project), releases_on_tags(core, &project)) {
                    (true, _) => Some(Target::Tag { name }),
                    (false, true) => Some(Target::Package {
                        registry: places::GITHUB_RELEASES.to_owned(),
                    }),
                    (false, false) => None,
                };
                match (target, deployed) {
                    (Some(target), _) => {
                        ship = Ship {
                            kind: ShipKind::Publish,
                            target: Some(target),
                            what: ship.what,
                        };
                    }
                    (None, Some(deployed)) if may_reach_project => {
                        project = deployed;
                        ship = to_vercel(ship);
                    }
                    (None, _) => {}
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
    if let Some(Target::Workflow { file, .. }) = ship.target.as_ref().map(Target::bare) {
        if let Some(starter) = places::found::starter_of(Path::new(&project), file) {
            return Err(format!(
                "Not run: the {starter} workflow starts {file} by itself, so nobody starts it by hand. If it hasn't, read {starter}'s latest run for why."
            ));
        }
    }
    let cwd = pushed_in.as_path();
    if matches!(ship.kind, ShipKind::Push | ShipKind::Merge) {
        if (core.repo_above().board_landing)(core, &project) == Landing::Core {
            return Ok(Reading::Passes);
        }
        return Ok(Reading::Lands {
            project,
            ship,
            cwd: cwd.to_owned(),
        });
    }
    if exact {
        let sent = (api_lands.is_none()).then(|| push::sent(&ship.what, cwd));
        let to = (api_lands.is_none()).then(|| push::destination(&ship.what, cwd));
        let parts = [sends, sent, work::of(cwd), to];
        sends = Some(parts.into_iter().flatten().collect());
    } else if let Some(repository) = &plain {
        let sent = pushed.then(|| push::sent(&ship.what, cwd));
        let parts: Vec<String> = [sends, sent].into_iter().flatten().collect();
        sends = Some(format!("{} to {repository}", parts.join(" ")));
    } else if ship.kind == ShipKind::Deploy && names_its_own(&ship.what) {
        sends = Some(ship.what.clone());
    }
    let name = core.names().name_or_id(agent_id);
    let (project, judged) =
        places::judge_anywhere(core, &project, &ship, &name, addresses, plain.as_deref());
    Ok(Reading::Judged(Box::new(Reads {
        merging: merging.is_some(),
        cwd: cwd.to_owned(),
        project,
        ship,
        exact,
        tool,
        sends,
        plain,
        judged,
    })))
}

/// [`check`] for a Sikemux tool call, which a hold keeps to run once the person allows it.
pub(super) fn check_call(
    core: &Core,
    agent_id: &str,
    ship: Ship,
    cwd: &Path,
    call: Option<ToolCall>,
    follows: bool,
    addresses: Option<&str>,
) -> Result<Option<Deploy>, String> {
    let attempt = addresses.is_none().then(|| Attempt {
        ship: ship.clone(),
        cwd: cwd.to_owned(),
        tool: call.is_some(),
    });
    let reads = match read(core, agent_id, ship, cwd, call.is_some(), addresses)? {
        Reading::Passes => return Ok(None),
        Reading::Lands { project, ship, cwd } => {
            return hold(core, agent_id, &project, ship, &cwd, call, None)
        }
        Reading::Judged(reads) => reads,
    };
    let keyed = match reads.judged.goes {
        Goes::Tells | Goes::Asks => reads.key(),
        Goes::Freely | Goes::Unplaced => None,
    };
    let Reads {
        project,
        ship,
        cwd,
        merging,
        plain,
        judged,
        ..
    } = *reads;
    let cwd = cwd.as_path();
    let place = judged.place.as_ref().map(|place| place.name().to_owned());
    let deploy = |call: Option<String>, telling: Option<Telling>| {
        let followed = ship.kind == ShipKind::Deploy || call.is_some() || telling.is_some();
        place.as_ref().filter(|_| followed).map(|name| Deploy {
            head: (Git::quick(cwd).trimmed(&["rev-parse", "HEAD"]).ok()).filter(|_| !merging),
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
            let ((key, _), told) = keyed.zip(judged.place.as_ref()).expect("a place tells");
            let raise = call_for(core, agent_id, &project, &ship, cwd, told, None);
            Ok(deploy(None, Some(Telling { key, raise })))
        }
        Goes::Asks => {
            if core.ship().state.locked().used.len() >= PRUNE_USED_AT {
                let live = core.person_calls().known_ids();
                core.ship().prune_settled(&live);
            }
            #[expect(
                clippy::expect_used,
                reason = "a judgement that asks always names its place"
            )]
            let ((key, once), gated) = keyed.zip(judged.place.clone()).expect("a place asks");
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
                attempt,
                routine: plain.filter(|_| places::judge::routine(&gated, &ship)),
            };
            gate(core, agent_id, &project, ship, cwd, asking, judged.why)
        }
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
        check_call(&checking, &agent_id, ship, &cwd, None, follows, None)
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
        let cwd = Path::new(&params.project);
        let addresses = Some(params.with_queries.as_str());
        check_call(&checking, &params.agent, ship, cwd, None, false, addresses)
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
