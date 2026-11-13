//! What the core gives the modules it stands on to call: each thing they ask of the features
//! built over them, as those features do it.

use crate::server::parts::PartCore;
use crate::server::workspace_state::WorkspaceParts;
use sikemux_wire::protocol::{AgentState, Event, SessionId};

use super::above::Above;
use super::access::{Needs, Program};
use super::activity::{Activity, ActivityCore};
use super::device_view::DeviceViewCore;
use super::notify::Signal;
use super::parts::Parts;
use super::session::Session;
use super::spawn::{self, Ending};
use std::path::PathBuf;

use sikemux_view::prefs::PreferenceChange;

use super::activity_log::{transcript_of, Place};
use super::chat_above::ChatAbove;
use super::launching;
use super::notch;
use super::workspace_above::WorkspaceAbove;
use super::ServerCore;
use super::{
    about, backdrop, board, calls, chat, chief, codehost, git, goal_summary, harness, hosts,
    lifecycle, messages, pins, requests, session_watch, ssh, tasks, themes, triggers, upgrade,
    worktree, Core, CoreResult,
};
use super::{agent_accounts, memory, person_calls, plugins, questions, turn_state};
use crate::launch::named_account;

pub(super) fn above() -> Above {
    Above {
        terminal_colors: |core| core.look().terminal_colors(),
        ports_changed: |core| core.ports().changed(),
        ssh_moved: ssh::moved,
        ssh_connected: |core, alias| hosts::probe::ask(core, alias, hosts::probe::Why::Connected),
        task_ended,
        agent_exited,
        agent_moved,
        view_due: |core| core.device_view().due.notify_one(),
        agents_moved,
        view_moved,
        told: |core, event| {
            if let Some(signal) = signal_of(event) {
                core.notify().send(signal);
            }
        },
        frozen_request: |core, peer, payload, writer| {
            Box::pin(upgrade::answer(core, peer, payload, writer))
        },
        send_device_view: |core, client| core.send_device_view(client),
        run_requests: |core, client, queue| Box::pin(requests::run(core, client, queue)),
        client_joined: |core| core.view_watched(),
        client_gone: |core, client| core.tell_parts_client_gone(client),
        task_output_noticed: harness::note_output,
        admit: requests::admit,
        write_to_program: requests::write_to_program,
    }
}

pub(super) fn workspace_above() -> WorkspaceAbove {
    WorkspaceAbove {
        projects_changed: git::follow_projects,
        project_removed: pins::project_removed,
        code_hosts_picked: |core, path| {
            git::read_hosts(core, vec![path.to_owned()]);
            codehost::picked(core, path);
        },
        agent_closing: |core, agent| {
            triggers::pause(core, &agent.id, core.workspaces().held_chat(agent));
            core.chats().handoffs.forget(&agent.id);
        },
        agent_closed: |core, agent_id| {
            chat::rotation::forget(core, agent_id);
            chief::closed(core, agent_id);
            core.line().agent_closed(core, agent_id);
        },
        worktrees_closed: worktree::build_output::closed,
        title_moved: hosts::route::rows_moved,
        chat_titled: goal_summary::chat_titled,
        agents_gone,
        settled: |core| {
            triggers::resume(core);
            session_watch::follow(core);
        },
        foreign_agents: |core| core.hosts().team.ids(),
        chat_draft_changed: |core, agent| {
            if let Some(feed) = core.chats().heard(agent) {
                feed.set_draft(core.drafts().chat_draft(agent));
            }
        },
        session_deleted: board::lease::session_deleted,
        question_keeps_picture: |core, path| core.harness().questions.keeps_picture(path),
        open_chats: |core| core.chats().list(),
        held_chats: |core| core.triggers().held(),
        workspace_view: |core| core.workspace_view(),
        agents_shown: |core, agent_ids, focused, draws_terminals| {
            if draws_terminals {
                launching::show_terminals(core, agent_ids);
            }
            chief::on_screen(core, agent_ids, focused);
        },
        chat_agents: |core| core.chats().ids(),
        permit: |core, peer, needs| core.permit(peer, needs),
        chat_place,
        preference_changed,
        shown_preferences: |core| core.shown_preferences(),
    }
}

fn chat_place(core: &Core, agent_id: &str) -> Option<Place> {
    let chat = core.chats().get(agent_id)?;
    let launch = chat.current_launch();
    let home = named_account(&launch.provider, &launch.env).map(PathBuf::from);
    let session = chat.feed.start().map(|start| start.session_id);
    Some(Place {
        transcript: transcript_of(&launch.provider, session, home.as_deref()),
        cwd: launch.cwd.to_string_lossy().into_owned(),
        agent: launch.provider,
        chat: agent_id.to_owned(),
        account: launch.account.map(|account| account.id).unwrap_or_default(),
    })
}

fn preference_changed(core: &Core, change: &PreferenceChange) -> CoreResult<()> {
    match change {
        PreferenceChange::Notch(_) => notch::follow(core),
        PreferenceChange::UpdateChannel(_) => core.updates().check_soon(),
        PreferenceChange::PaneShader(_) | PreferenceChange::PaneImage(_) => backdrop::follow(core),
        PreferenceChange::PaneImageFollowsOmarchy(_) => {
            themes::follow_omarchy_background(core);
            backdrop::follow(core);
        }
        PreferenceChange::OnboardingComplete(_) => about::introduced(core)?,
        _ => {}
    }
    core.theme_changed();
    Ok(())
}

