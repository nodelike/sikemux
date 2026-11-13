//! Bringing a chat up: from a client's ask, a hand-over, a restart or an agent that started it.

use super::offers::offer_options;
use super::*;
use crate::server::activity::ActivityCore;
use crate::server::parts::Parts;
use crate::server::ServerCore;

/// The notice a chat shows when an update started its agent again rather
/// than carrying it over.
pub(crate) fn update_notice(why: &str) -> Value {
    notice(
        "warning",
        "Sikemux restarted this agent while it updated",
        Some(&format!(
            "The agent could not be carried over because {why}. Anything it was doing stopped."
        )),
    )
}

/// A chat that is listed but never started, for tests of what surrounds it.
#[cfg(test)]
pub(crate) fn idle(core: &Arc<Core>, agent_id: &str, provider: &str, cwd: &str) -> Arc<Chat> {
    let launch = ChatLaunch {
        agent_id: agent_id.into(),
        provider: provider.into(),
        cwd: cwd.into(),
        program: "/bin/false".into(),
        args: Vec::new(),
        env: Default::default(),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: PermissionMode::WorkspaceWrite,
        model: None,
        model_provider: None,
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    };
    let (chat, _) = core
        .chats()
        .insert(core, launch, Origin::default(), None)
        .unwrap_or_else(|error| panic!("{error}"));
    chat
}

pub(super) fn validate_account(account: &ChatAccount) -> CoreResult<()> {
    bounded_text("account id", &account.id, 256)?;
    bounded_text("account name", &account.label, 256)?;
    for (key, value) in &account.env {
        bounded_text("account variable", key, 128)?;
        if value.len() > 4_096 || value.contains('\0') {
            return Err("account variables must be bounded text".into());
        }
    }
    Ok(())
}

pub(super) fn validate(launch: &ChatLaunch) -> CoreResult<()> {
    bounded_text("agent id", &launch.agent_id, MAX_AGENT_ID)?;
    bounded_text("provider", &launch.provider, 64)?;
    bounded_text("working directory", &launch.cwd.to_string_lossy(), 4_096)?;
    if !launch.cwd.is_absolute() {
        return Err("A chat's working directory must be absolute".into());
    }
    if let Some(resume_id) = launch.resume_id.as_deref() {
        let agent = crate::agent_catalog::AgentKind::from_name(&launch.provider);
        if !crate::sessions::is_session_id(agent, resume_id) {
            return Err(format!("{resume_id:?} is not a {} session id", launch.provider).into());
        }
    }
    if launch.program.as_os_str().is_empty() {
        return Err("This agent's program is missing".into());
    }
    for account in launch.account.iter().chain(&launch.fallbacks) {
        validate_account(account)?;
    }
    Ok(())
}

/// Starts the chat's connection. The task ends the chat however the
/// connection ends.
pub(super) fn launch(
    core: &Arc<Core>,
    chat: &Arc<Chat>,
    mut queue: mpsc::UnboundedReceiver<ChatCommand>,
    begin: Begin,
) {
    let ending = chat.clone();
    let owner = core.clone();
    let task = tokio::spawn(async move {
        if ending.unstarted() {
            ending.wanted.notified().await;
            crate::server::activity_log::chat_launched(&owner, &ending.launch);
            ending.keep(kept::Progress::STARTING);
        }
        // The agent can be started again under the chat, on another account
        // or after a sign-in, and the chat carries on through it.
        let mut begin = begin;
        let mut first = ending.current_launch();
        if crate::server::launching::own_home(&owner, &mut first).await {
            ending.set_current(first);
        }
        let result = loop {
            let launch = ending.current_launch();
            let ran = native::run(ending.clone(), launch, &mut queue, begin).await;
            if !matches!(ran, Ok(rebind::Outcome::Released)) {
                ending.feed.agent_gone();
                ending.drop_model_grant();
            }
            match ran {
                Ok(rebind::Outcome::Rebind(next)) => {
                    ending.set_current(next.launch.clone());
                    begin = Begin::Start {
                        rebind: Some(next),
                        notice: None,
                    };
                }
                Ok(rebind::Outcome::Released) => {
                    ending.keeper.leave();
                    return;
                }
                Ok(rebind::Outcome::Ended(end)) => break Ok(end),
                Err(error) => break Err(error),
            }
        };
        ending.keeper.forget();
        ending.feed.close();
        ending.report_end(&owner, &result);
        let started = ending.is_ready();
        ending.emit(ChatEventKind::Status, ended_status(&result, started));
        if let Err(error) = &result {
            ending.fail_for_good(serde_json::json!({ "message": error }));
        }
        ending.end(result.err());
        if owner.chats().remove_if_current(&ending) {
            owner.note_agent_gone(ending.agent_id());
        }
        ending.cancel_permissions();
        ending.publish(AgentState::Stopped);
    });
    let mut slot = chat.task.locked();
    *slot = Some(task);
}

