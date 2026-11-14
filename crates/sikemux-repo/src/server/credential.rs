//! Hands git and gh a credential that lasts one command for a remote with an identity of the
//! agents' own. Only an agent this core started, or the core's own git, gets one.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use sikemux_agents::callers::{self, Asker};
use sikemux_plugin_api::codehost::asks::{Merge, PullRef, RepoRef};
use sikemux_plugin_api::credential::{Ask, Credential, Reach};
use sikemux_plugin_api::vault::{as_caller, Caller};

use crate::cli::protocol::HarnessRequest;
use crate::git_guard::identity;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

/// Has the stand-ins tell from the plugins, as they know without asking over the network,
/// which remotes have an identity of the agents' own.
pub fn provide(core: &Arc<Core>) {
    super::outbox::sweep(core);
    let core = Arc::downgrade(core);
    identity::provide(move |remote| {
        let core = core.upgrade()?;
        let actor = crate::server::plugged::acts_as(&core, remote)?;
        Some(identity::Actor {
            name: actor.name,
            email: actor.email,
        })
    });
}

/// Who asks from `peer` on the tool endpoint's `port`, and the project it works in. None for
/// a process this core did not start, nor an agent it started.
pub(super) async fn asker(
    core: &Arc<Core>,
    request: &HarnessRequest,
    peer: SocketAddr,
    port: u16,
) -> Option<(Asker, String)> {
    let core = core.clone();
    let cwd = request.project.clone();
    tokio::task::spawn_blocking(move || {
        let asker = callers::asker(peer, port)?;
        Some((asker, core.workspaces().project_of(Path::new(&cwd))))
    })
    .await
    .ok()
    .flatten()
}

/// Answers the helper at `peer` with a username and password, or with nothing, which leaves
/// git to fail as it would with no credential.
pub async fn call(
    core: &Arc<Core>,
    request: &HarnessRequest,
    peer: SocketAddr,
    port: u16,
) -> Result<Value, String> {
    let remote = request.params["remote"]
        .as_str()
        .ok_or("remote is missing")?
        .to_owned();
    let Some((asker, project)) = asker(core, request, peer, port).await else {
        return Ok(json!({}));
    };
    let grant = request.params["grant"].as_str();
    match answer(core, asker, &remote, grant, project).await {
        Ok(Some(credential)) => Ok(json!({
            "username": credential.username,
            "password": credential.secret,
            "expiresAt": credential.expires_at,
        })),
        Ok(None) => {
            identity::forget(&remote);
            Ok(json!({}))
        }
        Err(failure) => Err(crate::server::plugged::explain(&failure)),
    }
}

/// What `asker` gets for `remote`. This is where the core decides what a credential may do:
/// only the git the core starts to forward a push may write the repository, and only one the
/// person passed may change its workflows.
async fn answer(
    core: &Arc<Core>,
    asker: Asker,
    remote: &str,
    grant: Option<&str>,
    project: String,
) -> Result<Option<Credential>, sikemux_calls::plugins::PluginFailure> {
    let agent = match asker {
        Asker::Agent(agent) => Some(agent),
        Asker::Core => {
            if let Some(grant) = grant {
                if let Some(found) = super::edits::workflows::granted(core, grant, remote).await {
                    return Ok(Some(found));
                }
                if let Some(found) = super::outbox::minted(core, grant, remote).await {
                    return Ok(Some(found));
                }
            }
            None
        }
    };
    let ask = Ask {
        remote: remote.to_owned(),
        reach: Reach::Agent,
    };
    let caller = Caller {
        agent,
        project: Some(project),
    };
    as_caller(caller, crate::server::plugged::credential(core, ask)).await
}

/// Merges the pull request `gh pr merge` names as the agents' own identity, and deletes its
/// branch through the same forward as any push when asked to.
pub async fn merge(
    core: &Arc<Core>,
    request: &HarnessRequest,
    peer: SocketAddr,
    port: u16,
) -> Result<Value, String> {
    let params = &request.params;
    let text = |name: &str| params[name].as_str().map(str::to_owned);
    let remote = text("remote").ok_or("remote is missing")?;
    let number = params["number"].as_u64().ok_or("number is missing")?;
    let Some((asker, project)) = asker(core, request, peer, port).await else {
        return Err("Sikemux merges only for its own agents".to_owned());
    };
    let (owner, name) =
        owner_and_name(&remote).ok_or("Sikemux cannot read this remote's address")?;
    let merge = Merge {
        pull: PullRef {
            repo: RepoRef { owner, name },
            number,
        },
        method: text("method"),
        sha: text("sha"),
        message: None,
    };
    let caller = Caller {
        agent: match asker {
            Asker::Agent(agent) => Some(agent),
            Asker::Core => None,
        },
        project: Some(project),
    };
    let merged = as_caller(
        caller,
        crate::server::plugged::merge_as_agents(core, &remote, merge),
    )
    .await
    .map_err(|failure| crate::server::plugged::explain(&failure))?;
    if !merged {
        return Err(format!(
            "Sikemux has no identity of the agents' own on {remote}"
        ));
    }
    let (Some(branch), Some(head)) = (text("deleteBranch"), text("head")) else {
        return Ok(json!({ "said": format!("Merged pull request #{number}") }));
    };
    let cwd = Path::new(&request.project);
    let (deleted, said) = super::outbox::delete_branch(core, cwd, &remote, &branch, &head).await;
    let said = match deleted {
        true => {
            format!("Merged pull request #{number} and deleted its branch {branch} on the remote")
        }
        false => format!("Merged pull request #{number}, but its branch {branch} stays:\n{said}"),
    };
    Ok(json!({ "said": said }))
}

fn owner_and_name(remote: &str) -> Option<(String, String)> {
    let https = identity::https_of(remote)?;
    let path = https.strip_prefix("https://")?.split_once('/')?.1;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    Some((owner.to_owned(), name.to_owned()))
}
