//! Hand off: the person gives what they typed to an agent that works on it
//! alone, in a checkout of its own and out of sight, tracked as a card on the
//! board.

use crate::server::board_state::BoardParts;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::board::{BoardHandOffParams, HandedOff};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::{add_card, claim_card, long_text, project_at, project_view, title_of, Adding, By};
use crate::server::connection::blocking;
use crate::server::names::Starting;
use crate::server::worktree::agent::{self, Unclaimed};
use crate::server::worktree::{self, AgentWorktree};
use crate::server::{chat, workspace, Core};
use crate::text::clipped_line;
use sikemux_wire::protocol::{ChatContext, Event, PermissionMode, FOR_AGENT_URI};

/// The longest card title made from what was typed; the whole text goes into its details.
const TITLE_CHARS: usize = 80;

/// The agent started and the card it holds, before it is sent its task.
struct Started {
    project: String,
    card: u64,
    checkout: AgentWorktree,
    cwd: std::path::PathBuf,
}

/// Starts an agent on what the person typed in a checkout of its own, cut
/// from where the project is now, with the card that tracks it. Nothing comes
/// to the front: the work shows on the board, and reaches the person only
/// when it needs them.
pub async fn hand_off(core: &Arc<Core>, params: BoardHandOffParams) -> Result<HandedOff, String> {
    let text = long_text("text", &params.text)?;
    if text.is_empty() {
        return Err("Say what should get done".into());
    }
    let title = title_of(&clipped_line(&text, TITLE_CHARS))?;
    let held = HeldCard {
        project: String::new(),
        details: if title == text {
            String::new()
        } else {
            text.clone()
        },
        title,
        agent: String::new(),
    };
    let worker = Worker {
        project: params.project,
        launcher: params.launcher,
        provider: params.provider,
        model: params.model,
        permission_mode: params.permission_mode,
        effort: params.effort,
    };
    let (agent, started) = start_worker(core, worker, text.clone(), move |core, project, agent| {
        place(
            core,
            HeldCard {
                project: project.to_owned(),
                agent: agent.to_owned(),
                ..held
            },
        )
    })
    .await?;
    let board = project_view(core, &started.project);
    let card = started.card;
    let core = core.clone();
    let worker = agent.clone();
    tokio::spawn(async move {
        hand_task(
            &core,
            &worker,
            started,
            text,
            params.files,
            chat::Whose::PERSON,
        )
        .await
    });
    Ok(HandedOff { card, agent, board })
}

/// The card a handed-off agent holds from the start.
#[derive(Serialize, Deserialize)]
pub struct HeldCard {
    project: String,
    title: String,
    #[serde(default)]
    details: String,
    agent: String,
}

/// Puts `held` on its project's board, at the project's home when another host keeps the board.
fn place(core: &Arc<Core>, held: HeldCard) -> Result<u64, String> {
    let params = serde_json::to_value(&held).map_err(|error| error.to_string())?;
    match (core.board_above().records_act)(core, &held.project, "boardHeldCard", params) {
        Some(placed) => placed?
            .as_u64()
            .ok_or_else(|| "the project's home answered without a card".into()),
        None => held_card(core, held),
    }
}

fn held_card(core: &Arc<Core>, held: HeldCard) -> Result<u64, String> {
    let adding = Adding {
        title: held.title,
        details: held.details,
        ..Adding::default()
    };
    add_card(core, &held.project, &By::Person, adding, Some(&held.agent)).map(|(card, _)| card)
}

/// A linked host handed work to one of its agents: its card goes on the board this host keeps.
pub async fn person_held_card(core: &Arc<Core>, mut held: HeldCard) -> Result<u64, String> {
    let core = core.clone();
    blocking(move || {
        held.project = project_at(&core, &held.project)?;
        Ok(held_card(&core, held)?)
    })
    .await
    .map_err(|error| error.to_string())
}

/// Starts an agent in a checkout of its own on card `card` of `project`, which a trigger added
/// and nobody holds, and sends it the card's title and details as its task.
pub async fn take_up(core: Arc<Core>, project: String, card: u64, task: String) {
    let worker = Worker {
        project,
        launcher: None,
        provider: None,
        model: None,
        permission_mode: None,
        effort: None,
    };
    let started = start_worker(&core, worker, task.clone(), move |core, project, agent| {
        claim_card(core, project, agent, card).map(|_| card)
    })
    .await;
    match started {
        Ok((agent, started)) => {
            hand_task(
                &core,
                &agent,
                started,
                task,
                Vec::new(),
                chat::Whose::NOT_PERSON,
            )
            .await
        }
        Err(error) => eprintln!("sikemux core: no agent took up card #{card}: {error}"),
    }
}

