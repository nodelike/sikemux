//! What the core gives the modules it stands on to call: each thing they ask of the features
//! built over them, as those features do it.

use crate::server::chat_state::ChatParts;
use crate::server::parts::PartCore;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
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
use super::agent_now::AgentNowCore;
use super::chat_above::ChatAbove;
use super::launching;
use super::notch;
use super::remote_above::RemoteAbove;
use super::repo_above::RepoAbove;
use super::roster;
use super::workspace_above::WorkspaceAbove;
use super::ServerCore;
use super::{
    about, backdrop, board, calls, chat, chief, codehost, git, goal_summary, harness, hosts,
    lifecycle, messages, pins, requests, session_watch, ssh, tasks, themes, triggers, upgrade,
    worktree, Core, CoreResult,
};
use super::{hooks, inbox, stash};
use serde_json::Value;
use sikemux_wire::protocol::ChatLaunch;

use super::chat::Chat;
use super::edits;
use super::tools;

use super::{
    agent_accounts, memory, person_calls, plugins, ports, questions, saved, team_told, turn_state,
};
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

fn account_with_room(
    core: &Core,
    provider: &str,
    launch: &ChatLaunch,
    among: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let chat = agent_accounts::Placing {
        model: launch.model.clone(),
        typical_tokens: None,
        cwd: Some(launch.cwd.clone()),
    };
    agent_accounts::placement(
        &core.workspaces().accounts().unwrap_or_default(),
        provider,
        &chat,
        crate::agent_accounts::allocate::Aim::Headroom,
        |profile| among(&profile.id),
    )
    .map(|placed| placed.id)
}