/// Starts the chat's connection, and shows a chat that waits for the person what it can
/// before its agent starts: its saved history and what it offers to pick.
fn bring_up(
    core: &Arc<Core>,
    chat: &Arc<Chat>,
    queue: mpsc::UnboundedReceiver<ChatCommand>,
    begin: Begin,
) {
    launch(core, chat, queue, begin);
    if chat.unstarted() {
        native::show_saved(chat);
        offer_options(core, chat);
    }
}

fn wait_for_person(chat: &Chat) {
    chat.feed.wait_for_person();
    chat.unstarted.store(true, Ordering::Release);
}

/// Starts a chat agent, with `subscriber` hearing it from its first event.
/// The chat is known to the core when this returns, so a stop asked for
/// next finds it. With `waits_for_person`, its agent starts only once the
/// person writes in the chat or something is sent to it. `briefed` when its
/// first task carries what Sikemux keeps for the project already, as Auto's does.
pub(crate) fn begin(
    core: &Arc<Core>,
    launch_spec: ChatLaunch,
    subscriber: Option<&Arc<ClientConn>>,
    launcher: Option<String>,
    waits_for_person: bool,
    briefed: bool,
) -> CoreResult<Arc<Chat>> {
    let starting = Starting {
        waits_for_person,
        briefed,
        notice: None,
    };
    begin_as(core, launch_spec, subscriber, launcher, starting)
}

/// Starts a chat as [`begin`] does, as `starting` says.
pub(crate) fn begin_as(
    core: &Arc<Core>,
    launch_spec: ChatLaunch,
    subscriber: Option<&Arc<ClientConn>>,
    launcher: Option<String>,
    starting: Starting,
) -> CoreResult<Arc<Chat>> {
    let origin = Origin {
        started_by: subscriber.and_then(|client| client.peer.device_id()),
        launcher,
    };
    start(core, launch_spec, origin, subscriber, starting)
}

/// How a chat starts: once the person writes in it or at once, with its brief already in its
/// launch or not, and what it says once its agent is up.
pub(crate) struct Starting {
    pub waits_for_person: bool,
    pub briefed: bool,
    pub notice: Option<Value>,
}

/// Starts a chat agent another agent asked for, with `lead` as who started it.
pub(crate) fn begin_worker(
    core: &Arc<Core>,
    launch_spec: ChatLaunch,
    lead: &str,
    launcher: Option<String>,
) -> CoreResult<Arc<Chat>> {
    let origin = Origin {
        started_by: Some(lead.to_owned()),
        launcher,
    };
    let starting = Starting {
        waits_for_person: false,
        briefed: true,
        notice: None,
    };
    start(core, launch_spec, origin, None, starting)
}

pub(super) fn start(
    core: &Arc<Core>,
    mut launch_spec: ChatLaunch,
    origin: Origin,
    subscriber: Option<&Arc<ClientConn>>,
    starting: Starting,
) -> CoreResult<Arc<Chat>> {
    let Starting {
        waits_for_person,
        briefed,
        notice,
    } = starting;
    crate::server::launching::own_tools(core, &mut launch_spec);
    rebind::name_accounts(core, &mut launch_spec);
    for (key, value) in crate::server::plugins::agent_environment(core, &launch_spec.cwd) {
        launch_spec.env.entry(key).or_insert(value);
    }
    let here = crate::server::ports::checkout::here(core, &launch_spec.cwd);
    for (key, value) in here.map(|here| here.environment()).unwrap_or_default() {
        launch_spec.env.entry(key).or_insert(value);
    }
    validate(&launch_spec)?;
    let resume_id = launch_spec.resume_id.clone();
    let waking = core.workspaces().agent(&launch_spec.agent_id).is_some();
    let (chat, queue) = core.chats().insert(core, launch_spec, origin, None)?;
    match &resume_id {
        None if !briefed => {
            *chat.brief.locked() = crate::server::memory::for_chat(core, &chat.launch.cwd);
        }
        Some(session) if !waking => {
            *chat.brief.locked() = crate::server::board::lease::resumed_session(
                core,
                chat.agent_id(),
                chat.provider(),
                session,
            );
        }
        _ => {}
    }
    if crate::server::chief::is_chief(core, chat.agent_id()) {
        *chat.brief.locked() = Some(crate::server::chief::brief(core, &chat.launch.cwd));
    }
    core.workspaces().note_begun(&chat);
    crate::server::model_providers::chat_begun(core, &chat.launch);
    let naming = (
        core.clone(),
        chat.agent_id().to_owned(),
        chat.launch.cwd.clone(),
        chat.provider().to_owned(),
    );
    tokio::task::spawn_blocking(move || {
        let (core, agent_id, cwd, provider) = naming;
        core.name_agent(&agent_id, &cwd, Some(&provider), resume_id.as_deref());
    });
    if !waking {
        core.seen().active(chat.agent_id());
    }
    if !waits_for_person {
        crate::server::activity_log::chat_launched(core, &chat.launch);
    }
    if waits_for_person {
        wait_for_person(&chat);
    }
    let notice = match notice {
        Some(update) if waits_for_person => {
            super::offers::tell_update(&chat, update);
            None
        }
        notice => notice,
    };
    chat.keep(kept::Progress::STARTING);
    if let Some(client) = subscriber {
        chat.feed.subscribe(client);
    }
    bring_up(
        core,
        &chat,
        queue,
        Begin::Start {
            rebind: None,
            notice,
        },
    );
    Ok(chat)
}

