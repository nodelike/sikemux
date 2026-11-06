//! What a mod's page reads and writes through the bridge the app gives it. A read goes to the
//! core's own data or to a connector the person is signed in to, and only to what the mod's
//! `mod.json` declares it reads. A write must be declared too, and runs only once the person has
//! allowed the mod's writes. Credentials stay in the core: a page only ever sees answers.

use std::sync::Arc;

use crate::server::plugins::{AgentTool, Manifest};
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::codehost as host_method;

use super::super::connection::{blocking, ClientConn};
use super::super::plugins::{self, Provided};
use super::super::{board, codehost, git, tasks, Core, CoreError, CoreResult};
use super::{read_manifest, Manifest as ModManifest};

/// Where a name the bridge is asked for goes.
enum Reach {
    Agents,
    Board,
    Tasks,
    Runs,
    Questions,
    Inbox,
    Usage,
    Load,
    Repo,
    Git(String),
    /// A method of a connector's plugin.
    Plugin {
        plugin: String,
        method: String,
    },
    /// One of a connector's agent tools, by the name agents know it by.
    Tool(String),
    Send,
    BoardAdd,
    BoardUpdate,
}

struct Asked {
    /// What `mod.json` must name: a read's data or connector, or the write itself.
    declares: String,
    writes: bool,
    reach: Reach,
}

impl Asked {
    fn read(declares: &str, reach: Reach) -> Self {
        Self {
            declares: declares.to_owned(),
            writes: false,
            reach,
        }
    }

    fn write(name: &str, reach: Reach) -> Self {
        Self {
            declares: name.to_owned(),
            writes: true,
            reach,
        }
    }
}

/// What makes a mod's read of a name out of date: this host's view, or a topic's change.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    View,
    Topic(sikemux_wire::changed::Topic),
}

/// What a mod's watch of `name` reads again on; a name with nothing to follow is read again on
/// a timer of the page's own while the pane shows.
pub fn follows(name: &str) -> Option<Moved> {
    use sikemux_wire::changed::Topic;
    Some(match name {
        "agents" | "inbox" => Moved::View,
        "board" => Moved::Topic(Topic::Boards),
        "tasks" => Moved::Topic(Topic::Tasks),
        "runs" => Moved::Topic(Topic::Runs),
        "usage" => Moved::Topic(Topic::Usage),
        "questions" => Moved::Topic(Topic::Questions),
        name if name.starts_with("git.") => Moved::Topic(Topic::Git),
        _ => return None,
    })
}

fn asked(core: &Core, name: &str) -> CoreResult<Asked> {
    let core_read = match name {
        "agents" => Some(Reach::Agents),
        "board" => Some(Reach::Board),
        "tasks" => Some(Reach::Tasks),
        "runs" => Some(Reach::Runs),
        "questions" => Some(Reach::Questions),
        "inbox" => Some(Reach::Inbox),
        "usage" => Some(Reach::Usage),
        "load" => Some(Reach::Load),
        "repo" => Some(Reach::Repo),
        _ => None,
    };
    if let Some(reach) = core_read {
        return Ok(Asked::read(name, reach));
    }
    let core_write = match name {
        "agents.send" => Some(Reach::Send),
        "board.add" => Some(Reach::BoardAdd),
        "board.update" => Some(Reach::BoardUpdate),
        _ => None,
    };
    if let Some(reach) = core_write {
        return Ok(Asked::write(name, reach));
    }
    if name.starts_with("git.") {
        return match git::reads(name) {
            true => Ok(Asked::read("git", Reach::Git(name.to_owned()))),
            false => Err(format!("{name} is not a git read a mod can make").into()),
        };
    }
    connector(&plugins::manifests(core), name)
        .ok_or_else(|| CoreError::from(format!("a mod cannot ask for {name}")))
}

/// What a connector's agent tool does: a tool that searches or reads only reads, and one the
/// person approves call by call is never offered to a mod.
fn tool_reads(tool: &AgentTool) -> Option<bool> {
    if tool.approve.is_some() {
        return None;
    }
    let kind = tool.row.as_ref().and_then(|row| row["kind"].as_str());
    Some(matches!(kind, Some("read" | "search")))
}

