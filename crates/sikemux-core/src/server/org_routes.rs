//! An org's key runs only in the projects shared with that org, while the person holds a seat
//! there. A chat on one anywhere else moves to its agent's own model and says why.

use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::Value;
use sikemux_fold::update::notice;
use sikemux_process::sync::Locked;

use crate::launch::providers;

use super::model_proxy::org_only;
use super::workspace::{update_agent, AgentPatch, UpdateAgentParams};
use super::{launching, shares, Core};

/// Why a chat on `provider`, or on the provider `model` is routed to, may not run in `cwd`,
/// when it may not.
pub(crate) fn refused(
    core: &Core,
    cwd: &Path,
    provider: Option<&str>,
    model: Option<&str>,
) -> Option<String> {
    let route = provider.or_else(|| providers::routed_id(model?))?;
    if !providers::org_route(route) {
        return None;
    }
    let held = (core.workspaces().providers()).and_then(|providers| providers.find(route));
    let org = match held {
        Some(held) => held.org.clone().map(|org| (org.id, org.name))?,
        None => match shares::org_of_route(core, route) {
            Some(org) => (org.id, org.name),
            None => return Some(UNKNOWN_ORG.to_owned()),
        },
    };
    match shares::project_org(core, cwd) {
        Some(allowed) if allowed == org.0 => None,
        _ => Some(org_only(&org.1)),
    }
}

const UNKNOWN_ORG: &str = "An org's key runs only in projects shared with that org";

/// What a chat moved off an org's key says, `why` being [`refused`]'s answer.
pub(crate) fn moved_notice(why: &str) -> Value {
    notice(
        "warning",
        why,
        Some("This chat moved to the agent's own model."),
    )
}

/// The agent's record no longer names the org's model, so it starts on its own from now on.
pub(crate) fn forget_route(core: &Core, agent_id: &str) {
    if core.workspaces().agent(agent_id).is_none() {
        return;
    }
    let patch = AgentPatch {
        model: Some(None),
        model_provider: Some(None),
        ..AgentPatch::default()
    };
    let params = UpdateAgentParams {
        agent_id: agent_id.to_owned(),
        patch,
    };
    if let Err(error) = update_agent(core, params) {
        eprintln!("sikemux core: a chat kept an org's model it may not run on: {error}");
    }
}

/// The chats waiting to move, so a sweep while one waits adds no second mover. Each is kept by
/// where it lives, which stays its own while its mover holds it.
static MOVING: LazyLock<Mutex<HashSet<usize>>> = LazyLock::new(Mutex::default);

struct Mover(usize);

impl Drop for Mover {
    fn drop(&mut self) {
        MOVING.locked().remove(&self.0);
    }
}

/// Moves every running chat on an org's key it may no longer use, once its turn and the subagents it started are over.
pub(crate) fn sweep(core: &Arc<Core>) {
    for chat in core.chats().all() {
        let launch = chat.current_launch();
        let provider = launch.model_provider.as_deref();
        if refused(core, &launch.cwd, provider, launch.model.as_deref()).is_none() {
            continue;
        }
        let at = Arc::as_ptr(&chat) as usize;
        if !MOVING.locked().insert(at) {
            continue;
        }
        let mover = Mover(at);
        let core = core.clone();
        tokio::spawn(async move {
            let _mover = mover;
            chat.settled().await;
            let agent_id = chat.agent_id().to_owned();
            if !(core.chats().get(&agent_id)).is_some_and(|now| Arc::ptr_eq(&now, &chat)) {
                return;
            }
            core.workspaces()
                .note_stopping(&agent_id, chat.current_now());
            core.chats().stop(&agent_id);
            if let Err(error) = launching::start_saved(&core, &agent_id, true).await {
                eprintln!("sikemux core: a chat on an org's key did not move off it: {error}");
            }
        });
    }
}
