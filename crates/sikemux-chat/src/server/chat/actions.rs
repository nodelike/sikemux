//! What the core's calls ask of one chat.

use super::offers::{choose_before_start, choose_while_asleep, keep_model_choice};
use super::*;
use crate::agent_accounts::Takers;
use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceParts;

/// The person began writing in a new chat, or a sleeping one, so its agent starts while
/// they write.
pub fn composing(core: &Arc<Core>, agent_id: &str) {
    if let Some(chat) = core.chats().get(agent_id) {
        chat.start_agent();
    } else if stopped(core, agent_id) {
        let (core, agent_id) = (core.clone(), agent_id.to_owned());
        tokio::spawn(async move {
            match wake_shown(&core, &agent_id).await {
                Ok(chat) => chat.start_agent(),
                Err(error) => eprintln!(
                    "sikemux core: a chat the person began writing in did not wake: {error}"
                ),
            }
        });
    }
}

/// Answers once the running chat is ready, or says why it never got there. A new chat
/// still waiting for the person answers at once.
pub async fn until_ready(core: &Core, agent_id: &str) -> CoreResult<()> {
    let chat = core.chats().running(agent_id)?;
    if chat.unstarted() {
        return Ok(());
    }
    chat.until_ready().await.map(drop).map_err(CoreError::from)
}

/// Takes up what an earlier core kept of the agent's chat, asleep or closed, before it is read
/// or woken on `session` if one is named.
pub async fn unshelve(core: &Arc<Core>, agent_id: &str, session: Option<&str>) {
    let (reading, agent, session) = (
        core.clone(),
        agent_id.to_owned(),
        session.map(str::to_owned),
    );
    let taken =
        tokio::task::spawn_blocking(move || reading.chats().unshelve(&agent, session.as_deref()))
            .await;
    if let Ok(Some(feed)) = taken {
        (core.chat_above().kept_feed_taken_up)(core, agent_id, &feed);
    }
}

pub async fn attach(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    agent_id: &str,
    since: Option<ChatMark>,
) {
    let missing = || client.answer::<ask::AcpAttach>(request_id, Ok(ChatAttachment::Missing));
    // A sleeping chat is read from the feed it kept, or else taken up to wait for the person,
    // its saved history shown when the core can read it; neither starts its agent.
    let chat = match core.chats().get(agent_id) {
        Some(chat) => chat,
        None => {
            unshelve(core, agent_id, None).await;
            if let Ok(feed) = core.chats().readable(agent_id) {
                let standing = Standing {
                    turned: false,
                    draft: core.drafts().chat_draft(agent_id),
                };
                feed.attach(client, request_id, standing, since);
                return;
            }
            let in_terminal = core
                .workspaces()
                .agent(agent_id)
                .is_some_and(|agent| agent.terminal);
            let taken_up = stopped(core, agent_id)
                && !in_terminal
                && (core.chat_above().start_saved)(core, agent_id, true)
                    .await
                    .is_ok();
            match core.chats().get(agent_id).filter(|_| taken_up) {
                Some(chat) => chat,
                None => {
                    missing();
                    return;
                }
            }
        }
    };
    // A chat still starting, such as one resumed after an update, is taken up
    // once it is ready. One that never gets there is gone. A new chat waiting
    // for the person is taken up at once, and its start follows as changes.
    if !chat.unstarted() {
        let ready = tokio::time::timeout(START_TIMEOUT, chat.until_ready()).await;
        if !matches!(ready, Ok(Ok(_))) {
            missing();
            return;
        }
    }
    chat.feed.attach(
        client,
        request_id,
        Standing {
            turned: chat.turn.locked().turned,
            draft: core.drafts().chat_draft(agent_id),
        },
        since,
    );
}

pub fn history(
    core: &Core,
    agent_id: &str,
    feed: &str,
    before: u64,
    turns: u32,
) -> CoreResult<sikemux_view::chat::ChatPage> {
    Ok(core
        .chats()
        .readable(agent_id)?
        .history(feed, before, turns as usize)?)
}

/// The rows of a chat a client pages to: before or after an event, or around the row holding
/// one, whole and with their meta, under the window's size.
pub fn page(
    core: &Core,
    params: calls::ChatPageParams,
) -> CoreResult<sikemux_view::chat::ChatPage> {
    use feed::Anchor;
    let anchor = match (params.before, params.after, params.around) {
        (Some(before), None, None) => Anchor::Before(before),
        (None, Some(after), None) => Anchor::After(after),
        (None, None, Some(around)) => Anchor::Around(around),
        _ => return Err("name one of before, after or around".into()),
    };
    let turns = params
        .turns
        .map_or(sikemux_fold::TAIL_TURNS, |turns| turns as usize);
    Ok(core.chats().readable(&params.agent_id)?.page(
        &params.feed,
        anchor,
        turns,
        params.session.as_deref(),
    )?)
}