fn agents_gone(core: &Core, gone: &[&String]) {
    for agent_id in gone {
        core.chats().stop(agent_id);
        core.chats().forget_rest(agent_id);
    }
    hosts::route::closed(core, gone);
    pins::agents_closed(core, gone);
    if let (Some(core), Ok(runtime), false) = (
        core.arc(),
        tokio::runtime::Handle::try_current(),
        gone.is_empty(),
    ) {
        let gone: Vec<String> = gone.iter().map(|agent_id| (*agent_id).clone()).collect();
        runtime.spawn(async move {
            for agent_id in gone {
                harness::stop_runs(&core, harness::RunSelector::of_agent(&agent_id)).await;
            }
        });
    }
}

pub(super) fn chat_above() -> ChatAbove {
    ChatAbove {
        agent_accounts_note_live: agent_accounts::note_live,
        board_standing_since: board::goal::standing_since,
        memory_brief: memory::brief,
        memory_for_chat: memory::for_chat,
        questions_ask_for_dialog: questions::ask_for_dialog,
        turn_state_now: turn_state::now,
        agent_accounts_chat_refused: agent_accounts::chat_refused,
        board_subagent_started: board::chat_goal::subagent_started,
        board_take_note: board::chat_goal::take_note,
        board_resumed_session: board::lease::resumed_session,
        board_stopped_waiting: board::recover::stopped_waiting,
        chief_brief: chief::brief,
        chief_chat_opened: chief::chat_opened,
        chief_chat_ready: chief::chat_ready,
        chief_keep_session: chief::keep_session,
        chief_kept_session: chief::kept_session,
        chief_person_wrote: chief::person_wrote,
        hosts_name_of: hosts::name_of,
        hosts_rows_moved: hosts::route::rows_moved,
        launching_own_tools: launching::own_tools,
        messages_from_person: messages::from_person,
        messages_person_wrote: messages::person_wrote,
        messages_used_tool: messages::used_tool,
        messages_worked: messages::worked,
        person_calls_hides_typed: person_calls::hides_typed,
        plugins_agent_environment: plugins::agent_environment,
        questions_ask_from_cli: questions::ask_from_cli,
        questions_offers_in_reply: questions::offers_in_reply,
        questions_withdraw_dialogs: questions::withdraw_dialogs,
        triggers_seed: triggers::seed,
    }
}

pub(super) fn access_of(method: &str) -> (Needs, Program) {
    let row = calls::row(method);
    (row.needs, row.program)
}

fn task_ended(core: &Core, id: SessionId, code: Option<u32>, signal: Option<String>) {
    let run = core.harness().run_on(id).map(|run| run.execution_id);
    if let Some(failed) = core.harness().session_exited(id, code, signal) {
        harness::tell_agent_run_failed(core, failed);
    }
    tasks::ended(core, id);
    if let Some(execution_id) = run {
        core.line().run_ended(core, &execution_id);
    }
}

fn agent_exited(
    core: &Core,
    session: &Session,
    code: Option<u32>,
    signal: Option<&str>,
    mid_turn: bool,
) {
    let Some(agent) = session.agent.as_ref() else {
        return;
    };
    lifecycle::terminal_exited(
        core,
        session.id,
        agent.agent_id(),
        code,
        signal,
        session.is_killed(),
    );
    let failed = match (code, signal) {
        (_, Some(signal)) => Some(format!("It stopped from signal {signal}.")),
        (Some(code), None) if code != 0 => Some(format!("It exited with code {code}.")),
        _ => None,
    }
    .filter(|_| !session.is_killed());
    if let Some(reason) = &failed {
        core.note_activity(agent.agent_id(), Activity::Problem(reason));
    }
    core.note_agent_gone(agent.agent_id());
    if mid_turn || !session.is_killed() {
        spawn::terminal_turn_ended(
            core,
            agent.agent_id(),
            Ending::Gone {
                reason: failed,
                mid_turn,
                asked: session.is_killed(),
            },
        );
    }
}

fn agent_moved(
    core: &Core,
    agent_id: &str,
    next: AgentState,
    busy: bool,
    typed: bool,
    last_output_ms: u64,
) {
    let activity = match next {
        AgentState::Working if busy => Some(Activity::Working),
        AgentState::Working => Some(Activity::Started),
        AgentState::Blocked => Some(Activity::NeedsYou),
        AgentState::Idle if busy => Some(Activity::Finished { cancelled: false }),
        _ => None,
    };
    if let Some(activity) = activity {
        core.note_activity(agent_id, activity);
    }
    match next {
        AgentState::Working if typed => messages::prompt_taken(core, agent_id),
        AgentState::Blocked => messages::prompt_taken(core, agent_id),
        AgentState::Idle | AgentState::Stopped if busy => {
            messages::prompt_settled(core, agent_id, last_output_ms)
        }
        _ => {}
    }
    if next == AgentState::Idle && busy {
        spawn::terminal_turn_ended(core, agent_id, Ending::Finished);
    }
}

fn agents_moved(core: &Core) {
    core.harness().mailbox.wake.now();
    core.harness().questions.wake.now();
    core.boards().wake.now();
    core.updates().chats_changed();
}

fn view_moved(core: &Core) {
    core.device_view().stale.notify_one();
    core.hosts().copies.wake.now();
    hosts::route::rows_moved(core);
    core.notify().inbox_moved();
}

fn signal_of(event: &Event) -> Option<Signal> {
    match event {
        Event::Attention { attention } => Some(Signal::Attention(Box::new(attention.clone()))),
        Event::AttentionCleared { id, agent_id } => Some(Signal::Cleared {
            id: id.clone(),
            agent_id: agent_id.clone(),
        }),
        Event::AgentState(state) => Some(Signal::AgentState {
            agent_id: state.agent_id.clone(),
            state: state.state,
        }),
        _ => None,
    }
}