/// A connector's method, as `github.pulls`, or one of its agent tools, as `jira_search`.
fn connector(manifests: &[&Manifest], name: &str) -> Option<Asked> {
    let key = |manifest: &Manifest| manifest.id.trim_start_matches("sikemux.").to_owned();
    let Some((wanted, method)) = name.split_once('.') else {
        let (manifest, tool) = manifests.iter().find_map(|manifest| {
            let tool = manifest.tools.iter().find(|tool| tool.name == name)?;
            Some((manifest, tool))
        })?;
        let reach = Reach::Tool(name.to_owned());
        return Some(match tool_reads(tool)? {
            true => Asked::read(&key(manifest), reach),
            false => Asked::write(name, reach),
        });
    };
    let manifest = manifests.iter().find(|manifest| key(manifest) == wanted)?;
    if manifest.account_methods.iter().any(|known| known == method) {
        return None;
    }
    let tool = manifest.tools.iter().find(|tool| tool.method == method);
    let reads = match tool {
        Some(tool) => tool_reads(tool)?,
        None if manifest.code_host.is_some() && host_method::reads(method) => true,
        None if manifest.code_host.is_some() && host_method::writes(method) => false,
        None => return None,
    };
    let reach = Reach::Plugin {
        plugin: manifest.id.clone(),
        method: method.to_owned(),
    };
    Some(match reads {
        true => Asked::read(wanted, reach),
        false => Asked::write(name, reach),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in super::super) struct BridgeParams {
    id: String,
    name: String,
    #[serde(default)]
    params: Value,
    /// The project of the strip the mod's pane is in.
    #[serde(default)]
    project: Option<String>,
}

/// The mod's manifest, once its folder still holds one that reads.
async fn manifest_of(core: &Core, id: &str) -> CoreResult<ModManifest> {
    let folder = super::shown_folder(core, id)?;
    blocking(move || read_manifest(&folder).map_err(CoreError::from)).await
}

/// What `asked.name` answers the mod `asked.id`, when its `mod.json` declares reading it.
pub(in super::super) async fn read(
    core: &Arc<Core>,
    client: &ClientConn,
    asked: BridgeParams,
) -> CoreResult<Value> {
    let manifest = manifest_of(core, &asked.id).await?;
    let wanted = self::asked(core, &asked.name)?;
    if wanted.writes {
        return Err(format!("{} writes; ask for it with write", asked.name).into());
    }
    if !manifest.reads.contains(&wanted.declares) {
        return Err(format!(
            "this mod's mod.json does not declare reading {}; add \"{}\" to its reads",
            asked.name, wanted.declares
        )
        .into());
    }
    run(core, client, wanted.reach, asked.params, asked.project).await
}

/// Runs the write `asked.name` for the mod `asked.id`, or answers `ask` with every write it
/// declares while the person has not allowed them.
pub(in super::super) async fn write(
    core: &Arc<Core>,
    client: &ClientConn,
    asked: BridgeParams,
) -> CoreResult<Value> {
    let manifest = manifest_of(core, &asked.id).await?;
    let wanted = self::asked(core, &asked.name)?;
    if !wanted.writes {
        return Err(format!("{} only reads; ask for it with read", asked.name).into());
    }
    if !manifest.writes.contains(&wanted.declares) {
        return Err(format!(
            "this mod's mod.json does not declare the write {0}; add \"{0}\" to its writes",
            asked.name
        )
        .into());
    }
    if !core.mods.allowed(&asked.id, &manifest.writes) {
        return Ok(json!({ "ask": { "name": manifest.name, "writes": manifest.writes } }));
    }
    let done = run(core, client, wanted.reach, asked.params, asked.project).await?;
    Ok(json!({ "done": done }))
}

/// `params` with the project the pane is in, unless they name one.
fn in_project(mut params: Value, project: &Option<String>) -> CoreResult<Value> {
    if params.is_null() {
        params = json!({});
    }
    let fields = params
        .as_object_mut()
        .ok_or_else(|| CoreError::from("a mod's params must be an object"))?;
    if !fields.contains_key("project") {
        let project = project
            .clone()
            .ok_or_else(|| CoreError::from("this mod's pane is in no project"))?;
        fields.insert("project".into(), project.into());
    }
    Ok(params)
}

async fn run(
    core: &Arc<Core>,
    client: &ClientConn,
    reach: Reach,
    params: Value,
    project: Option<String>,
) -> CoreResult<Value> {
    match reach {
        Reach::Agents => {
            let every = params["all"].as_bool().unwrap_or(false);
            let core = core.clone();
            blocking(move || {
                let project = project.filter(|_| !every);
                Ok(Value::Array(crate::server::roster::rows(
                    &core,
                    project.as_deref(),
                    "",
                )))
            })
            .await
        }
        Reach::Board => {
            let params = super::super::calls::parse(in_project(params, &project)?)?;
            Ok(serde_json::to_value(board::board(core, params).await?)?)
        }
        Reach::Tasks => {
            let project = in_project(params, &project)?["project"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            Ok(serde_json::to_value(tasks::list(core, project).await?)?)
        }
        Reach::Runs => Ok(serde_json::to_value(tasks::runs(core))?),
        Reach::Questions => Ok(serde_json::to_value(super::super::questions::views(core))?),
        Reach::Inbox => Ok(serde_json::to_value(
            super::super::inbox::read(core).await?,
        )?),
        Reach::Usage => Ok(serde_json::to_value(
            super::super::agent_accounts::usages(core).await,
        )?),
        Reach::Load => Ok(serde_json::to_value(super::super::load::now())?),
        Reach::Repo => {
            let project = in_project(params, &project)?["project"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let repo = codehost::repo::of_project(core, &project)
                .await
                .map_err(String::from)?;
            let id = repo.id();
            Ok(json!({
                "connector": id.provider.trim_start_matches("sikemux."),
                "owner": id.owner,
                "name": id.name,
                "slug": repo.slug(),
                "account": repo.account(),
                "label": repo.label(),
            }))
        }
        Reach::Git(method) => {
            let mut params = params;
            if params.is_null() {
                params = json!({});
            }
            if let (Some(fields), Some(project)) = (params.as_object_mut(), project) {
                fields.entry("repo").or_insert(project.into());
            }
            git::call(core, &client.peer, &method, params).await
        }
        Reach::Plugin { plugin, method } => {
            let done = plugins::call(core, &plugin, &method, params.clone())
                .await
                .map_err(|failure| plugins::explain(&failure))?;
            plugins::wrote(core, &plugin, &method, &params);
            Ok(done)
        }
        Reach::Tool(name) => {
            let Some(Provided(plugins)) = core.config.plugins else {
                return Err("connectors are not loaded on this host".into());
            };
            plugins
                .call_agent_tool(&name, params)
                .await
                .map_err(|failure| CoreError::from(plugins::explain(&failure)))
        }
        Reach::Send => {
            #[derive(Deserialize)]
            struct Send {
                agent: String,
                text: String,
            }
            let send: Send = super::super::calls::parse(params)?;
            let agent_id = agent_named(core, &send.agent).await?;
            let prompt =
                super::super::calls::parse(json!({ "agentId": agent_id, "text": send.text }))?;
            super::super::chat::calls::prompt_chat(core, prompt).await?;
            Ok(Value::Null)
        }
        Reach::BoardAdd => {
            let params = super::super::calls::parse(in_project(params, &project)?)?;
            Ok(serde_json::to_value(
                board::person_add(core, params).await?,
            )?)
        }
        Reach::BoardUpdate => {
            let params = super::super::calls::parse(in_project(params, &project)?)?;
            Ok(serde_json::to_value(
                board::person_update(core, params).await?,
            )?)
        }
    }
}

/// The id of the open agent called `agent`, or with that id.
async fn agent_named(core: &Arc<Core>, agent: &str) -> CoreResult<String> {
    let (core, agent) = (core.clone(), agent.to_owned());
    blocking(move || {
        crate::server::roster::listed(&core)
            .into_iter()
            .find(|listed| {
                listed.agent_id == agent
                    || listed
                        .name
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&agent))
            })
            .map(|listed| listed.agent_id)
            .ok_or_else(|| CoreError::from(format!("no open agent is called {agent}")))
    })
    .await
}