/// Answers once the chat's session is ready, or says why it never got there.
pub(crate) async fn until_started(core: &Arc<Core>, chat: Arc<Chat>) -> CoreResult<ChatStart> {
    match tokio::time::timeout(START_TIMEOUT, chat.until_ready()).await {
        Ok(result) => result.map_err(CoreError::from),
        Err(_) => {
            if core.chats().remove_if_current(&chat) {
                core.note_agent_gone(chat.agent_id());
            }
            Chats::halt(&chat);
            Err(format!(
                "The agent did not become ready within {} seconds",
                START_TIMEOUT.as_secs()
            )
            .into())
        }
    }
}

/// Starts again a chat an earlier core handed over. Its events wait in its
/// replay for a client to attach.
pub(crate) fn resume(core: &Arc<Core>, record: ChatRecord) {
    let agent_id = record.launch.agent_id.clone();
    if let Err(error) = validate(&record.launch) {
        eprintln!("sikemux core: chat {agent_id} was not resumed after the update: {error}");
        return;
    }
    let notice = record.notice.as_deref().map(update_notice);
    match core
        .chats()
        .insert(core, record.launch, record.origin, None)
    {
        Ok((chat, queue)) => {
            if record.unstarted {
                wait_for_person(&chat);
            }
            chat.keep(kept::Progress::STARTING);
            bring_up(
                core,
                &chat,
                queue,
                Begin::Start {
                    rebind: None,
                    notice,
                },
            );
        }
        Err(error) => {
            eprintln!("sikemux core: chat {agent_id} was not resumed after the update: {error}")
        }
    }
}

/// What an agent whose turn was cut short is sent to carry it on, `what`
/// saying what cut it short.
pub(crate) fn carry_on_prompt(what: &str) -> rebind::Replay {
    rebind::Replay {
        announce: false,
        message_id: None,
        text: format!("{what}, so your last turn stopped part-way. Check what got done, since a step that was cut off may not have finished, then carry on where you left off."),
        paths: Vec::new(),
        context: Vec::new(),
        person: false,
        answered: None,
        brief_for: None,
    }
}

pub(super) const RESTARTED: &str = "Sikemux restarted while this agent was working";

/// Takes up again every chat an earlier core was running when it stopped,
/// but for those this core already runs and those put to sleep as it
/// stopped. Each is listed at once, and starts on its saved session in the
/// background.
pub(crate) fn restore(core: &Arc<Core>) {
    let Some(dir) = core.chats().kept.clone() else {
        return;
    };
    for kept in kept::read(&dir) {
        let agent_id = kept.launch.agent_id.clone();
        let asleep = (core.workspaces().agent(&agent_id)).is_some_and(|agent| agent.asleep);
        match core.chats().get(&agent_id) {
            Some(chat) if chat.feed.id() == kept.feed => {}
            Some(_) => kept::forget(&dir, &kept.feed),
            None if !kept.begun() || asleep => {
                if let Some(agent) = kept.agent {
                    agent.stop();
                }
                kept::forget(&dir, &kept.feed);
            }
            None => {
                if let Err(error) = take_up(core, &dir, kept) {
                    eprintln!("sikemux core: chat {agent_id} was not taken up again: {error}");
                }
            }
        }
    }
}

