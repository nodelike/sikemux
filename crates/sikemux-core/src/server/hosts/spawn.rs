//! `agent_spawn` with `host`: the lead's host names the worker, counting the
//! whole team, and hands the linked host the task and the lead's code. That
//! host starts the worker on its own agent sign-ins, which never cross hosts,
//! in its own checkout. Each time the worker's turn ends, its new commits come
//! back before its report.

use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::agent_accounts::allocate::Aim;
use crate::agent_accounts::status::AgentStatus as AccountStatus;
use crate::agent_accounts::usage;
use crate::cli::protocol::{is_tool_agent_id, HarnessRequest};
use crate::server::agent_accounts::{placement, Placing};
use crate::server::workspace::LauncherId;
use sikemux_wire::protocol::AgentStatus;

use super::super::connection::blocking;
use super::super::follow::Happened;
use super::super::names::{Adopted, Worker};
use super::super::remote::StoredLink;
use super::super::spawn::{self as local, Ending, ForHost};
use super::super::worktree;
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::code::{self, Checked, Code, Packed};
use super::link::{Received, Refusal};
use super::route::pin;
use super::team::{Foreign, Role, Tree};
use super::{call, find, name_of, post, Failed, Sending, Stands};

/// Under the minute an agent's tool call may take.
const SPAWN_WAIT: Duration = Duration::from_secs(50);
/// How often a host is asked again whether a worker it may be starting has started.
const PENDING_AGAIN: Duration = Duration::from_secs(20);
const MAX_PROMPT_BYTES: usize = 64 * 1024;

fn text<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