pub fn find(
    core: &Core,
    client: ClientId,
    params: find::FindParams,
) -> CoreResult<find::FindAnswer> {
    Ok(core
        .chats()
        .readable(&params.agent_id)?
        .find(client, &params)?)
}

pub fn tool_detail(
    core: &Core,
    agent_id: &str,
    tool_call_id: &str,
) -> CoreResult<sikemux_view::chat::ToolDetail> {
    core.chats()
        .running(agent_id)?
        .feed
        .tool_detail(tool_call_id)
        .ok_or_else(|| CoreError::from("that call left nothing to show"))
}

/// The message `message_id` of `agent_id`'s chat with the sentences it left out back in place,
/// for a client that draws only the core's trees. Nothing when the chat keeps no such message.
pub fn left_out(
    core: &Core,
    agent_id: &str,
    message_id: &str,
) -> CoreResult<sikemux_view::chat::ChatLeftOut> {
    let parts = core.chats().readable(agent_id)?.left_out(message_id);
    Ok(sikemux_view::chat::ChatLeftOut { parts })
}

pub fn detach(core: &Core, client: ClientId, agent_id: &str) {
    if let Some(chat) = core.chats().get(agent_id) {
        chat.feed.unsubscribe(client);
    }
    let resting = core.chats().resting.locked();
    if let Some(resting) = resting.iter().find(|resting| resting.agent_id == agent_id) {
        resting.feed.unsubscribe(client);
    }
}

/// Whether `agent_id` is an agent of the workspace whose chat isn't running, asleep or
/// stopped, so a message to it must start it first on its saved conversation.
pub fn stopped(core: &Core, agent_id: &str) -> bool {
    core.chats().get(agent_id).is_none() && core.workspaces().agent(agent_id).is_some()
}

/// Readies `agent_id`'s chat for the person's message. A sleeping chat shows its saved
/// history and the message at once, its agent starting as the message is sent.
pub async fn wake_for_prompt(
    core: &Arc<Core>,
    agent_id: &str,
    context: &mut [ChatContext],
) -> CoreResult<()> {
    if stopped(core, agent_id) {
        wake_shown(core, agent_id).await?;
    }
    mentions::wake(core, agent_id, context).await
}

/// Starts a sleeping chat waiting for the person, with its saved history in place.
pub async fn wake_shown(core: &Arc<Core>, agent_id: &str) -> CoreResult<Arc<Chat>> {
    (core.chat_above().ensure_running)(core, agent_id, true).await?;
    let chat = core.chats().running(agent_id)?;
    chat.until_saved_read().await;
    Ok(chat)
}

/// Readies `agent_id`'s chat for a message from any client: starts it when it isn't
/// running, and opens again the earlier chats `context` names.
pub async fn wake_for_message(
    core: &Arc<Core>,
    agent_id: &str,
    context: &mut [ChatContext],
) -> CoreResult<()> {
    if stopped(core, agent_id) {
        (core.chat_above().wake_chat)(core, agent_id.to_owned()).await?;
    }
    mentions::wake(core, agent_id, context).await
}

/// Starts a turn of `agent_id`'s chat with `text`, whose the record says `whose` are.
pub fn prompt(
    core: &Core,
    agent_id: &str,
    message_id: Option<String>,
    text: String,
    paths: Vec<String>,
    mut context: Vec<ChatContext>,
    whose: super::Whose,
) -> CoreResult<()> {
    // A sent message is edited later by its id, so one the client did not name gets one here.
    let message_id = match message_id {
        Some(message_id) => {
            bounded_text("message id", &message_id, 256)?;
            message_id
        }
        None => uuid::Uuid::now_v7().to_string(),
    };
    mentions::fill(core, agent_id, &mut context);
    super::side::tell(core, agent_id, &mut context);
    (core.chat_above().messages_worked)(core, agent_id);
    (core.chat_above().messages_person_wrote)(core, agent_id);
    if whose.person {
        let words = context.first().map(|context| context.title.as_str());
        let title = feed::title(if text.trim().is_empty() {
            words.unwrap_or_default()
        } else {
            &text
        });
        crate::server::workspace::retitle(core, agent_id, title, true);
    }
    let chat = core.chats().running(agent_id)?;
    context.extend((core.chat_above().board_take_note)(core, agent_id));
    context.extend((core.chat_above().memory_take_removed)(core, agent_id));
    context.extend(
        (core.chat_above().turn_state_now)(core, agent_id)
            .and_then(|turn| (core.chat_above().turn_told)(&turn)),
    );
    // The agent reads it only once it has started, which takes seconds, so the person
    // sees it sent now.
    let shown = !chat.is_ready() && chat.history_in_place();
    if shown {
        chat.record_prompt(Some(&message_id), &text, &paths, &context, whose);
    }
    chat.send(ChatCommand::Prompt {
        message_id: Some(message_id),
        text,
        paths,
        context,
        shown,
        whose,
    })
}

