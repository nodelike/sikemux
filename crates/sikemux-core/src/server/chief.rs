//! Each project's chief, @Chief: one persistent chat per project that every other agent there
//! works under. It is a chat like any other, on the agent and model a new chat in the project
//! would use, with a seat: a brief from the core and a session kept small. Clients draw its row
//! for every project from the project itself; its chat is made the first time the person opens
//! that row, and its agent starts only once they write to it.

use std::collections::HashSet;
use std::sync::Arc;

use sikemux_view::workspace::ProjectInfo;

use super::launching::{self, Launch};
use super::{Core, CoreResult};

/// What every project's chief is called, which no other agent may be.
pub(crate) const NAME: &str = "Chief";

/// How the chief works, ahead of the project's rules in its instructions.
pub(crate) const SEAT: &str = "You are @Chief, this project's chief in Sikemux. The person talks to you; every other agent in the project works under you. Your job is to decide and route, not to do the work yourself.

- Decide and record, don't ask. Anything inside the machine is yours: who works on what, on which model, in what order, how to check it, landing by the project's rule, deploys to places that are not production. Record each decision with board_decide, with what it rests on, so the person can overrule it.
- Raise only four kinds of call, with ask: a change to a production place, spending money beyond plans already paid for, taste or direction the person hasn't spoken on, and access only they have. Word each in what the person asked for, give your pick and the strongest point against it. If it cannot be worded that way, it is not theirs: decide it.
- Do not ask before a change to a production place: make it, and Sikemux's gate holds it and raises the call. A level of trust moves when the person says so; for a production place only through a call they tap.
- Start workers with agent_spawn for anything longer than a few minutes, and stay thin: shell output and file reads belong to workers. Their reports reach you; read more with agent_read only when you need it.
- Report a result as what changed, where it is and what to try. Never say something is done unless Sikemux's state line for that work says so.
- Come back to the person at five moments only: a call; done, with where the work is and what to try; slipped, with the new time and why; stuck on something only they have; and when they ask.";

/// What the chief is handed as it starts: how it works, then the project's rules and knowledge.
pub(crate) fn brief(core: &Core, cwd: &std::path::Path) -> String {
    match super::memory::for_chat(core, cwd) {
        Some(kept) => format!("{SEAT}\n\n{kept}"),
        None => SEAT.to_owned(),
    }
}

/// Every project's chief, by agent id.
pub(crate) fn all(core: &Core) -> HashSet<String> {
    core.workspaces.chiefs()
}

/// Whether `agent_id` is a project's chief.
pub(crate) fn is_chief(core: &Core, agent_id: &str) -> bool {
    core.workspaces.is_chief(agent_id)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenChiefParams {
    /// The project, by id.
    pub project: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedChief {
    pub agent_id: String,
}

/// Opens a project's chief, as the person opening its row or writing to it does: made the
/// first time, waiting for the person, and brought back as itself after a close.
pub(crate) async fn open(core: &Arc<Core>, params: OpenChiefParams) -> CoreResult<OpenedChief> {
    static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one_at_a_time = OPENING.lock().await;
    let project = (core.workspace_view().projects.into_iter())
        .find(|project| project.id == params.project)
        .ok_or("that project is not on this host")?;
    let agent_id = seat(core, &project).await?;
    Ok(OpenedChief { agent_id })
}

/// The project's chief, open: itself while it is, taken up again as itself after a close,
/// else made, waiting for the person.
async fn seat(core: &Arc<Core>, project: &ProjectInfo) -> CoreResult<String> {
    match &project.chief {
        Some(chief) if core.workspaces.agent(chief).is_some() => Ok(chief.clone()),
        Some(chief) if core.names.ran_session(chief).is_some() => {
            launching::reopen_agent(core, chief.clone())
                .await
                .map(|taken| taken.agent_id)
        }
        Some(chief) => make(core, project, chief.clone()).await,
        None => {
            let chief = super::names::new_agent_id();
            core.workspaces.set_chief(&project.id, &chief)?;
            make(core, project, chief).await
        }
    }
}

/// Starts the chief's chat on the launcher a new chat in the project would use, waiting for
/// the person.
async fn make(core: &Arc<Core>, project: &ProjectInfo, chief: String) -> CoreResult<String> {
    let launcher = (project.launcher.clone()).ok_or("no agent here can run it")?;
    let kind = super::workspace::LauncherId::parse(&launcher)
        .kind
        .to_owned();
    let pick = core.workspaces.model_pick(&kind).unwrap_or_default();
    core.names
        .name_chief(&chief, &core.workspaces.project_of(&project.path));
    let chosen = Launch {
        agent_id: chief,
        launcher,
        cwd: project.path.clone(),
        permission_mode: None,
        model: pick.model,
        model_provider: None,
        effort: pick.effort,
        resume_id: None,
        account: None,
        fallbacks: Vec::new(),
        program: None,
        waits_for_person: true,
    };
    let agent_id = chosen.agent_id.clone();
    launching::start(core, None, chosen).await?;
    Ok(agent_id)
}
