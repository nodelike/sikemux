//! Actions agents take that leave this machine, held for the person. An agent
//! pushing, merging, deploying, publishing, writing to production, or writing to
//! a website or a connector is stopped
//! there, and the person is asked in the inbox. Allowing a Sikemux tool call runs
//! it and sends the agent what it answered; allowing a command lets exactly that
//! command run once, until the end of the turn in which the agent hears of it.
//! Where the person lets agents land work, pushes and merges go through. A
//! deploy to an environment the project marked safe goes without asking, and
//! one that goes is followed to its end and kept as evidence on the slice.

use crate::server::changed::What;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::vault::{as_caller, Caller};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::inbox::{ShipHold, ShipKind};
use sikemux_view::slice::Verdict;

use super::board::environments::{Always, Goes};
use super::board::{self, auto::Landing, DeployRan};
use super::connection::blocking;
use super::plugins::Provided;
use super::repo_events::RepoEvent;
use super::{messages, person_calls, plugins, questions, Core, CoreResult};
use crate::cli::protocol::HarnessRequest;
use crate::git::Git;
use crate::ship::{Looks, Ship, Target, ToolShip};
use sikemux_view::calls::{
    CallAsker, CallKind, CallOption, CallOutcome, CallPlace, CallResult, CallState, PersonCall,
};

/// More than an agent waits on at once; past this its oldest hold goes.
const MAX_HOLDS_PER_AGENT: usize = 16;
/// The person's answers remembered, across every agent, while the core runs.
const MAX_ANSWERED: usize = 256;
const DATABASE: &str = "sikemux.database";
const RUNDECK: &str = "sikemux.rundeck";
const VERCEL: &str = "sikemux.vercel";
/// Deploys let go whose end the core is still to hear of.
const MAX_GOING: usize = 64;
/// How often a long deploy is looked at, and how long a look can take, before
/// the core stops following it and says it did not see the end.
const FOLLOW_LOOKS: usize = 60;
const FOLLOW_EVERY: Duration = Duration::from_secs(15);
const HEALTH_TRIES: usize = 6;
const HEALTH_EVERY: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
/// The end of what a deploy printed that its evidence keeps.
const MAX_OUTPUT_CHARS: usize = 4000;

#[derive(Default)]
pub(crate) struct Ships {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next: u64,
    holds: Vec<Hold>,
    allowed: Vec<Allowed>,
    answered: VecDeque<Answered>,
    going: VecDeque<(u64, Deploy)>,
}

/// A deploy let go toward one of the project's environments.
#[derive(Clone)]
pub(crate) struct Deploy {
    agent_id: String,
    project: String,
    environment: String,
    what: String,
    /// Why it went or how it was let go, as its evidence opens.
    why: String,
    by_person: bool,
    /// The commit its checkout was at as it went, which is what it deployed.
    head: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Answered {
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
    /// What the person could always allow from now on.
    let_go: Option<Always>,
    /// The Sikemux tool call held, which the core runs itself once the person allows it.
    call: Option<ToolCall>,
    /// The deploy it is, to follow once the person lets it go.
    deploy: Option<Deploy>,
}

/// A Sikemux tool an agent called, as it called it.
pub(crate) struct ToolCall {
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
    /// One deploy to this environment of the project, by passing its gate.
    Environment { project: String, name: String },
}

/// What waits on the person, as the inbox lists it.
pub(crate) struct Waiting {
    pub hold: ShipHold,
    /// The site or connector it writes to, when that is what it does.
    pub writes_to: Option<String>,
    pub agent_id: String,
    pub project: String,
    pub at: u64,
}

impl Ships {
    /// Whether an action of the agent's waits on the person.
    pub(crate) fn holds(&self, agent_id: &str) -> bool {
        (self.state.locked().holds.iter()).any(|hold| hold.agent_id == agent_id)
    }

    /// The actions waiting on the person, oldest first.
    pub(crate) fn waiting(&self) -> Vec<Waiting> {
        (self.state.locked().holds.iter())
            .map(|hold| Waiting {
                hold: ShipHold {
                    id: hold.token.clone(),
                    kind: hold.ship.kind,
                    what: hold.ship.what.clone(),
                    environment: hold.environment.clone(),
                    why: hold.why.clone(),
                    let_go: hold.let_go.as_ref().map(|always| always.covers.clone()),
                    let_go_in: (hold.let_go.as_ref())
                        .and_then(|always| always.folder.as_deref())
                        .and_then(|folder| Path::new(folder).file_name())
                        .map(|name| name.to_string_lossy().into_owned()),
                },
                writes_to: hold.ship.writes_to().map(str::to_owned),
                agent_id: hold.agent_id.clone(),
                project: hold.project.clone(),
                at: hold.at,
            })
            .collect()
    }