pub async fn edit(
    core: &Core,
    agent_id: &str,
    message_id: String,
    text: String,
    paths: Vec<String>,
    mut context: Vec<ChatContext>,
) -> CoreResult<()> {
    bounded_text("message id", &message_id, 256)?;
    mentions::fill(core, agent_id, &mut context);
    let (reply, answer) = oneshot::channel();
    core.chats().running(agent_id)?.send(ChatCommand::Edit {
        message_id,
        text,
        paths,
        context,
        reply,
    })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

/// Puts the person's `text` into the running turn, whose the record says `whose` are.
pub async fn steer(
    core: &Core,
    agent_id: &str,
    text: String,
    paths: Vec<String>,
    mut context: Vec<ChatContext>,
    whose: super::Whose,
) -> CoreResult<native::Steered> {
    mentions::fill(core, agent_id, &mut context);
    (core.chat_above().messages_person_wrote)(core, agent_id);
    let (reply, answer) = oneshot::channel();
    core.chats().running(agent_id)?.send(ChatCommand::Steer {
        text,
        paths,
        context,
        whose,
        reply,
    })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

/// Hands the chat a message; `person` when it carries the person's own words, such as their
/// answer to its question.
pub async fn letter(
    core: &Core,
    agent_id: &str,
    text: String,
    paths: Vec<String>,
    person: bool,
) -> CoreResult<Delivery> {
    let (reply, answer) = oneshot::channel();
    core.chats().running(agent_id)?.send(ChatCommand::Letter {
        text,
        paths,
        person,
        reply,
    })?;
    answer.await.map_err(|_| CoreError::from(STOPPED))
}

/// Moves the agent's chat to `account`, a turn at work carrying on there, and answers once the
/// chat says whether it is going. The agent's record names the account at once, so every client
/// shows the chat on its way there, and names the one before again when the chat stays. An agent
/// with no chat running starts on it next time.
pub async fn move_chat(core: &Core, agent_id: &str, account: ChatAccount) -> CoreResult<()> {
    validate_account(&account)?;
    let before = core.workspaces().agent_or_gone(agent_id)?.profile_id;
    let to = account.id.clone();
    core.workspaces()
        .edit_agent(agent_id, |agent| agent.profile_id = Some(to))?;
    let going = send_switch(core, agent_id, account).await;
    if going.is_err() {
        let _ = core
            .workspaces()
            .edit_agent(agent_id, |agent| agent.profile_id = before);
    }
    going
}

const MOVE_ANSWERED_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);

async fn send_switch(core: &Core, agent_id: &str, account: ChatAccount) -> CoreResult<()> {
    let Some(chat) = core.chats().get(agent_id) else {
        return Ok(());
    };
    if chat.unstarted() {
        // A chat waiting for the person starts on the account chosen, as on a model chosen.
        chat.set_current(rebind::on_account(
            chat.provider(),
            &chat.current_launch(),
            &account,
        ));
        return Ok(());
    }
    let (moved, answered) = tokio::sync::oneshot::channel();
    chat.send(ChatCommand::SwitchAccount {
        account,
        moved: Some(moved),
    })?;
    match tokio::time::timeout(MOVE_ANSWERED_WITHIN, answered).await {
        Ok(Ok(going)) => Ok(going?),
        Ok(Err(_)) => Err("The chat ended before it could move".into()),
        Err(_) => Err("The chat did not answer in time, so it stayed on its account".into()),
    }
}

/// The account the agent's chat runs on, and those it may carry on in, as one rule of the core's
/// decides for every client.
pub fn accounts(core: &Core, agent_id: &str) -> CoreResult<Takers> {
    let agent = core.workspaces().agent_or_gone(agent_id)?;
    let book = core.workspaces().accounts().unwrap_or_default();
    if agent.pty_id.is_some() || agent.terminal {
        let takers = book.takers(&agent.provider, agent.profile_id.as_deref(), true);
        return Ok(Takers {
            others: Vec::new(),
            none_because: Some("A chat in its own terminal stays on its account".into()),
            ..takers
        });
    }
    let started = core
        .chats()
        .get(agent_id)
        .is_none_or(|chat| !chat.unstarted());
    Ok(book.takers(&agent.provider, agent.profile_id.as_deref(), started))
}

pub async fn move_to_account(core: &Core, agent_id: &str, account_id: &str) -> CoreResult<()> {
    let takers = accounts(core, agent_id)?;
    let account = takers
        .others
        .into_iter()
        .find(|other| other.id == account_id)
        .ok_or_else(|| {
            takers
                .none_because
                .unwrap_or_else(|| "That account cannot carry this chat on".into())
        })?;
    move_chat(core, agent_id, account).await
}

/// Stops the running turn. The messages waiting behind it go back to the
/// person rather than out.
pub fn cancel(core: &Core, agent_id: &str) -> CoreResult<()> {
    let chat = core.chats().running(agent_id)?;
    chat.feed.clear_queue();
    super::cap::stop_summarising(&chat);
    chat.send(ChatCommand::Cancel)
}

pub fn stop_task(core: &Core, agent_id: &str, task_id: String) -> CoreResult<()> {
    bounded_text("task id", &task_id, 256)?;
    core.chats()
        .running(agent_id)?
        .send(ChatCommand::StopTask { task_id })
}

/// Passes a request an MCP app made on to its server, through the chat's agent. A chat whose
/// agent is not running is never started for it.
pub async fn mcp_app_request(
    core: &Core,
    agent_id: &str,
    request: crate::server::mcp_apps::ServerRequest,
) -> CoreResult<Value> {
    let chat = core.chats().running(agent_id)?;
    if chat.unstarted.load(Ordering::Acquire) {
        return Err("Send a message in this chat first, so its agent is running".into());
    }
    let (reply, answer) = oneshot::channel();
    chat.send(ChatCommand::McpApp { request, reply })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

pub fn reply_permission(
    core: &Core,
    agent_id: &str,
    request_id: &str,
    option_id: Option<String>,
    peer: &Peer,
) -> CoreResult<()> {
    if let Some(chat) = core.chats().get(agent_id) {
        return chat.reply_permission(request_id, option_id, peer);
    }
    match (core.chat_above().permit_elsewhere)(core, agent_id, request_id, option_id) {
        true => Ok(()),
        false => Err(super::permission_gone()),
    }
}

pub async fn set_permission_mode(
    core: &Core,
    agent_id: &str,
    mode: PermissionMode,
) -> CoreResult<()> {
    let chat = core.chats().running(agent_id)?;
    if chat.unstarted() {
        let mut launch = chat.current_launch();
        launch.permission_mode = mode;
        chat.set_current(launch);
        chat.set_permission_mode(mode);
        return Ok(());
    }
    let (reply, answer) = oneshot::channel();
    chat.send(ChatCommand::SetPermissionMode { mode, reply })?;
    chat.feed.set_changing_mode(true);
    let answered = tokio::time::timeout(REPLY_TIMEOUT, answer).await;
    chat.feed.set_changing_mode(false);
    answered
        .map_err(|_| CoreError::from("Permission update timed out"))?
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

/// Asks `agent_id`'s agent to sign in by `method_id`, one of the methods it
/// offered that it handles itself.
pub async fn authenticate(core: &Core, agent_id: &str, method_id: String) -> CoreResult<()> {
    bounded_text("sign-in method", &method_id, 256)?;
    let (reply, answer) = oneshot::channel();
    core.chats()
        .running(agent_id)?
        .send(ChatCommand::Authenticate { method_id, reply })?;
    answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)
}

pub async fn set_config(
    core: &Core,
    agent_id: &str,
    config_id: String,
    value: String,
) -> CoreResult<Value> {
    bounded_text("config id", &config_id, 256)?;
    bounded_text("config value", &value, 4096)?;
    let Ok(chat) = core.chats().running(agent_id) else {
        return choose_while_asleep(core, agent_id, &config_id, value);
    };
    if chat.unstarted() {
        return choose_before_start(core, &chat, &config_id, value);
    }
    let (reply, answer) = oneshot::channel();
    chat.send(ChatCommand::SetConfig {
        config_id,
        value,
        reply,
    })?;
    let answer = answer
        .await
        .map_err(|_| CoreError::from(STOPPED))?
        .map_err(CoreError::from)?;
    keep_model_choice(core, agent_id, &answer["configOptions"]);
    Ok(answer)
}
