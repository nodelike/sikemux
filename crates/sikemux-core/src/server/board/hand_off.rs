//! Hand off: the person gives what they typed to an agent that works on it
//! alone, in a checkout of its own and out of sight, tracked as a slice on the
//! board whose promise is what they typed. Its Built gate needs the project's
//! check to pass on its work, run by the core.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    add_card, long_text, project_at, project_view, slice, title_of, Adding, BoardView, By,
};
use crate::protocol::Event;
use crate::server::connection::blocking;
use crate::server::names::Starting;
use crate::server::worktree::agent::{self, Unclaimed};
use crate::server::worktree::{self, AgentWorktree};
use crate::server::{chat, workspace, Core};
use crate::text::{clipped, clipped_line};

/// The longest card title made from what was typed; the whole text goes into its details.
const TITLE_CHARS: usize = 80;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandOffParams {
    pub project: String,
    /// What the person typed, which the agent is sent and the slice promises.
    pub text: String,
    /// The files that go with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// The app's launcher the agent starts with; none for the first that works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The connected provider it runs on in place of its own account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// The slice that tracks the work, and the agent doing it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandedOff {
    pub card: u64,
    pub agent: String,
    pub board: BoardView,
}

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
pub(crate) async fn hand_off(core: &Arc<Core>, params: HandOffParams) -> Result<HandedOff, String> {
    let text = long_text("text", &params.text)?;
    if text.is_empty() {
        return Err("Say what should get done".into());
    }
    let title = title_of(&clipped_line(&text, TITLE_CHARS))?;
    let promise = slice::promise_of(&clipped(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        slice::MAX_PROMISE_CHARS,
    ))?;
    let adding = Adding {
        details: if title == text {
            String::new()
        } else {
            text.clone()
        },
        title,
        promise: Some(promise),
        ..Adding::default()
    };
    let agent = format!("agent-{}", uuid::Uuid::new_v4().simple());
    let started = {
        let (core, agent, text) = (core.clone(), agent.clone(), text.clone());
        let (project, launcher, provider, model) = (
            params.project,
            params.launcher,
            params.provider,
            params.model,
        );
        blocking(move || {
            let project = project_at(&core, &project)?;
            let folder = Path::new(&project);
            let (checkout, cwd) = agent::cut_for(folder, folder, &worktree::branch_for(&text))?;
            let mut unclaimed = Unclaimed(Some(checkout.clone()));
            let (mut launch, launcher) = core.workspaces.board_launch(
                launcher.as_deref(),
                &cwd,
                provider.as_deref(),
                model,
                "in the start box's agent menu",
            )?;
            core.names.give(
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
                .inspect_err(|_| core.names.forget(&agent))?;
            let placed = core
                .workspaces
                .edit_agent(&agent, |worker| worker.worktree = Some(checkout.clone()))
                .map_err(|error| error.to_string())
                .and_then(|_| add_card(&core, &project, &By::Person, adding, Some(&agent)));
            let card = match placed {
                Ok((card, _)) => card,
                Err(error) => {
                    let _ = workspace::close_agent(&core, agent);
                    return Err(error.into());
                }
            };
            unclaimed.0 = None;
            core.broadcast_local(&Event::ChatBegun { chat: chat.info() });
            core.view_changed();
            super::tell_clients(&core);
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
    let board = project_view(core, &started.project);
    let card = started.card;
    let core = core.clone();
    let worker = agent.clone();
    tokio::spawn(async move { hand_task(&core, &worker, started, text, params.files).await });
    Ok(HandedOff { card, agent, board })
}

/// Sets up the agent's checkout as the project asks, then sends it what the
/// person typed, as the person, with how its work is checked and landed.
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
    let set_up = agent::set_up(core, &checkout.repo, Some(agent), &cwd).await;
    let checked = {
        let (owner, project) = (core.clone(), project.clone());
        blocking(move || Ok(super::project_check(&owner, &project)))
            .await
            .ok()
            .flatten()
    };
    let base = checkout.base.as_deref().unwrap_or("the project's branch");
    let mut message = format!(
        "{text}\n\nThis is card #{card} on the project's board, which you hold; what I wrote above is its promise. You work on it alone, in a checkout of your own on branch {} cut from {base}, and nobody watches this chat: if you need me, ask with ask.",
        checkout.branch
    );
    if let Some(error) = set_up {
        message.push_str(&format!(
            "\n\nYour checkout's setup did not all finish: {error}"
        ));
    }
    message.push_str(&match checked {
        Some(check) => format!(
            "\n\nWhen it is done, commit it with git_commit and land it on {base} with land true, then mark the card done with board_update. It is Built once the project's check, {}, passes on it there, which Sikemux runs itself.",
            check.label()
        ),
        None => format!(
            "\n\nWhen it is done, commit it with git_commit and land it on {base} with land true. This project names no check, so nothing Sikemux runs can show the work Built yet: ask me how it is to be checked before you mark the card done."
        ),
    });
    if let Err(error) = chat::prompt(core, agent, None, message, files, Vec::new()) {
        eprintln!("sikemux core: the agent handed card #{card} never got its task: {error}");
    }
}
