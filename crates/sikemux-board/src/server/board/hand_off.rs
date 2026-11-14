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

use sikemux_process::sync::Locked;

use super::{add_card, long_text, project_at, project_view, title_of, Adding, By};
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
    let adding = Adding {
        details: if title == text {
            String::new()
        } else {
            text.clone()
        },
        title,
        ..super::Adding::default()
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
        add_card(core, project, &By::Person, adding, Some(agent)).map(|(card, _)| card)
    })
    .await?;
    let board = project_view(core, &started.project);
    let card = started.card;
    let core = core.clone();
    let worker = agent.clone();
    tokio::spawn(async move { hand_task(&core, &worker, started, text, params.files).await });
    Ok(HandedOff { card, agent, board })
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
) {
    let Started {
        project,
        card,
        checkout,
        cwd,
    } = started;
    let title = (core.boards().state.locked().boards.get(&project))
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
        "\n\nWhen it is done, commit it with git_commit and land it on {base} with land true, then mark the card done with board_update."
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
    if let Err(error) = chat::prompt(
        core,
        agent,
        None,
        text,
        files,
        vec![tracking],
        chat::Whose::PERSON,
    ) {
        eprintln!("sikemux core: the agent handed card #{card} never got its task: {error}");
    }
}
