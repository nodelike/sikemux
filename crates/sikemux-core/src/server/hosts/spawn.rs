//! `agent_spawn` with `host`: the lead's host names the worker, counting the
//! whole team, and hands the linked host the task and the lead's code. That
//! host starts the worker on its own agent sign-ins, which never cross hosts,
//! in its own checkout. Each time the worker's turn ends, its new commits come
//! back before its report.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;

use crate::cli::protocol::{is_tool_agent_id, HarnessRequest};
use sikemux_wire::protocol::AgentStatus;

use super::super::connection::blocking;
use super::super::names::{Adopted, Worker};
use super::super::remote::StoredLink;
use super::super::spawn::{self as local, Ending, ForHost};
use super::super::worktree;
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::code::{self, Checked, Code, Packed};
use super::link::Received;
use super::route::pin;
use super::team::{Foreign, Role, Tree};
use super::{ask, find, name_of, post, Failed, Sending};

/// Under the minute an agent's tool call may take.
const SPAWN_WAIT: Duration = Duration::from_secs(50);
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
    let reports = !matches!(params.get("report"), Some(Value::Bool(false)));
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
        blocking(move || code::package(&cwd).map_err(Into::into)).await
    };
    let packed = match packed {
        Ok(packed) => packed,
        Err(error) => {
            core.names.forget(&agent_id);
            return Err(error.to_string());
        }
    };
    let lead_name = core.names.name(&lead_id).unwrap_or_else(|| lead_id.clone());
    let depth = core.names.depth(&lead_id);
    let Handed { name, answer } = hand_over(
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
    let branch = answer["branch"].as_str().unwrap_or_default();
    Ok(json!({
        "name": shown,
        "agentId": agent_id,
        "host": link.name,
        "cli": answer["cli"],
        "model": answer["model"],
        "reportsBack": reports,
        "note": format!(
            "{shown} is starting on {} in a checkout of its own{}, from your commit with your uncommitted changes committed on top, and gets your prompt once it is ready. Each time its turn ends, its new commits come back to a branch in your checkout{}.",
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
        reports,
        prompt,
        branch,
        packed,
        auto,
        wait,
    } = handing;
    core.hosts.team.add(Foreign {
        agent_id: agent_id.clone(),
        host: link.id.clone(),
        name: given.clone(),
        cli: provider.clone(),
        project,
        parent: Some(lead.agent_id.clone()),
        role: Role::Worker,
        status: AgentStatus::Starting,
        active_at: Some(epoch_ms()),
        repo: Some(packed.repo.clone()),
        landed: None,
    });
    pin(core);
    let task = params_of::<asks::Spawn>(&SpawnParams {
        agent_id: agent_id.clone(),
        name: given.clone(),
        lead,
        cli: provider,
        model,
        reports,
        prompt,
        branch,
        code: packed.code.clone(),
        auto,
    });
    let started = match ask(
        core,
        &link.id,
        asks::Spawn::METHOD,
        task.clone(),
        None,
        wait,
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
                    match ask(
                        core,
                        &link.id,
                        asks::Spawn::METHOD,
                        task,
                        Some(bundle.path()),
                        wait,
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
        Err(error) => {
            core.hosts.team.remove(&agent_id);
            core.names.forget(&agent_id);
            pin(core);
            super::changed(core);
            return Err(format!("{given} did not start on {}: {error}", link.name));
        }
    };
    let name = answer["name"].as_str().unwrap_or(&given).to_owned();
    if name != given {
        core.names.set_name(&agent_id, &name);
        core.hosts
            .team
            .update(&agent_id, |worker| worker.name = name.clone());
    }
    super::changed(core);
    Ok(Handed { name, answer })
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
    if core.names.get(&params.agent_id).is_some() {
        return Err("that agent already runs here".into());
    }
    let host_name = name_of(core, host);
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
    let project = core.workspaces.project_of(&cwd);
    let lead = &params.lead;
    if core.hosts.team.get(&lead.agent_id).is_none() {
        core.hosts.team.add(Foreign {
            agent_id: lead.agent_id.clone(),
            host: host.to_owned(),
            name: lead.name.clone(),
            cli: lead.cli.clone(),
            project: project.clone(),
            parent: None,
            role: Role::Lead,
            status: AgentStatus::Working,
            active_at: Some(epoch_ms()),
            repo: None,
            landed: None,
        });
        core.names.adopt(
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
    core.hosts.team.add_tree(tree.clone());
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
        reports: params.reports,
        cwd,
        prompt: params.prompt.clone(),
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
            core.hosts.team.remove_tree(&params.agent_id);
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

/// Before a worker here reports to its lead on another host, its commits
/// since the last went back are sent ahead of the report.
pub(crate) async fn send_commits(core: &Arc<Core>, worker: &str) {
    let Some(tree) = core.hosts.team.tree(worker) else {
        return;
    };
    let Some(host) = core
        .names
        .get(worker)
        .and_then(|named| named.parent)
        .and_then(|lead| core.hosts.team.get(&lead))
        .map(|lead| lead.host)
    else {
        return;
    };
    send_commits_to(core, worker, tree, &host).await;
}

async fn send_commits_to(core: &Arc<Core>, worker: &str, tree: Tree, host: &str) {
    let into = core.hosts.outbox.body_path(&format!(
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
                Sending {
                    method: asks::Commits::METHOD,
                    params: params_of::<asks::Commits>(&code::ArrivingCommits {
                        agent_id: worker.to_owned(),
                        head: head.clone(),
                        count,
                    }),
                    key: Some(format!("commits:{worker}")),
                    body: Some(into),
                    keep: true,
                },
            );
            core.hosts.team.tree_sent(worker, &head);
        }
        Ok(None) => {}
        Err(error) => {
            let _ = std::fs::remove_file(&into);
            eprintln!("sikemux core: a worker's commits did not go back to its lead: {error}");
        }
    }
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
        .chats
        .get(&worker)
        .and_then(|chat| chat.feed.cost_usd());
    post(
        core,
        &host,
        Sending {
            method: asks::AutoTurn::METHOD,
            params: params_of::<asks::AutoTurn>(&AutoTurn {
                agent_id: worker,
                ending,
                answer,
                cost_usd,
            }),
            key: None,
            body: None,
            keep: true,
        },
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
    let (host, agent_id) = (&hired.host, &hired.agent_id);
    post(
        core,
        host,
        Sending {
            method: asks::Forget::METHOD,
            params: params_of::<asks::Forget>(&super::route::AboutAgent {
                agent_id: agent_id.clone(),
            }),
            key: None,
            body: None,
            keep: false,
        },
    );
}