pub(super) fn take_up(core: &Arc<Core>, dir: &std::path::Path, kept: KeptChat) -> CoreResult<()> {
    if let Err(error) = validate(&kept.launch) {
        kept::forget(dir, &kept.feed);
        return Err(error);
    }
    if let Some(agent) = kept.agent {
        agent.stop();
    }
    let launch_spec = &kept.launch;
    let feed = Feed::recovering(
        launch_spec.agent_id.clone(),
        launch_spec.permission_mode,
        core.chats().history.as_deref(),
        &kept.feed,
    );
    kept::moved(dir, &kept, feed.id());
    let saved = launch_spec.resume_id.is_some();
    let flags = TurnProgress::saved(saved);
    let (chat, queue) = core.chats().insert(
        core,
        launch_spec.clone(),
        kept.origin.clone(),
        Some((feed, flags)),
    )?;
    if kept.unstarted {
        wait_for_person(&chat);
    }
    let core = core.clone();
    tokio::spawn(async move {
        let reading = (core.clone(), chat.clone());
        let read = tokio::task::spawn_blocking(move || {
            let (core, chat) = reading;
            if let Some(history) = core.chats().history.as_deref() {
                chat.feed.recover(history);
            }
            kept.unsealed(&core).map(|launch| (launch, kept))
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|read| read);
        match read {
            Ok((launch_spec, _)) if chat.unstarted() => {
                chat.set_current(launch_spec);
                // What it had shown came back with its feed, and must not be told twice.
                chat.saved_shown
                    .store(chat.feed.holds_said(), Ordering::Release);
                bring_up(
                    &core,
                    &chat,
                    queue,
                    Begin::Start {
                        rebind: None,
                        notice: None,
                    },
                );
            }
            Ok((launch_spec, kept)) => carry_on(&core, &chat, queue, launch_spec, &kept),
            Err(error) => {
                chat.end(Some(error.clone()));
                chat.feed
                    .emit(ChatEventKind::Error, rebind::error_message(&error));
                Chats::halt(&chat);
                if core.chats().remove_if_current(&chat) {
                    core.note_agent_gone(chat.agent_id());
                }
            }
        }
    });
    Ok(())
}

/// Starts a chat taken up after a restart on its saved session, ending the
/// turn the restart cut short and asking the agent to carry it on.
pub(super) fn carry_on(
    core: &Arc<Core>,
    chat: &Arc<Chat>,
    queue: mpsc::UnboundedReceiver<ChatCommand>,
    mut launch_spec: ChatLaunch,
    kept: &KeptChat,
) {
    // The kept record is written after the feed, so it may still say a turn is at work
    // whose end the feed has on disk.
    let working = kept.working && !chat.feed.settled();
    chat.feed.agent_gone();
    chat.cut_turn();
    let resumes = kept.reloadable && launch_spec.resume_id.is_some();
    let (replay, notice) = match (working, resumes) {
        (true, true) => (
            Some(carry_on_prompt("Sikemux restarted while you were working")),
            Some(restart_notice(
                RESTARTED,
                "It was asked to check what got done and carry on.",
            )),
        ),
        (true, false) => (
            kept.prompt.clone(),
            Some(restart_notice(
                RESTARTED,
                "This agent cannot load its saved conversation, so it started a new one on your last message.",
            )),
        ),
        (false, _) => (kept.offline.clone(), None),
    };
    if !kept.reloadable {
        launch_spec.resume_id = None;
    }
    chat.set_current(launch_spec.clone());
    bring_up(
        core,
        chat,
        queue,
        Begin::Start {
            rebind: Some(Box::new(rebind::Rebind {
                launch: launch_spec,
                replay,
                exhausted: Vec::new(),
                signed_in_again: false,
                notice,
            })),
            notice: None,
        },
    );
}

/// Takes up a chat an earlier core handed over whole, its agent still
/// running on `pipes`. Answers with why it could not be.
pub(crate) fn adopt(
    core: &Arc<Core>,
    live: LiveChat,
    pipes: [std::os::fd::OwnedFd; 3],
    subscribers: &[Arc<ClientConn>],
) -> Result<(), String> {
    validate(&live.current).map_err(|error| error.to_string())?;
    let feed = Feed::restored(live.launch.agent_id.clone(), live.feed);
    let (chat, queue) = core
        .chats()
        .insert(core, live.launch, live.origin, Some((feed, live.flags)))
        .map_err(|error| error.to_string())?;
    chat.set_current(live.current);
    if let Some(kept) = live.model_grant {
        chat.hold_model_grant(core.model_proxy().restore(kept));
    }
    if let Some(version) = &live.cli_version {
        chat.note_cli_version(version);
    }
    for client in subscribers {
        chat.feed.subscribe(client);
    }
    if chat.turn_running() {
        core.seen().started(chat.agent_id());
    }
    if !live.asked.is_empty() {
        core.seen().wants_a_look(chat.agent_id());
    }
    bring_up(
        core,
        &chat,
        queue,
        Begin::Adopt(Box::new(native::Adopted {
            session: live.session,
            backend: live.backend,
            agent: live.agent,
            pipes,
            asked: live.asked,
        })),
    );
    Ok(())
}