    /// What the person answered of the actions held since the core started, oldest first.
    pub(crate) fn answered(&self) -> Vec<Answered> {
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
    pub(crate) fn turn_started(&self, agent_id: &str) {
        for allowed in
            (self.state.locked().allowed.iter_mut()).filter(|allowed| allowed.agent_id == agent_id)
        {
            allowed.heard = true;
        }
    }

    /// The agent's turn ended, and with it what it was allowed in it.
    pub(crate) fn turn_ended(&self, agent_id: &str) {
        (self.state.locked().allowed)
            .retain(|allowed| allowed.agent_id != agent_id || !allowed.heard);
    }
}

/// Whether the agent `agent_id` may take `ship` from `cwd` now: as the
/// project's environments say for a deploy; once the person allowed exactly
/// that; or where they let agents land work, for a push or a merge. Otherwise
/// it is held, and the agent is told why. What goes toward an environment comes
/// back to be followed to its end.
pub(crate) fn check(
    core: &Core,
    agent_id: &str,
    ship: Ship,
    cwd: &Path,
) -> Result<Option<Deploy>, String> {
    check_call(core, agent_id, ship, cwd, None)
}

/// [`check`] for a Sikemux tool call, which a hold keeps to run once the person allows it.
fn check_call(
    core: &Core,
    agent_id: &str,
    ship: Ship,
    cwd: &Path,
    call: Option<ToolCall>,
) -> Result<Option<Deploy>, String> {
    let project = core.workspaces.project_of(cwd);
    let lands = matches!(ship.kind, ShipKind::Push | ShipKind::Merge)
        && board::landing(core, &project) == Landing::Core;
    if lands {
        return Ok(None);
    }
    let judged = board::judge_ship(core, &project, &ship);
    let environment = judged.environment.clone();
    let why = judged.why.clone();
    let deploy = |by_person: bool| {
        let deploying = environment
            .as_ref()
            .filter(|_| ship.kind == ShipKind::Deploy);
        deploying.map(|name| Deploy {
            head: Git::quick(cwd).trimmed(&["rev-parse", "HEAD"]).ok(),
            agent_id: agent_id.to_owned(),
            project: project.clone(),
            environment: name.clone(),
            what: ship.what.clone(),
            why: match by_person {
                true => "You allowed it.".to_owned(),
                false => why.clone().unwrap_or_default(),
            },
            by_person,
        })
    };
    match judged.goes {
        Goes::Freely => return Ok(deploy(false)),
        Goes::OnceChecked => {
            let head = Git::quick(cwd).trimmed(&["rev-parse", "HEAD"]).ok();
            return match board::check_stands(core, &project, agent_id, head.as_deref()) {
                Ok(()) => Ok(deploy(false)),
                Err(why) => Err(format!(
                    "Not yet: `{}` lands in {}, which goes once the project's check passes, and {why}. Run it again once it has; the person needn't be asked.",
                    ship.what,
                    environment.as_deref().unwrap_or_default()
                )),
            };
        }
        Goes::Holds => {}
    }
    let then = match call {
        Some(_) => "Do not call it again: once they allow it, Sikemux runs it and sends you what it answered.",
        None => "You will hear when they allow it, and then run exactly this again.",
    };
    let mut said = format!(
        "Held for the person: `{}` {}, so Sikemux asked them first: they answer it on its card in your chat or on Goals Home in Sikemux, on the notch, or in your chat on their phone. {then} Go on with other work meanwhile.",
        ship.what,
        leaves(&ship)
    );
    if let Some(why) = &judged.why {
        said = format!("{said} {why}");
    }
    let let_go = deploy(true);
    let unasked = board::always_offer(core, &project, &ship);
    {
        let mut state = core.ship.state.locked();
        let same =
            |agent: &str, held: &Ship, at: &Path| agent == agent_id && *held == ship && at == cwd;
        let permits = |allowed: &Allowed| match &allowed.permit {
            Permit::Exactly {
                ship: held,
                cwd: at,
            } => same(&allowed.agent_id, held, at),
            Permit::Environment {
                project: there,
                name,
            } => {
                allowed.agent_id == agent_id
                    && *there == project
                    && environment.as_ref() == Some(name)
            }
        };
        if let Some(at) = state.allowed.iter().position(permits) {
            let ran = state.allowed.remove(at).call;
            drop(state);
            if let Some(call) = ran {
                person_calls::set_outcome(core, &call, went(CallResult::Shipped));
            }
            return Ok(let_go);
        }
        if state
            .holds
            .iter()
            .any(|hold| same(&hold.agent_id, &hold.ship, &hold.cwd))
        {
            return Err(said);
        }
        let held = state
            .holds
            .iter()
            .filter(|hold| hold.agent_id == agent_id)
            .count();
        if held >= MAX_HOLDS_PER_AGENT {
            if let Some(oldest) = state
                .holds
                .iter()
                .position(|hold| hold.agent_id == agent_id)
            {
                state.holds.remove(oldest);
            }
        }
        state.holds.push(Hold {
            token: uuid::Uuid::new_v4().simple().to_string(),
            agent_id: agent_id.to_owned(),
            project,
            cwd: cwd.to_owned(),
            at: epoch_ms(),
            environment: judged.environment,
            why: judged.why,
            let_go: unasked,
            deploy: call.as_ref().and(let_go),
            call,
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

/// What the shim and the hook ask before an action leaving this machine runs:
/// whether it may, and if not, what the agent is told. A program no agent of
/// this core runs, as the person's own shell does, is never held. Where the asker
/// `follows` the program to its end it hears a `went` to answer with; otherwise
/// what went is kept at once, as not seen to end.
pub(crate) async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let Some(agent_id) = request.agent_id.clone() else {
        return Ok(json!({ "allowed": true }));
    };
    let kind: ShipKind = serde_json::from_value(request.params["kind"].clone())
        .map_err(|_| "kind must be what the action does")?;
    let what = (request.params["what"].as_str())
        .ok_or("what must be the action's command")?
        .to_owned();
    let target = serde_json::from_value::<Option<Target>>(request.params["target"].clone())
        .ok()
        .flatten();
    let follows = request.params["follows"] == true;
    let cwd = PathBuf::from(&request.project);
    let ship = Ship { kind, what, target };
    let checking = core.clone();
    let checked = tokio::task::spawn_blocking(move || check(&checking, &agent_id, ship, &cwd))
        .await
        .map_err(|error| error.to_string())?;
    Ok(match checked {
        Ok(None) => json!({ "allowed": true }),
        Ok(Some(deploy)) if follows => {
            let mut state = core.ship.state.locked();
            state.next += 1;
            let went = state.next;
            if state.going.len() >= MAX_GOING {
                state.going.pop_front();
            }
            state.going.push_back((went, deploy));
            json!({ "allowed": true, "went": went.to_string() })
        }
        Ok(Some(deploy)) => {
            let core = core.clone();
            tokio::spawn(async move { ended(core, deploy, None, String::new()).await });
            json!({ "allowed": true })
        }
        Err(said) => json!({ "allowed": false, "said": said }),
    })
}

/// Whether the process `command`, which agent `agent_id` starts in `cwd` with `env` set,
/// may run: each of its commands that leaves this machine, as [`check`] says. The core
/// starts the process outside the agent's own, so the shim inside it can't tell it is the
/// agent's.
pub(crate) async fn check_process(
    core: &Arc<Core>,
    agent_id: &str,
    command: &str,
    env: &[(String, String)],
    cwd: &Path,
) -> Result<(), String> {
    let (checking, agent_id, command, env, cwd) = (
        core.clone(),
        agent_id.to_owned(),
        command.to_owned(),
        env.to_vec(),
        cwd.to_owned(),
    );
    let going = tokio::task::spawn_blocking(move || {
        let extra = crate::git_guard::shim::project_ships(&cwd);
        let set = |name: &str| {
            (env.iter().rev())
                .find(|(set, _)| set == name)
                .map(|(_, value)| value.clone())
                .or_else(|| sikemux_process::user_environment::var(name))
        };
        let ships = crate::ship::of_command(&command, &extra, Looks::Every, &cwd, &set);
        let mut going = Vec::new();
        for ship in ships {
            let checked = check(&checking, &agent_id, ship, &cwd)
                .map_err(|said| format!("{said} Start the process with a new idempotencyKey."))?;
            going.extend(checked);
        }
        Ok::<_, String>(going)
    })
    .await
    .map_err(|error| error.to_string())??;
    for deploy in going {
        let core = core.clone();
        tokio::spawn(async move { ended(core, deploy, None, String::new()).await });
    }
    Ok(())
}

/// How a deploy the shim followed ended, by the `went` the core gave it.
pub(crate) fn ran_call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let went: u64 = (request.params["went"].as_str())
        .and_then(|went| went.parse().ok())
        .ok_or("went must be what the check answered with")?;
    let deploy = {
        let mut state = core.ship.state.locked();
        let at = (state.going.iter())
            .position(|(id, deploy)| {
                *id == went && Some(&deploy.agent_id) == request.agent_id.as_ref()
            })
            .ok_or("no such deploy is going")?;
        state.going.remove(at).map(|(_, deploy)| deploy)
    };
    let exit = request.params["exit"].as_i64();
    let verdict = match exit {
        Some(0) => Verdict::Passed,
        _ => Verdict::Failed,
    };
    let output = exit.map_or_else(
        || "It was stopped before it ended.".to_owned(),
        |code| format!("It exited with {code}."),
    );
    if let Some(deploy) = deploy {
        let core = core.clone();
        tokio::spawn(async move { ended(core, deploy, Some(verdict), output).await });
    }
    Ok(json!({}))
}

/// Where the call of Sikemux tool `name` lands, from what the tool's own plugin
/// says. None when it can't say.
async fn tool_target(core: &Arc<Core>, name: &str, arguments: &Value) -> Option<Target> {
    match name {
        "rundeck_run" => {
            let place = plugins::call(core, RUNDECK, "agentJobPlace", arguments.clone())
                .await
                .ok()?;
            Some(Target::RundeckJob {
                path: place["path"].as_str()?.to_owned(),
                production: place["production"].as_bool()?,
            })
        }
        name if name.starts_with("jira_") => Some(Target::Connector {
            name: "Jira".to_owned(),
        }),
        name if name.starts_with("bitbucket_") && name != "bitbucket_merge" => {
            Some(Target::Connector {
                name: "Bitbucket".to_owned(),
            })
        }
        "vercel_promote" => Some(Target::Vercel { production: true }),
        "vercel_redeploy" => {
            let target = match arguments["target"].as_str() {
                Some(target) => target.to_owned(),
                None => {
                    let read = plugins::call(core, VERCEL, "deployment", arguments.clone())
                        .await
                        .ok()?;
                    read["target"].as_str()?.to_owned()
                }
            };
            match target.as_str() {
                "production" => Some(Target::Vercel { production: true }),
                "preview" => Some(Target::Vercel { production: false }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether an agent's call to the Sikemux tool `name` may run now, as
/// [`check`] says for what it does.
pub(crate) async fn check_tool(
    core: &Arc<Core>,
    request: &HarnessRequest,
    name: &str,
    arguments: &Value,
) -> Result<Option<Deploy>, String> {
    let (Some(agent_id), Some(when)) = (&request.agent_id, crate::ship::of_tool(name)) else {
        return Ok(None);
    };
    let kind = match when {
        ToolShip::Always(kind) => kind,
        ToolShip::ProductionDatabase => {
            let asked = plugins::call(core, DATABASE, "agentWritesProduction", arguments.clone())
                .await
                .map_err(|failure| plugins::explain(&failure))?;
            if asked != true {
                return Ok(None);
            }
            ShipKind::ProductionWrite
        }
    };
    let what = format!("{name} {arguments}");
    let target = tool_target(core, name, arguments).await;
    let (checking, agent_id, cwd) = (core.clone(), agent_id.clone(), request.project.clone());
    let call = ToolCall {
        name: name.to_owned(),
        arguments: arguments.clone(),
        caller: plugins::caller_of(core, request),
    };
    tokio::task::spawn_blocking(move || {
        check_call(
            &checking,
            &agent_id,
            Ship { kind, what, target },
            Path::new(&cwd),
            Some(call),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Deserialize, Serialize)]
pub struct WebWriteParams {
    pub agent: String,
    pub project: String,
    pub what: String,
    pub origin: String,
}

/// Whether an agent's browser may send the write it is about to make to `origin`,
/// as [`check`] says: none when it may, otherwise what the agent is told.
pub(crate) async fn check_web_write(
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
    Ok(checked.err())
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
pub(crate) fn tool_ran(core: &Arc<Core>, deploy: Deploy, tool: &str, answer: Result<&Value, &str>) {
    let (core, tool) = (core.clone(), tool.to_owned());
    let answer = answer.cloned().map_err(str::to_owned);
    tokio::spawn(async move {
        let (verdict, output) = match answer {
            Err(error) => (Some(Verdict::Failed), error),
            Ok(answer) => followed(&core, &tool, answer).await,
        };
        ended(core, deploy, verdict, output).await;
    });
}

/// Reads a deploy tool's run until it ends, within `FOLLOW_LOOKS`. No verdict
/// when the core did not see the end.
async fn followed(core: &Arc<Core>, tool: &str, mut answer: Value) -> (Option<Verdict>, String) {
    let Some(Provided(plugins)) = core.config.plugins else {
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

/// A deploy reached its end: what ran, where and its result, with the project's
/// health check on one that went well, is kept on the agent's slice.
async fn ended(core: Arc<Core>, deploy: Deploy, verdict: Option<Verdict>, output: String) {
    let mut verdict = verdict;
    let mut lines = vec![deploy.why.clone(), output];
    if verdict.is_none() {
        lines.push("Sikemux did not see how it ended.".to_owned());
    }
    if verdict == Some(Verdict::Passed) {
        if let Some(address) = board::health_of(&core, &deploy.project, &deploy.environment) {
            let (health, said) = health_of(&address).await;
            verdict = Some(health);
            lines.push(said);
        }
    }
    let text: String = lines
        .iter()
        .filter(|line| !line.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let start = text.chars().count().saturating_sub(MAX_OUTPUT_CHARS);
    let ran = DeployRan {
        environment: deploy.environment,
        what: deploy.what,
        verdict,
        output: text.chars().skip(start).collect(),
        by_person: deploy.by_person,
        head: deploy.head,
    };
    core.repo_events.tell(RepoEvent::Deployed {
        project: deploy.project,
        agent: deploy.agent_id,
        ran,
    });
}

#[derive(Deserialize, Serialize)]
pub struct HoldParams {
    pub hold: String,
}

/// The person allows the held action once: a tool call runs and its agent hears
/// what it answered; a command its agent is told it may run again, until the end
/// of the turn in which it hears this.
pub(crate) async fn allow(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    allow_hold(core, hold).await;
    Ok(())
}

async fn allow_hold(core: &Arc<Core>, mut hold: Hold) {
    core.ship.remember(&hold, true);
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
        tokio::spawn(async move { messages::for_person_waiting(&core, hold.agent_id, text).await });
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let ran = as_caller(
            call.caller,
            plugins::run_agent_tool(&core, &call.name, call.arguments, hold.deploy),
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
        let _ = messages::for_person(&core, hold.agent_id, text).await;
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
    core.ship.state.locked().allowed.push(Allowed {
        agent_id: agent_id.clone(),
        permit,
        call,
        heard: false,
    });
    let told = messages::for_person(core, agent_id.clone(), text).await;
    let heard =
        told.is_ok_and(|told| matches!(told["delivery"].as_str(), Some("steered" | "newTurn")));
    if heard {
        core.ship.turn_started(&agent_id);
    }
}

/// The person passed the gate of `environment` while `agent` waits on it: its
/// deploys held for there go once, and the gate passes when one has gone. False
/// when none is held, so the gate is the person's to pass by itself.
pub(crate) async fn allow_held(
    core: &Arc<Core>,
    agent: &str,
    project: &str,
    environment: &str,
) -> bool {
    let ids: Vec<String> = (core.ship.state.locked().holds.iter())
        .filter(|hold| {
            hold.agent_id == agent
                && hold.project == project
                && hold.environment.as_deref() == Some(environment)
        })
        .map(|hold| hold.token.clone())
        .collect();
    for id in &ids {
        if let Ok(hold) = take(core, id) {
            allow_hold(core, hold).await;
        }
    }
    !ids.is_empty()
}

/// The person passed the gate of `environment` with nothing held: the agent may
/// deploy there once.
pub(crate) async fn permit_environment(
    core: &Arc<Core>,
    agent: &str,
    project: &str,
    environment: &str,
) {
    let permit = Permit::Environment {
        project: project.to_owned(),
        name: environment.to_owned(),
    };
    let text = format!(
        "The person passed {environment}. Deploy there now if you haven't; one deploy may run, until this turn ends."
    );
    permitted(core, agent.to_owned(), permit, text, None).await;
}

/// The person always allows what the held action lands in, for its project or for every
/// project in its folder. Each action held for there goes: a tool call runs, and a command's
/// agent is told to run it again.
pub(crate) async fn let_go(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let (project, always) = {
        let state = core.ship.state.locked();
        let hold = (state.holds.iter())
            .find(|hold| hold.token == params.hold)
            .ok_or("that action no longer waits on you")?;
        let always = (hold.let_go.clone()).ok_or("this action can't be always allowed")?;
        (hold.project.clone(), always)
    };
    let (allowing, there, kept) = (core.clone(), project.clone(), always.clone());
    blocking(move || Ok(board::allow_always(&allowing, &there, &kept)?)).await?;
    let covered = |held: &str| match &always.folder {
        Some(folder) => Path::new(held).starts_with(folder),
        None => held == project,
    };
    let held: Vec<(String, String, Ship)> = (core.ship.state.locked().holds.iter())
        .filter(|hold| covered(&hold.project))
        .map(|hold| (hold.token.clone(), hold.project.clone(), hold.ship.clone()))
        .collect();
    for (id, project, ship) in held {
        if board::judge_ship(core, &project, &ship).goes != Goes::Freely {
            continue;
        }
        if let Ok(mut hold) = take(core, &id) {
            core.ship.remember(&hold, true);
            if let Some(call) = hold.call.take() {
                hold.deploy = check(core, &hold.agent_id, hold.ship.clone(), &hold.cwd)
                    .ok()
                    .flatten();
                let allowed = format!("The person always allows {} now", always.covers);
                run_held(core, hold, call, allowed);
                continue;
            }
            let text = format!(
                "The person always allows {} now. Run `{}` again now.",
                always.covers, hold.ship.what
            );
            let _ = messages::for_person(core, hold.agent_id, text).await;
        }
    }
    Ok(())
}

/// The person refuses the held action, and the agent is told not to take it.
pub(crate) async fn refuse(core: &Arc<Core>, params: HoldParams) -> CoreResult<()> {
    let hold = take(core, &params.hold)?;
    core.ship.remember(&hold, false);
    let text = format!(
        "The person did not allow `{}`. Do not run it; if your work needed it, say so in your answer.",
        hold.ship.what
    );
    let _ = messages::for_person(core, hold.agent_id, text).await;
    Ok(())
}

fn take(core: &Core, id: &str) -> CoreResult<Hold> {
    let hold = {
        let mut state = core.ship.state.locked();
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
    let state = core.ship.state.locked();
    let wanted = (state.holds.iter())
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
    let name = core.names.name_or_id(&hold.agent_id);
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
    let always = (hold.let_go.as_ref()).map(|always| {
        option(
            person_calls::HOLD_ALWAYS,
            "Always allow it here".to_owned(),
            Some(always.covers.clone()),
        )
    });
    let options = [
        Some(option(
            person_calls::HOLD_ALLOW,
            "Allow it once".to_owned(),
            None,
        )),
        Some(option(
            person_calls::HOLD_REFUSE,
            "Not now".to_owned(),
            None,
        )),
        always,
    ];
    PersonCall {
        id: hold.token.clone(),
        project: hold.project.clone(),
        kind: CallKind::Production,
        asker: CallAsker {
            agent_id: hold.agent_id.clone(),
            cli: questions::cli_of(core, &hold.agent_id),
            name,
            chief: false,
        },
        joined: Vec::new(),
        asked_for: None,
        question,
        context: hold.why.clone(),
        options: options.into_iter().flatten().collect(),
        pick: None,
        against: None,
        evidence: Vec::new(),
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
    use super::*;

    /// `"ship": ["./deploy.sh"]` in a QA project's sikemux.json, and `./deploy.sh` ran with no
    /// hold once the agent started it with process_start.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_ship_command_started_as_a_process_waits_for_the_person() {
        let project = tempfile::tempdir().unwrap();
        let config = r#"{ "version": 1, "ship": ["./deploy.sh"] }"#;
        std::fs::write(project.path().join("sikemux.json"), config).unwrap();
        let core = Core::new(Default::default(), None).unwrap();
        let said = check_process(&core, "agent", "./deploy.sh", &[], project.path())
            .await
            .unwrap_err();
        assert!(said.starts_with("Held for the person"), "{said}");
        let waiting = core.ship.waiting();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].hold.let_go.as_deref(), Some("./deploy.sh"));
    }
}