fn tool_updated(core: &Core, chat: &Chat, update: &Value) {
    core.line().tool_update(chat.agent_id(), update);
    if core
        .collisions()
        .saw(chat.agent_id(), &chat.launch.cwd, update)
    {
        core.edits().command_began(chat.agent_id());
    }
    let noted = (chat.edits.downcast_ref::<edits::Open>()).and_then(|open| open.note(update));
    if let Some((tool_call_id, changes)) = noted {
        core.edits().record(chat, tool_call_id, changes);
    }
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
        note_activity: |core, agent_id, activity| core.note_activity(agent_id, activity),
        note_ending: |core, agent_id, ending| core.note_ending(agent_id, ending),
        note_agent_gone: |core, agent_id| core.note_agent_gone(agent_id),
        chat_infos: |core| core.chat_infos(),
        session_infos: |core| core.session_infos(),
        board_wake: |core| core.boards().wake.now(),
        inbox_not_retrying: |core, agent_id| core.inbox().not_retrying(agent_id),
        inbox_failed: |core, agent_id, said, fault, kind| {
            core.inbox().failed(agent_id, said, fault, kind)
        },
        letter_returned: |core, agent_id, text| core.harness().mailbox.returned(agent_id, text),
        tool_updated,
        new_token: tools::new_token,
        new_edits: || Box::<edits::Open>::default(),
        account_with_room,
        other_agent_with_room: agent_accounts::other_agent_with_room,
        turn_told: turn_state::told,
        permit_elsewhere: |core, agent_id, request_id, option_id| {
            let Some(worker) = core.hosts().team.get(agent_id) else {
                return false;
            };
            hosts::route::permit(core, &worker, request_id, option_id);
            true
        },
        checkout_environment: |core, folder| {
            (ports::checkout::here(core, folder))
                .map(|here| here.environment())
                .unwrap_or_default()
        },
        person_calls_written: person_calls::written,
        locate_saved: saved::locate,
        note_elsewhere: team_told::note_elsewhere,
        open_work: team_told::open_work,
        start_saved: |core, agent_id, waits| {
            Box::pin(launching::start_saved(core, agent_id, waits))
        },
        ensure_running: |core, agent_id, waits| {
            Box::pin(lifecycle::ensure_running(core, agent_id, waits))
        },
        wake_chat: |core, agent_id| Box::pin(launching::wake_chat(core, agent_id)),
        start_new: |core, choice| {
            Box::pin(async move {
                let (agent_id, _) = launching::start_new(core, None, choice).await?;
                Ok(agent_id)
            })
        },
        own_home: |core, launch| Box::pin(launching::own_home(core, launch)),
        chief_digest: |core, chief| Box::pin(chief::digest(core, chief)),
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

pub(super) fn remote_above() -> RemoteAbove {
    RemoteAbove {
        hosts_retry_now: |core| core.hosts().retry_now(),
        hosts_wake: |core| core.hosts().wake.now(),
        hosts_relink: |core| core.hosts().relink.now(),
        serve_link: |core, connection| Box::pin(hosts::serve(core, connection)),
        forget_hosts_off_account: hosts::account::forget_hosts_off_account,
        host_rows: hosts::rows,
        linked_hosts: hosts::views,
        check_updates_soon: |core| core.updates().check_soon(),
        vault_books: |core| {
            (core.config().plugins)
                .map(|plugins::Provided(plugins)| plugins.vault_books())
                .unwrap_or_default()
        },
        stash_heard: stash::heard,
        stash_catch_up: stash::catch_up,
        inbox_told: inbox::told,
        agent_needs: inbox::agent_needs,
        activity_glance: |core| {
            let (chats, sessions) = core.ranked_infos();
            inbox::view(core, &chats, &sessions).glance
        },
        waits_on_answer: |core, agent_id| core.harness().questions.waits_on_answer(agent_id),
        hooks_connect: hooks::connect,
        hosts_halt: hosts::halt,
        hosts_unlink: hosts::unlink,
        hosts_unlinked: hosts::unlinked,
        person_calls_asked_by: person_calls::asked_by,
        person_calls_open_call: person_calls::open_call,
        person_calls_raised_open: person_calls::raised_open,
        turn_state_set_a_wake: turn_state::set_a_wake,
    }
}

pub(super) fn repo_above() -> RepoAbove {
    RepoAbove {
        board_check_stands: board::check_stands,
        board_health_of: board::health_of,
        board_project_at: board::project_at,
        board_committed: board::backup::committed,
        board_landed: board::roles::landed,
        chief_all: chief::all,
        chief_landed: chief::landed,
        questions_asks_production: questions::asks_production,
        questions_cli_of: questions::cli_of,
        roster_listed: roster::listed,
        messages_from_sikemux: |core, to, text| Box::pin(messages::from_sikemux(core, to, text)),
        messages_for_person: |core, to, text| Box::pin(messages::for_person(core, to, text)),
        messages_for_person_waiting: |core, to, text| {
            Box::pin(messages::for_person_waiting(core, to, text))
        },
        messages_work_from_sikemux: |core, to, text, why| {
            Box::pin(messages::work_from_sikemux(core, to, text, why))
        },
        messages_reachable: |core, to| Box::pin(messages::reachable(core, to)),
        agent_chat_state: |core, agent| core.agent_now(agent).chat_state(),
        board_agent_result: |core, agent| {
            board::agent_card(core, agent).and_then(|card| card.result)
        },
        tasks_module_tests: |core, project| Box::pin(tasks::module_tests(core, project)),
        tasks_config_trusted: |core, project, agent| {
            Box::pin(tasks::config_trusted(core, project, agent))
        },
        launching_start_agent: |core, agent| {
            Box::pin(async move {
                launching::start_agent(core, None, agent, false)
                    .await
                    .map(drop)
            })
        },
        host_note: |core, what| core.host_watch().note(what),
        running_sessions: |core| core.running_sessions(),
        agent_running: |core, agent| core.agent_now(agent).running(),
        tasks_config_needs: tasks::config_needs,
        tasks_ask_person: |core, project, ask, execution_id| {
            Box::pin(tasks::ask_person(core, project, ask, execution_id))
        },
        saved_locate: saved::locate,
        board_waiting_on_ci: board::waiting_on_ci,
        board_ci_moved: board::ci_moved,
        triggers_waiting_on_ci: triggers::waiting_on_ci,
        triggers_ci_moved: triggers::ci_moved,
        git_pending_pulls: git::pending_pulls,
        git_ci_moved: git::ci_moved,
        board_landed_set: board::landed_set,
        board_person_settings: |core, params| Box::pin(board::person_settings(core, params)),
        board_mirror_places: board::mirror_places,
        board_landing: board::landing,
        board_record_check: board::record_check,
        board_ci_ruled: board::ci_ruled,
        board_unbuilt_heads: board::unbuilt_heads,
        board_ci_card: board::ci_card,
        harness_note_repository: |core, project, kind| {
            core.harness().note_repository(project, kind)
        },
        questions_hears_hidden: |core, agent| core.harness().questions.hears_hidden(agent),
        questions_answered_by_writing: |core, agent| {
            core.harness().questions.answered_by_writing(agent)
        },
        questions_is_open: |core, id| core.harness().questions.is_open(id),
        questions_answer: |core, params| Box::pin(questions::answer(core, params)),
        questions_answer_all: |core, params| Box::pin(questions::answer_all(core, params)),
        questions_hand_back: |core, id| Box::pin(questions::hand_back(core, id)),
        plugins_caller_of: plugins::caller_of,
        plugins_run_agent_tool: |core, name, arguments, deploy| {
            Box::pin(plugins::run_agent_tool(core, name, arguments, deploy))
        },
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