pub(crate) async fn spawn(
    core: &Arc<Core>,
    request: &HarnessRequest,
    host: &str,
    prompt: String,
) -> Result<Value, String> {
    let lead_id = request
        .agent_id
        .clone()
        .ok_or("agent_spawn needs the calling agent's id")?;
    let params = &request.params;
    if text(params, "resume").is_some() {
        return Err("resume reopens a session on this host; leave out host".into());
    }
    if text(params, "mode") == Some("terminal") {
        return Err("an agent on another host runs as a chat; leave out mode".into());
    }
    if text(params, "provider").is_some() {
        return Err(
            "a worker on another host runs on that host's own sign-ins; leave out provider".into(),
        );
    }
    let link = find(core, host)?;
    let model = text(params, "model").map(str::to_owned);
    let effort = text(params, "effort").map(str::to_owned);
    let reports = !matches!(params.get("report"), Some(Value::Bool(false)));
    let uncommitted = matches!(params.get("uncommitted"), Some(Value::Bool(true)));
    let agent_id = crate::server::names::new_agent_id();
    let (lead, cwd, named, provider) = {
        let (core, request, agent_id) = (core.clone(), request.clone(), agent_id.clone());
        let cli = text(params, "cli").map(str::to_lowercase);
        blocking(move || {
            let (lead, cwd) = local::lead_of(&core, &lead_id, &request.project)?;
            let provider = cli
                .or_else(|| lead.cli().map(str::to_owned))
                .ok_or("cli must name the agent to start, such as claude or codex")?;
            let named = local::hire(
                &core,
                Worker {
                    agent_id: &agent_id,
                    lead: &lead_id,
                    project: None,
                    provider: &provider,
                    session: None,
                    reports,
                },
            )?;
            Ok((lead, cwd, named, provider))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let lead_id = request.agent_id.clone().unwrap_or_default();
    let packed = {
        let cwd = cwd.clone();
        blocking(move || code::package_for(&cwd, uncommitted).map_err(Into::into)).await
    };
    let packed = match packed {
        Ok(packed) => packed,
        Err(error) => {
            core.names().forget(&agent_id);
            return Err(error.to_string());
        }
    };
    let started_from = format!(
        "your commit {}{}",
        &packed.code.commit[..packed.code.commit.len().min(10)],
        match (packed.code.patch.is_some(), packed.left_behind) {
            (true, _) => " with your uncommitted changes committed on top".to_owned(),
            (false, 0) => String::new(),
            (false, 1) => ", without the 1 changed file not committed in your checkout".to_owned(),
            (false, count) =>
                format!(", without the {count} changed files not committed in your checkout"),
        }
    );
    let lead_name = core
        .names()
        .name(&lead_id)
        .unwrap_or_else(|| lead_id.clone());
    let depth = core.names().depth(&lead_id);
    let Handed {
        name,
        answer,
        pending,
    } = hand_over(
        core,
        &link,
        Handing {
            agent_id: agent_id.clone(),
            name: named.name.clone(),
            provider,
            project: named.project.clone(),
            lead: LeadParams {
                agent_id: lead_id,
                name: lead_name,
                cli: lead.provider.clone(),
                mode: lead.permission_mode.map(|mode| mode.as_str().to_owned()),
                depth,
            },
            model,
            effort,
            reports,
            prompt,
            branch: text(params, "branch").map(str::to_owned),
            packed,
            auto: false,
            wait: SPAWN_WAIT,
        },
    )
    .await?;
    let shown = format!("{name} on {}", link.name);
    if pending {
        return Ok(json!({
            "name": shown,
            "agentId": agent_id,
            "host": link.name,
            "started": "not known yet",
            "note": format!("{} did not answer in time, so whether {shown} started is not known yet. Do not start it again: Sikemux asks {} until it knows, and tells you whether it started. It starts from {started_from}.", link.name, link.name),
        }));
    }
    let branch = answer["branch"].as_str().unwrap_or_default();
    Ok(json!({
        "name": shown,
        "agentId": agent_id,
        "host": link.name,
        "cli": answer["cli"],
        "model": answer["model"],
        "reportsBack": reports,
        "note": format!(
            "{shown} is starting on {} in a checkout of its own{}, from {started_from}, and gets your prompt once it is ready. Each time its turn ends, its new commits come back to a branch in your checkout{}.",
            link.name,
            if branch.is_empty() { String::new() } else { format!(" on its branch {branch}") },
            if reports { " and its final answer comes to you as a message" } else { "" },
        ),
    }))
}

/// A worker this host named, to start on a linked host.
pub(crate) struct Handing {
    pub agent_id: String,
    pub name: String,
    pub provider: String,
    pub project: String,
    pub lead: LeadParams,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub reports: bool,
    pub prompt: String,
    pub branch: Option<String>,
    pub packed: Packed,
    pub auto: bool,
    pub wait: Duration,
}

pub(crate) struct Handed {
    pub name: String,
    pub answer: Value,
    /// Whether it started is not known yet; its lead hears once it is.
    pub pending: bool,
}

/// Starts a named worker on `link`, sending the code when it lacks it. When it does not
/// start, this host forgets the worker.
pub(crate) async fn hand_over(
    core: &Arc<Core>,
    link: &StoredLink,
    handing: Handing,
) -> Result<Handed, String> {
    let Handing {
        agent_id,
        name: given,
        provider,
        project,
        lead,
        model,
        effort,
        reports,
        prompt,
        branch,
        packed,
        auto,
        wait,
    } = handing;
    let id = super::call_id();
    core.hosts().team.add(Foreign {
        agent_id: agent_id.clone(),
        host: link.id.clone(),
        name: given.clone(),
        cli: provider.clone(),
        project,
        parent: Some(lead.agent_id.clone()),
        role: Role::Worker,
        status: AgentStatus::Starting,
        said: None,
        active_at: Some(epoch_ms()),
        repo: Some(packed.repo.clone()),
        landed: None,
        landed_head: None,
        branch: None,
        pending: Some(id.clone()),
    });
    pin(core);
    let task = params_of::<asks::Spawn>(&SpawnParams {
        agent_id: agent_id.clone(),
        name: given.clone(),
        lead,
        cli: provider,
        model,
        effort,
        reports,
        prompt,
        branch,
        code: packed.code.clone(),
        auto,
    });
    let started = match call(
        core,
        &link.id,
        asks::Spawn::METHOD,
        task.clone(),
        None,
        wait,
        &id,
    )
    .await
    {
        Ok(answer) if answer["needs"] == "bundle" => {
            let tips: Vec<String> = answer["tips"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let (repo, commit) = (packed.repo.clone(), packed.code.commit.clone());
            match blocking(move || code::bundle(&repo, &commit, &tips).map_err(Into::into)).await {
                Ok(bundle) => {
                    match call(
                        core,
                        &link.id,
                        asks::Spawn::METHOD,
                        task,
                        Some(bundle.path()),
                        wait,
                        &id,
                    )
                    .await
                    {
                        Ok(answer) if answer["needs"] == "bundle" => Err(Failed::Refused(
                            "it could not take your code from the bundle".into(),
                        )),
                        answer => answer,
                    }
                }
                Err(error) => Err(Failed::Refused(error.to_string())),
            }
        }
        answer => answer,
    };
    let answer = match started {
        Ok(answer) => answer,
        Err(Failed::Unknown(_) | Failed::Retry(_)) => {
            let host = link.id.clone();
            let (core, agent_id) = (core.clone(), agent_id.clone());
            tokio::spawn(
                async move { settle_pending(&core, &host, &agent_id, PENDING_AGAIN).await },
            );
            return Ok(Handed {
                name: given,
                answer: Value::Null,
                pending: true,
            });
        }
        Err(error) => {
            not_started(core, &agent_id);
            return Err(format!("{given} did not start on {}: {error}", link.name));
        }
    };
    Ok(Handed {
        name: started_as(core, &agent_id, &given, &answer),
        answer,
        pending: false,
    })
}

/// The worker started there, under the name its answer gives.
fn started_as(core: &Core, agent_id: &str, given: &str, answer: &Value) -> String {
    let name = answer["name"].as_str().unwrap_or(given).to_owned();
    if name != given {
        core.names().set_name(agent_id, &name);
    }
    core.hosts().team.update(agent_id, |worker| {
        worker.name = name.clone();
        worker.pending = None;
    });
    super::changed(core);
    name
}

fn not_started(core: &Core, agent_id: &str) {
    if let Some(worker) = core.hosts().team.remove(agent_id) {
        core.harness()
            .note_agent(&worker.project, Happened::Gone.kind(), agent_id);
    }
    core.names().forget(agent_id);
    pin(core);
    super::changed(core);
}

/// Holds a worker's place among those whose start is asked after, until dropped.
struct Settling<'a> {
    core: &'a Core,
    agent_id: String,
}

impl<'a> Settling<'a> {
    fn claim(core: &'a Core, agent_id: &str) -> Option<Self> {
        let mut state = core.hosts().state.locked();
        state.settling.insert(agent_id.to_owned()).then(|| Self {
            core,
            agent_id: agent_id.to_owned(),
        })
    }
}

impl Drop for Settling<'_> {
    fn drop(&mut self) {
        self.core
            .hosts()
            .state
            .locked()
            .settling
            .remove(&self.agent_id);
    }
}

/// The workers being started on `host` whose start is not known yet find out how it went.
pub(crate) fn settle_pending_on(core: &Arc<Core>, host: &str) {
    for worker in core.hosts().team.on(host) {
        if worker.pending.is_none() {
            continue;
        }
        let (core, host) = (core.clone(), host.to_owned());
        tokio::spawn(async move {
            settle_pending(&core, &host, &worker.agent_id, Duration::ZERO).await
        });
    }
}

/// Asks `host` what became of starting the worker `agent_id`, while it is reachable, and tells
/// its lead once that is known. A host too old to say has the worker stopped instead.
async fn settle_pending(core: &Arc<Core>, host: &str, agent_id: &str, first: Duration) {
    let Some(_settling) = Settling::claim(core, agent_id) else {
        return;
    };
    tokio::time::sleep(first).await;
    loop {
        let Some(worker) = core.hosts().team.get(agent_id) else {
            return;
        };
        let (Some(id), Some(lead)) = (worker.pending.clone(), worker.parent.clone()) else {
            return;
        };
        let place = name_of(core, host);
        let told = match super::receipt(core, host, &id).await {
            Ok(Stands::Done(Ok(answer))) => {
                let name = started_as(core, agent_id, &worker.name, &answer);
                format!("{name} on {place} has started, and gets your prompt once it is ready.")
            }
            Ok(Stands::Done(Err(why))) => {
                not_started(core, agent_id);
                format!("{} did not start on {place}: {why}", worker.name)
            }
            Ok(Stands::Unknown) => {
                not_started(core, agent_id);
                format!("{} did not start on {place}: what started it never arrived there. Start it again if you still need it.", worker.name)
            }
            Err(Failed::Refused(_)) => {
                super::route::stop_worker(core, agent_id);
                not_started(core, agent_id);
                format!("{} may have started on {place}, but {place} runs an older Sikemux that cannot say, so Sikemux stopped it there. Start it again if you still need it.", worker.name)
            }
            Ok(Stands::Running) | Err(_) => {
                if core.hosts().connected(host).is_none() {
                    return;
                }
                tokio::time::sleep(PENDING_AGAIN).await;
                continue;
            }
        };
        super::route::notice(core, lead, told);
        return;
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LeadParams {
    pub agent_id: String,
    pub name: String,
    #[serde(default)]
    pub cli: String,
    pub mode: Option<String>,
    #[serde(default)]
    pub depth: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SpawnParams {
    agent_id: String,
    name: String,
    lead: LeadParams,
    cli: String,
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    reports: bool,
    prompt: String,
    #[serde(default)]
    branch: Option<String>,
    code: Code,
    #[serde(default)]
    auto: bool,
}

/// A linked host asks this one to start a worker for its agent.
pub(super) async fn start_here(
    core: &Arc<Core>,
    host: &str,
    params: SpawnParams,
    body: Option<Received>,
) -> Result<Value, String> {
    if !is_tool_agent_id(&params.agent_id) || !is_tool_agent_id(&params.lead.agent_id) {
        return Err("that is not an agent id".into());
    }
    if params.prompt.trim().is_empty() || params.prompt.len() > MAX_PROMPT_BYTES {
        return Err("the prompt is empty or too long".into());
    }
    if params.cli.trim().is_empty() {
        return Err("the worker needs a cli".into());
    }
    if core.names().get(&params.agent_id).is_some() {
        return Err("that agent already runs here".into());
    }
    let host_name = name_of(core, host);
    let launcher = {
        let (core, cli) = (core.clone(), params.cli.trim().to_lowercase());
        blocking(move || Ok(account_here(&core, &cli))).await
    }
    .map_err(|error| error.to_string())?
    .map_err(|why| format!("{why}; {}", can_start_here(core, &params.cli)))?;
    let checked = {
        let branch = params
            .branch
            .clone()
            .unwrap_or_else(|| worktree::branch_for(&params.prompt));
        let (core, host, agent_id, code) = (
            core.clone(),
            host.to_owned(),
            params.agent_id.clone(),
            params.code.clone(),
        );
        blocking(move || {
            code::checkout(
                &core,
                &host,
                &agent_id,
                &code,
                body.as_ref().map(Received::path),
                &branch,
            )
            .map_err(Into::into)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let (mut tree, cwd) = match checked {
        Checked::Needs(tips) => return Ok(json!({ "needs": "bundle", "tips": tips })),
        Checked::Ready(tree, cwd) => (tree, cwd),
    };
    tree.auto = params.auto;
    let project = core.workspaces().project_of(&cwd);
    let lead = &params.lead;
    if core.hosts().team.get(&lead.agent_id).is_none() {
        core.hosts().team.add(Foreign {
            agent_id: lead.agent_id.clone(),
            host: host.to_owned(),
            name: lead.name.clone(),
            cli: lead.cli.clone(),
            project: project.clone(),
            parent: None,
            role: Role::Lead,
            status: AgentStatus::Working,
            said: None,
            active_at: Some(epoch_ms()),
            repo: None,
            landed: None,
            landed_head: None,
            branch: None,
            pending: None,
        });
        core.names().adopt(
            Adopted {
                agent_id: &lead.agent_id,
                name: &lead.name,
                project: project.clone(),
                parent: None,
                provider: &lead.cli,
                reports: true,
                depth: lead.depth,
            },
            core.open_agents(),
        );
    }
    pin(core);
    core.hosts().team.add_tree(tree.clone());
    let plan = ForHost {
        agent_id: params.agent_id.clone(),
        name: params.name.clone(),
        lead_id: lead.agent_id.clone(),
        lead_name: format!("{} on {host_name}", lead.name),
        lead_cli: if lead.cli.is_empty() {
            "agent".to_owned()
        } else {
            lead.cli.clone()
        },
        mode: lead.mode.clone(),
        provider: params.cli.trim().to_lowercase(),
        model: params.model.clone(),
        effort: (params.effort.as_deref()).and_then(|effort| {
            crate::agent_accounts::defaults::clean_effort(&params.cli.trim().to_lowercase(), effort)
        }),
        reports: params.reports,
        cwd,
        prompt: params.prompt.clone(),
        launcher,
    };
    let started = {
        let core = core.clone();
        blocking(move || local::start_for_host(&core, plan).map_err(Into::into)).await
    };
    match started {
        Ok(mut answer) => {
            answer["branch"] = tree.branch.clone().into();
            super::changed(core);
            Ok(answer)
        }
        Err(error) => {
            core.hosts().team.remove_tree(&params.agent_id);
            let (repo, path, branch) = (tree.repo, tree.path, tree.branch);
            let _ = blocking(move || {
                worktree::remove(&repo, &path, &branch);
                Ok(())
            })
            .await;
            Err(error.to_string())
        }
    }
}

/// The launcher a worker of `cli` starts on here: the account a new chat of it would go to.
/// Refuses only for what is known: no account signed in, or every one at its limit. An account
/// whose sign-in or usage was not read yet counts as able to start.
fn account_here(core: &Core, cli: &str) -> Result<Option<String>, String> {
    let of_cli: Vec<_> = (core.workspaces().chat_launchers().into_iter())
        .filter(|launcher| launcher.agent.provider == cli)
        .collect();
    if of_cli.is_empty() {
        return Err(format!("{cli} is not set up there"));
    }
    let startable: Vec<_> = (of_cli.iter())
        .filter(|launcher| match &launcher.status {
            None => true,
            Some(status) => status.refusal(&launcher.label).is_none(),
        })
        .collect();
    if startable.is_empty() {
        let signed_out = (of_cli.iter())
            .any(|launcher| matches!(launcher.status, Some(AccountStatus::SignedOut)));
        return Err(match signed_out {
            true => format!("{cli} is signed in to no account there; the person signs in from Sikemux's Settings on that host"),
            false => format!("{cli} does not start there"),
        });
    }
    let now = epoch_ms();
    let limit_ends = |config_path: Option<&str>| -> Option<Option<u64>> {
        let reading = usage::last_reading(cli, config_path)?;
        let full: Vec<_> = (reading.usage.windows.iter())
            .filter(|window| window.scope.is_none() && window.used_at(now) >= 100.0)
            .collect();
        (!full.is_empty()).then(|| {
            (full.iter())
                .map(|window| window.resets_at.as_ref()?.unix_ms())
                .collect::<Option<Vec<u64>>>()
                .and_then(|ends| ends.into_iter().max())
        })
    };
    let (full, room): (Vec<_>, Vec<_>) = (startable.into_iter())
        .map(|launcher| (launcher, limit_ends(launcher.agent.config_path.as_deref())))
        .partition(|(_, ends)| ends.is_some());
    if room.is_empty() {
        let soonest = (full.iter()).filter_map(|(_, ends)| ends.flatten()).min();
        return Err(match soonest {
            Some(at) => format!(
                "every {cli} account there is at its limit, the first free again in {}",
                in_words(at.saturating_sub(now))
            ),
            None => format!("every {cli} account there is at its limit"),
        });
    }
    let room: Vec<&str> = (room.iter())
        .map(|(launcher, _)| launcher.id.as_str())
        .collect();
    let book = core.workspaces().accounts().unwrap_or_default();
    let placed = placement(&book, cli, &Placing::default(), Aim::Soonest, |profile| {
        room.contains(&LauncherId::on(cli, &profile.id).to_string().as_str())
    })
    .map(|placed| LauncherId::on(cli, &placed.id).to_string());
    let chosen = LauncherId::on(cli, &book.chosen(cli)).to_string();
    Ok(placed
        .or_else(|| room.contains(&chosen.as_str()).then_some(chosen))
        .or_else(|| room.first().map(|id| (*id).to_owned())))
}

/// The other clis this host can start, for a refusal of `cli`.
fn can_start_here(core: &Core, cli: &str) -> String {
    let mut others: Vec<String> = (core.workspaces().chat_launchers().into_iter())
        .filter(|launcher| launcher.agent.provider != cli.trim().to_lowercase())
        .filter(|launcher| {
            (launcher.status.as_ref())
                .is_none_or(|status| status.refusal(&launcher.label).is_none())
        })
        .map(|launcher| launcher.agent.provider)
        .collect();
    others.sort_unstable();
    others.dedup();
    match others.is_empty() {
        true => "it can start no other cli".to_owned(),
        false => format!("it can start {} instead", others.join(", ")),
    }
}

fn in_words(ms: u64) -> String {
    let minutes = ms.div_ceil(60_000);
    match minutes {
        0..=89 => format!("{minutes} min"),
        90..=2879 => format!("{} h", minutes / 60),
        _ => format!("{} days", minutes / 1440),
    }
}

/// Before a worker here reports to its lead on another host, its commits
/// since the last went back are sent ahead of the report.
pub(crate) async fn send_commits(core: &Arc<Core>, worker: &str) {
    let Some(tree) = core.hosts().team.tree(worker) else {
        return;
    };
    let Some(host) = core
        .names()
        .get(worker)
        .and_then(|named| named.parent)
        .and_then(|lead| core.hosts().team.get(&lead))
        .map(|lead| lead.host)
    else {
        return;
    };
    send_commits_to(core, worker, tree, &host).await;
}

async fn send_commits_to(core: &Arc<Core>, worker: &str, tree: Tree, host: &str) {
    let into = core.hosts().outbox.body_path(&format!(
        "{worker}-{}.bundle",
        uuid::Uuid::new_v4().simple()
    ));
    let made = {
        let (tree, into) = (tree.clone(), into.clone());
        blocking(move || code::commits(&tree, &into).map_err(Into::into)).await
    };
    match made {
        Ok(Some((head, count))) => {
            post(
                core,
                host,
                Sending::new(
                    asks::CommitsReady::METHOD,
                    params_of::<asks::CommitsReady>(&code::ArrivingCommits {
                        agent_id: worker.to_owned(),
                        head: head.clone(),
                        count,
                    }),
                )
                .keyed(format!("commits:{worker}"))
                .pulled(into)
                .kept()
                .from(worker)
                .about(worker),
            );
            core.hosts().team.tree_sent(worker, &head);
        }
        Ok(None) => {}
        Err(error) => {
            let _ = std::fs::remove_file(&into);
            eprintln!("sikemux core: a worker's commits did not go back to its lead: {error}");
        }
    }
}

/// The bundle of a worker's commits that its lead's host, `host`, fetches after hearing they are
/// ready.
pub(super) fn commits_for(
    core: &Core,
    host: &str,
    wanted: code::ArrivingCommits,
) -> Result<std::path::PathBuf, Refusal> {
    if super::route::lead_host(core, &wanted.agent_id).as_deref() != Some(host) {
        return Err("that worker's lead is not on your host".into());
    }
    core.hosts()
        .outbox
        .pulled_body(host, |post| {
            post.params["agentId"] == wanted.agent_id.as_str()
                && post.params["head"] == wanted.head.as_str()
        })
        .filter(|path| path.is_file())
        .ok_or_else(|| "those commits no longer wait here; newer ones replaced them".into())
}

/// How a turn of a worker Auto started from another host ended.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoTurn {
    pub agent_id: String,
    pub ending: Ending,
    pub answer: Option<String>,
    pub cost_usd: Option<f64>,
}

/// A turn of a worker here that Auto on `host` started ended: its commits go back first,
/// then how the turn went, which `host` acts on as it does for its own agents.
pub(crate) async fn auto_turn_ended(
    core: &Arc<Core>,
    worker: String,
    tree: Tree,
    host: String,
    ending: Ending,
    answer: Option<String>,
) {
    send_commits_to(core, &worker, tree, &host).await;
    let cost_usd = core
        .chats()
        .get(&worker)
        .and_then(|chat| chat.feed.cost_usd());
    post(
        core,
        &host,
        Sending::new(
            asks::AutoTurn::METHOD,
            params_of::<asks::AutoTurn>(&AutoTurn {
                agent_id: worker.clone(),
                ending,
                answer,
                cost_usd,
            }),
        )
        .kept()
        .about(&worker),
    );
}

/// A worker the host its team's lead runs on named for an agent here.
#[derive(Clone)]
pub(crate) struct Hired {
    pub host: String,
    pub agent_id: String,
    pub name: String,
}

/// A worker here whose team is led from another host starts a worker of its
/// own: the lead's host names it, counting the whole team.
pub(crate) async fn hire_elsewhere(
    core: &Arc<Core>,
    lead: &str,
    provider: &str,
    reports: bool,
) -> Result<Option<Hired>, String> {
    let Some(host) = super::route::lead_host(core, lead) else {
        return Ok(None);
    };
    let agent_id = crate::server::names::new_agent_id();
    let name = super::route::hire(core, &host, &agent_id, lead, provider, reports).await?;
    Ok(Some(Hired {
        host,
        agent_id,
        name,
    }))
}

/// A worker the lead's host named never started here.
pub(crate) fn never_started(core: &Core, hired: &Hired) {
    super::route::forget_hired(core, &hired.host, &hired.agent_id);
}