/// The agent to start: in which project, and on what.
struct Worker {
    project: String,
    launcher: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    permission_mode: Option<PermissionMode>,
    effort: Option<String>,
}

/// Starts an agent in a checkout of its own, cut from where the project is now, with the branch
/// named after `text`, and puts it on the card `place` answers with. Answers with the agent.
async fn start_worker(
    core: &Arc<Core>,
    worker: Worker,
    text: String,
    place: impl FnOnce(&Arc<Core>, &str, &str) -> Result<u64, String> + Send + 'static,
) -> Result<(String, Started), String> {
    workspace::launchers_known(core).await;
    let agent = crate::server::names::new_agent_id();
    let started = {
        let (core, agent) = (core.clone(), agent.clone());
        blocking(move || {
            let project = project_at(&core, &worker.project)?;
            let folder = Path::new(&project);
            let (checkout, cwd) = agent::cut_for(folder, folder, &worktree::branch_for(&text))?;
            let mut unclaimed = Unclaimed(Some(checkout.clone()));
            let (mut launch, launcher) = core.workspaces().board_launch(
                worker.launcher.as_deref(),
                &cwd,
                worker.provider.as_deref(),
                worker.model,
                "in the start box's agent menu",
            )?;
            if let Some(mode) = worker.permission_mode {
                launch.permission_mode = mode;
            }
            launch.effort = worker.effort;
            core.names().give(
                Starting {
                    agent_id: &agent,
                    project: project.clone(),
                    provider: Some(&launch.provider),
                    resume_id: None,
                },
                core.open_agents(),
            );
            launch.agent_id = agent.clone();
            let chat = chat::begin(&core, launch, None, Some(launcher), false, false)
                .inspect_err(|_| core.names().forget(&agent))?;
            let mut kept = checkout.clone();
            crate::server::ports::checkout::claim(&core, &mut kept);
            let placed = core
                .workspaces()
                .edit_agent(&agent, |worker| worker.worktree = Some(kept))
                .map_err(|error| error.to_string())
                .and_then(|_| place(&core, &project, &agent));
            let card = match placed {
                Ok(card) => card,
                Err(error) => {
                    let _ = workspace::close_agent(&core, agent);
                    return Err(error.into());
                }
            };
            unclaimed.0 = None;
            core.changed(What::Event(&Event::ChatBegun { chat: chat.info() }));
            core.changed(What::View);
            Ok(Started {
                project,
                card,
                checkout,
                cwd,
            })
        })
        .await
        .map_err(|error| error.to_string())?
    };
    Ok((agent, started))
}

/// Sets up the agent's checkout as the project asks, then sends it `text`, marked as the
/// person's, with how its work is landed.
async fn hand_task(
    core: &Arc<Core>,
    agent: &str,
    started: Started,
    text: String,
    files: Vec<String>,
    whose: chat::Whose,
) {
    let Started {
        project,
        card,
        checkout,
        cwd,
    } = started;
    let title = (core.boards().state.locked().of(&project))
        .and_then(|board| board.card(card).ok())
        .map(|held| held.title.clone());
    workspace::retitle(core, agent, title, true);
    let set_up = agent::set_up(core, &checkout.repo, Some(agent), &cwd).await;
    let base = checkout.base.as_deref().unwrap_or("the project's branch");
    let mut message = format!(
        "This is card #{card} on the project's board, which you hold. You work on it alone, in a checkout of your own on branch {} cut from {base}, and nobody watches this chat: if you need me, ask with ask.",
        checkout.branch
    );
    if let Some(error) = set_up {
        message.push_str(&format!(
            "\n\nYour checkout's setup did not all finish: {error}"
        ));
    }
    message.push_str(&format!(
        "\n\nWhen it is done, commit it with git_commit and land it on {base} with land true, then mark the task done with task_update."
    ));
    if let Some(chat) = core.chats().get(agent) {
        chat.take_brief();
    }
    if let Some(brief) = (core.board_above().memory_brief)(core, agent, Some(&text)) {
        message.push_str(&format!("\n\n{brief}"));
    }
    let tracking = ChatContext {
        uri: format!("{FOR_AGENT_URI}card-{card}"),
        title: format!("Card #{card}"),
        text: message,
    };
    if let Err(error) = chat::prompt(core, agent, None, text, files, vec![tracking], whose) {
        eprintln!("sikemux core: the agent handed card #{card} never got its task: {error}");
    }
}
