use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallOutcome, CallResult};
use sikemux_view::inbox::ShipKind;
use sikemux_view::slice::Verdict;

use super::hold::hold;
use super::{check_call, Deploy, ToolCall, MAX_TOLD};
use crate::cli::protocol::HarnessRequest;
use crate::server::person_calls;
use crate::server::places::{self, Goes, Judged};
use crate::server::plugged::{self, Provided};
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;
use crate::ship::{Ship, Target};

const DATABASE: &str = "sikemux.database";
const RUNDECK: &str = "sikemux.rundeck";
const VERCEL: &str = "sikemux.vercel";
const CLOUDFLARE: &str = "sikemux.cloudflare";
const FOLLOW_LOOKS: usize = 60;
const FOLLOW_EVERY: Duration = Duration::from_secs(15);
const HEALTH_TRIES: usize = 6;
const HEALTH_EVERY: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);

/// The databases marked production, as the host each is reached at and its name.
pub(super) async fn production_databases(core: &Arc<Core>) -> Vec<(String, String)> {
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
pub(super) fn on_databases(ship: Ship, databases: &[(String, String)]) -> Option<Ship> {
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

pub(super) async fn at_production_database(core: &Arc<Core>, ship: Ship) -> Option<Ship> {
    let client = matches!(&ship.target, Some(Target::Site { origin })
        if origin.starts_with("psql://") || origin.starts_with("mysql://"));
    match client {
        true => on_databases(ship, &production_databases(core).await),
        false => Some(ship),
    }
}

enum ToolShip {
    None,
    Checked(ShipKind, Option<Target>),
    Unverified { kind: ShipKind, why: String },
}

pub(super) fn rundeck_target(place: Result<Value, String>) -> Result<Target, String> {
    let place = place.map_err(|_| {
        "Sikemux could not verify which Rundeck environment this job targets, so it waits for the person's yes."
            .to_owned()
    })?;
    let path = place.get("path").and_then(Value::as_str);
    let production = place.get("production").and_then(Value::as_bool);
    match (path, production) {
        (Some(path), Some(production)) => Ok(Target::RundeckJob {
            path: path.to_owned(),
            production,
        }),
        _ => Err(
            "Sikemux could not verify which Rundeck environment this job targets, so it waits for the person's yes."
                .to_owned(),
        ),
    }
}

pub(super) fn vercel_redeploy_target(
    explicit: Option<&str>,
    looked_up: impl FnOnce() -> Result<Value, String>,
) -> Result<Target, String> {
    let target = match explicit.map(str::trim).filter(|target| !target.is_empty()) {
        Some(target) => target.to_owned(),
        None => looked_up()
            .ok()
            .and_then(|read| read.get("target")?.as_str().map(str::trim).map(str::to_owned))
            .filter(|target| !target.is_empty())
            .ok_or_else(|| {
                "Sikemux could not verify which Vercel environment this deployment targets, so it waits for the person's yes."
                    .to_owned()
            })?,
    };
    Ok(Target::Vercel {
        production: target.eq_ignore_ascii_case("production"),
    })
}

fn is_cloudflare_zone_id(raw: &str) -> bool {
    raw.len() == 32 && raw.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn cloudflare_zone_target(
    raw: Option<&str>,
    looked_up: impl FnOnce() -> Result<Value, String>,
) -> Result<Target, String> {
    let unverified = || {
        "Sikemux could not verify which Cloudflare zone this targets, so it waits for the person's yes."
            .to_owned()
    };
    let raw = raw
        .map(str::trim)
        .filter(|zone| !zone.is_empty())
        .ok_or_else(unverified)?;
    let domain = if is_cloudflare_zone_id(raw) {
        looked_up()
            .ok()
            .and_then(|read| {
                read.get("name")?
                    .as_str()
                    .map(|name| name.trim().trim_end_matches('.').to_ascii_lowercase())
            })
            .filter(|name| !name.is_empty() && !is_cloudflare_zone_id(name))
            .ok_or_else(unverified)?
    } else {
        let normalized = raw.trim_end_matches('.').to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(unverified());
        }
        normalized
    };
    Ok(Target::CloudflareZone { zone: domain })
}

/// What a call to the Sikemux tool `name` changes and where it lands, for the tools that can
/// reach a production place; the places on the list decide whether it waits.
async fn tool_ship(core: &Arc<Core>, name: &str, arguments: &Value) -> Result<ToolShip, String> {
    Ok(match name {
        "db_query" => {
            let asked = plugged::call(core, DATABASE, "agentWritesProduction", arguments.clone())
                .await
                .map_err(|failure| plugged::explain(&failure))?;
            if asked != true {
                return Ok(ToolShip::None);
            }
            let name = arguments["database"].as_str().unwrap_or("production");
            let target = Target::Database {
                name: name.to_owned(),
            };
            ToolShip::Checked(ShipKind::ProductionWrite, Some(target))
        }
        "rundeck_run" | "rundeck_import" | "rundeck_edit" => {
            let place = plugged::call(core, RUNDECK, "agentJobPlace", arguments.clone())
                .await
                .map_err(|failure| plugged::explain(&failure));
            match rundeck_target(place) {
                Ok(target) => ToolShip::Checked(ShipKind::Deploy, Some(target)),
                Err(why) => ToolShip::Unverified {
                    kind: ShipKind::Deploy,
                    why,
                },
            }
        }
        "vercel_promote" => {
            ToolShip::Checked(ShipKind::Deploy, Some(Target::Vercel { production: true }))
        }
        "vercel_redeploy" => {
            let explicit = arguments["target"].as_str();
            let looked_up = match explicit.map(str::trim).filter(|target| !target.is_empty()) {
                Some(_) => None,
                None => Some(
                    plugged::call(core, VERCEL, "deployment", arguments.clone())
                        .await
                        .map_err(|failure| plugged::explain(&failure)),
                ),
            };
            match vercel_redeploy_target(explicit, || {
                looked_up.unwrap_or_else(|| Err("missing deployment lookup".to_owned()))
            }) {
                Ok(target) => ToolShip::Checked(ShipKind::Deploy, Some(target)),
                Err(why) => ToolShip::Unverified {
                    kind: ShipKind::Deploy,
                    why,
                },
            }
        }
        "cloudflare_zone_create"
        | "cloudflare_dns_upsert"
        | "cloudflare_dns_delete"
        | "cloudflare_dns_batch"
        | "cloudflare_purge_cache" => {
            let raw = match name {
                "cloudflare_zone_create" => arguments["name"].as_str(),
                _ => arguments["zone"].as_str(),
            };
            let trimmed = raw.map(str::trim).filter(|zone| !zone.is_empty());
            let looked_up = match trimmed {
                Some(zone) if is_cloudflare_zone_id(zone) => Some(
                    plugged::call(
                        core,
                        CLOUDFLARE,
                        "resolveZone",
                        json!({ "zone": zone, "account": arguments.get("account") }),
                    )
                    .await
                    .map_err(|failure| plugged::explain(&failure)),
                ),
                _ => None,
            };
            match cloudflare_zone_target(trimmed, || {
                looked_up.unwrap_or_else(|| Err("missing zone lookup".to_owned()))
            }) {
                Ok(target) => ToolShip::Checked(ShipKind::ProductionWrite, Some(target)),
                Err(why) => ToolShip::Unverified {
                    kind: ShipKind::ProductionWrite,
                    why,
                },
            }
        }
        _ => ToolShip::None,
    })
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
    let tool = tool_ship(core, name, arguments).await?;
    let (kind, target, unverified_why) = match tool {
        ToolShip::None => return Ok(None),
        ToolShip::Checked(kind, target) => (kind, target, None),
        ToolShip::Unverified { kind, why } => (kind, None, Some(why)),
    };
    let what = format!("{name} {arguments}");
    let (checking, agent_id, cwd) = (core.clone(), agent_id.clone(), request.project.clone());
    let call = ToolCall {
        name: name.to_owned(),
        arguments: arguments.clone(),
        caller: (core.repo_above().plugins_caller_of)(core, request),
    };
    tokio::task::spawn_blocking(move || {
        let cwd_path = Path::new(&cwd);
        if let Some(why) = unverified_why {
            let project = (checking.names().get(&agent_id).map(|named| named.project))
                .unwrap_or_else(|| checking.workspaces().project_of(cwd_path));
            let judged = Judged {
                goes: Goes::Asks,
                place: None,
                keep: false,
                why: Some(why),
            };
            return hold(
                &checking,
                &agent_id,
                &project,
                Ship {
                    kind,
                    what,
                    target: None,
                },
                cwd_path,
                Some(call),
                Some(judged),
            );
        }
        check_call(
            &checking,
            &agent_id,
            Ship { kind, what, target },
            cwd_path,
            Some(call),
            true,
            None,
        )
    })
    .await
    .map_err(|error| error.to_string())?
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
                if core.ship().state.locked().told.len() >= MAX_TOLD {
                    let live = core.person_calls().known_ids();
                    core.ship().prune_settled(&live);
                }
                let id = person_calls::told(&core, telling.raise, outcome);
                core.ship().record_told(telling.key, id);
            }
        }
    }
}
