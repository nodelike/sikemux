//! One chat agent's process and its ACP session, from `initialize` until the
//! session ends.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, Implementation, InitializeRequest, LoadSessionRequest, McpServer,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SetSessionConfigOptionRequest,
    SetSessionModeRequest,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, ErrorCode};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::acp::account::{self, Failure, FailureKind};
use crate::acp::{air, native, permission_mode_id, prompt_blocks, SessionEnd};
use crate::protocol::{ChatEventKind, ChatLaunch, ChatStart};

use super::rebind::{
    error_message, next_account, on_account, switch_notice, Carried, Outcome, Rebind, Replay,
};
use super::{Chat, ChatCommand};

/// How long a stopped turn may keep running before the agent is killed. The
/// agent only reads a cancel between steps, and a wedged tool never gets there.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

/// A turn that failed in a way a new agent process or another account may fix.
struct Failed {
    turn: u64,
    failure: Failure,
    response: Option<Value>,
}

fn servers(chat: &Chat, launch: &ChatLaunch) -> Vec<McpServer> {
    launch
        .mcp_servers
        .iter()
        .filter_map(|server| match serde_json::from_value(server.clone()) {
            Ok(server) => Some(server),
            Err(error) => {
                eprintln!(
                    "sikemux core: chat {} skips a tool server it cannot read: {error}",
                    chat.agent_id()
                );
                None
            }
        })
        .collect()
}

/// Carries the chat's saved model and effort into a native agent's session.
/// A choice the agent no longer offers is skipped, since model lists change
/// between launches and a stale one should not stop the chat from starting.
async fn apply_saved_choices(
    connection: &ConnectionTo<Agent>,
    session_id: &str,
    setup: &mut Value,
    model_outside_config: bool,
    model: Option<&str>,
    effort: Option<&str>,
) {
    if let Some(model) = model.filter(|model| native::offers(setup, "model", model)) {
        let applied = if model_outside_config {
            connection
                .send_request(native::SetSessionModel {
                    session_id: session_id.to_owned(),
                    model_id: model.to_owned(),
                })
                .block_task()
                .await
                .map(|_| native::select_model(setup, model))
        } else {
            connection
                .send_request(SetSessionConfigOptionRequest::new(
                    session_id.to_owned(),
                    "model",
                    model,
                ))
                .block_task()
                .await
                .map(|response| {
                    if let Ok(options) = serde_json::to_value(response.config_options) {
                        setup["configOptions"] = options;
                    }
                })
        };
        if let Err(error) = applied {
            eprintln!("The agent did not take the saved model {model}: {error}");
        }
    }
    let Some(effort) = effort else {
        return;
    };
    let Some(config_id) = native::effort_config_id(setup)
        .filter(|id| native::offers(setup, id, effort))
        .map(str::to_owned)
    else {
        return;
    };
    match connection
        .send_request(SetSessionConfigOptionRequest::new(
            session_id.to_owned(),
            config_id,
            effort,
        ))
        .block_task()
        .await
    {
        Ok(response) => {
            if let Ok(options) = serde_json::to_value(response.config_options) {
                setup["configOptions"] = options;
            }
        }
        Err(error) => eprintln!("The agent did not take the saved effort {effort}: {error}"),
    }
}

/// What sending a prompt needs from the loop that owns the session.
struct Turns {
    session_id: String,
    cancelled: Arc<AtomicU64>,
    broken: mpsc::UnboundedSender<()>,
    failed: mpsc::UnboundedSender<Failed>,
    account: Option<String>,
}

impl Turns {
    /// Sends one prompt as turn `turn`. Its answer arrives off the loop: a
    /// failure that another process or account may fix goes back to the loop
    /// to decide on, with the turn still running.
    fn prompt(
        &self,
        connection: &ConnectionTo<Agent>,
        chat: &Arc<Chat>,
        turn: u64,
        blocks: Vec<agent_client_protocol::schema::v1::ContentBlock>,
    ) {
        chat.unprompted.store(false, Ordering::Release);
        chat.emit(ChatEventKind::TurnStarted, json!({}));
        let answering = chat.clone();
        let cancelled = self.cancelled.clone();
        let broken = self.broken.clone();
        let failed = self.failed.clone();
        let account = self.account.clone();
        let sent = connection
            .send_request(PromptRequest::new(self.session_id.clone(), blocks))
            .on_receiving_result(async move |result| {
                answering.unprompted.store(false, Ordering::Release);
                answering.cancel_permissions();
                let settle = || answering.running.store(false, Ordering::Release);
                match result {
                    Ok(response) => {
                        let payload = serde_json::to_value(response).unwrap_or_else(|_| json!({}));
                        match payload.get("_meta").and_then(account::failure) {
                            Some(failure) if failure.kind != FailureKind::Other => {
                                let _ = failed.send(Failed {
                                    turn,
                                    failure,
                                    response: Some(payload),
                                });
                            }
                            Some(failure) => {
                                settle();
                                answering.emit(ChatEventKind::TurnCompleted, payload);
                                answering.emit(
                                    ChatEventKind::Error,
                                    failure.payload(account.as_deref()),
                                );
                            }
                            None => {
                                settle();
                                answering.answered.store(true, Ordering::Release);
                                answering.emit(ChatEventKind::TurnCompleted, payload);
                            }
                        }
                    }
                    // Hermes can crash out of a stopped turn and leave its
                    // session refusing every prompt after.
                    Err(_) if cancelled.load(Ordering::Acquire) == turn => {
                        settle();
                        let _ = broken.send(());
                    }
                    Err(error) if error.code == ErrorCode::AuthRequired => {
                        let _ = failed.send(Failed {
                            turn,
                            failure: Failure::sign_in(error.message.clone()),
                            response: None,
                        });
                    }
                    Err(error) => {
                        settle();
                        answering.emit(ChatEventKind::Error, error_message(error));
                    }
                }
                Ok(())
            });
        if let Err(error) = sent {
            chat.running.store(false, Ordering::Release);
            chat.emit(ChatEventKind::Error, error_message(error));
        }
    }
}

pub(super) async fn run(
    chat: Arc<Chat>,
    launch: ChatLaunch,
    commands: &mut mpsc::UnboundedReceiver<ChatCommand>,
    rebind: Option<Box<Rebind>>,
) -> Result<Outcome, String> {
    chat.emit(ChatEventKind::Status, json!({ "state": "starting" }));
    let signed_in_at_start = account::signed_in(&launch.provider, &launch.env);
    let Carried {
        replay,
        mut exhausted,
        mut signed_in_again,
        notice,
        quiet_load,
    } = Carried::from(rebind);
    let config = AcpAgentConfig::new(&launch.program)
        .args(launch.args.iter().cloned())
        .envs(sikemux_pty::user_shell::login_shell_locale())
        .envs(launch.env.clone());
    let agent = AcpAgent::new(config);
    let provider = launch.provider.clone();

    // Set once the session has loaded. A resumed session replays its history
    // before that, and none of it is a turn.
    let loaded_session = Arc::new(OnceLock::<String>::new());
    let event_chat = chat.clone();
    let event_session = loaded_session.clone();
    let permission_chat = chat.clone();

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: air::SessionUpdate, _connection| {
                let chat = &event_chat;
                if quiet_load && event_session.get().is_none() {
                    return Ok(());
                }
                chat.emit(ChatEventKind::SessionUpdate, notification.0);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let chat = &permission_chat;
                if chat.approves() {
                    if let Some(option) = native::approval(&request.options) {
                        return responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                option.option_id.clone(),
                            )),
                        ));
                    }
                }
                let request_id = Uuid::new_v4().to_string();
                let option_ids = request
                    .options
                    .iter()
                    .map(|option| option.option_id.to_string())
                    .collect();
                let mut payload = serde_json::to_value(&request).unwrap_or_else(|_| json!({}));
                if let Some(object) = payload.as_object_mut() {
                    object.insert("requestId".into(), Value::String(request_id.clone()));
                }
                let (answer, answered) = oneshot::channel();
                let held = chat.hold_permission(request_id, option_ids, answer, payload.clone());
                tokio::spawn(async move {
                    let outcome = match answered.await {
                        Ok(Some(option_id)) => RequestPermissionOutcome::Selected(
                            SelectedPermissionOutcome::new(option_id),
                        ),
                        _ => RequestPermissionOutcome::Cancelled,
                    };
                    let _ = responder.respond(RequestPermissionResponse::new(outcome));
                });
                if held {
                    chat.emit(ChatEventKind::PermissionRequest, payload);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| {
            let chat = chat.clone();
            let loaded_session = loaded_session.clone();
            let launch = launch.clone();
            let provider = provider.clone();
            async move {
                chat.emit(ChatEventKind::Status, json!({ "state": "initializing" }));
                let initialize = connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(air::client_capabilities())
                            .client_info(Implementation::new("sikemux", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await?;
                let mut capabilities = serde_json::to_value(&initialize.agent_capabilities)?;
                let initialize_meta = serde_json::to_value(&initialize.meta)?;
                let steering = air::steering_supported(&initialize_meta);
                let embedded_context = initialize
                    .agent_capabilities
                    .prompt_capabilities
                    .embedded_context;
                capabilities["steering"] = json!(steering);

                let tool_servers = servers(&chat, &launch);
                let can_load = initialize.agent_capabilities.load_session;
                let open_new = async |tool_servers: Vec<McpServer>| {
                    let response = connection
                        .send_request(native::NewSession(
                            NewSessionRequest::new(&launch.cwd).mcp_servers(tool_servers),
                        ))
                        .block_task()
                        .await?;
                    let session_id = response
                        .0
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            agent_client_protocol::Error::invalid_params()
                                .data("The agent opened a session without an id")
                        })?
                        .to_owned();
                    Ok::<_, agent_client_protocol::Error>((session_id, response.0))
                };
                let (session_id, mut setup) = if let Some(existing) = launch.resume_id.clone() {
                    if !initialize.agent_capabilities.load_session {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data("This agent cannot load existing sessions"));
                    }
                    let loaded = connection
                        .send_request(native::LoadSession(
                            LoadSessionRequest::new(existing.clone(), &launch.cwd)
                                .mcp_servers(tool_servers.clone()),
                        ))
                        .block_task()
                        .await;
                    match loaded {
                        Ok(response) => (existing, response.0),
                        // A chat whose turns all failed may have nothing saved
                        // to load, and starting again is all there is to do.
                        Err(_) if quiet_load && !chat.has_answered() => {
                            open_new(tool_servers).await?
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    open_new(tool_servers).await?
                };
                let _ = loaded_session.set(session_id.clone());

                let model_outside_config = native::models_outside_config(&setup);
                setup = native::with_model_config(setup);

                let mode_id = permission_mode_id(&provider, &launch.permission_mode, &setup)
                    .map_err(|error| agent_client_protocol::Error::invalid_params().data(error))?;
                if let Some(mode_id) = mode_id {
                    connection
                        .send_request(SetSessionModeRequest::new(session_id.clone(), mode_id))
                        .block_task()
                        .await?;
                    if let Some(modes) = setup.get_mut("modes").and_then(Value::as_object_mut) {
                        modes.insert("currentModeId".into(), json!(mode_id));
                    }
                }

                apply_saved_choices(
                    &connection,
                    &session_id,
                    &mut setup,
                    model_outside_config,
                    launch.model.as_deref(),
                    launch.effort.as_deref(),
                )
                .await;

                let start = ChatStart {
                    session_id: session_id.clone(),
                    capabilities,
                    setup: setup.clone(),
                };
                chat.mark_ready(start.clone());
                chat.emit(
                    ChatEventKind::Ready,
                    serde_json::to_value(&start).unwrap_or_else(|_| json!({})),
                );

                let mut turn: u64 = 0;
                let (stalled_tx, mut stalled_rx) = mpsc::unbounded_channel::<u64>();
                let cancelled_turn = Arc::new(AtomicU64::new(0));
                let (broken_tx, mut broken_rx) = mpsc::unbounded_channel::<()>();
                let (failed_tx, mut failed_rx) = mpsc::unbounded_channel::<Failed>();
                let turns = Turns {
                    session_id: session_id.clone(),
                    cancelled: cancelled_turn.clone(),
                    broken: broken_tx.clone(),
                    failed: failed_tx,
                    account: launch.account.as_ref().map(|account| account.id.clone()),
                };
                // The prompt the running turn answers, and the one whose turn
                // failed for want of an account, which a switch sends again.
                let mut last_prompt: Option<Replay> = None;
                let mut failed_prompt: Option<Replay> = None;

                if let Some(update) = notice {
                    chat.emit(
                        ChatEventKind::SessionUpdate,
                        json!({ "sessionId": session_id, "update": update }),
                    );
                }
                if let Some(replay) = replay {
                    match prompt_blocks(
                        replay.text.clone(),
                        replay.paths.clone(),
                        replay.context.clone(),
                        embedded_context,
                    ) {
                        Ok(blocks) => {
                            if let Some(from) = replay.announce {
                                chat.feed.prompted(from, replay.message_id.as_deref(), &replay.text, &replay.paths);
                            }
                            chat.running.store(true, Ordering::Release);
                            turn += 1;
                            turns.prompt(&connection, &chat, turn, blocks);
                            last_prompt = Some(Replay {
                                announce: None,
                                ..replay
                            });
                        }
                        Err(error) => {
                            chat.running.store(false, Ordering::Release);
                            chat.emit(ChatEventKind::Error, error_message(error));
                        }
                    }
                }

                let end = loop {
                    let command = tokio::select! {
                        command = commands.recv() => match command {
                            Some(command) => command,
                            None => break Outcome::Ended(SessionEnd::Requested),
                        },
                        () = connection.incoming_closed() => break Outcome::Ended(SessionEnd::Exited),
                        Some(()) = broken_rx.recv() => {
                            chat.emit(
                                ChatEventKind::Error,
                                error_message("The agent failed while stopping, so its session was restarted"),
                            );
                            break Outcome::Ended(SessionEnd::Exited);
                        }
                        Some(failed) = failed_rx.recv() => {
                            if failed.turn != turn {
                                continue;
                            }
                            let current = launch.account.as_ref().map(|account| account.id.clone());
                            let retry = match failed.failure.kind {
                                FailureKind::SignIn if can_load && !signed_in_again => chat
                                    .relaunch(&launch)
                                    .map(|launch| Rebind {
                                        launch,
                                        replay: last_prompt.clone(),
                                        exhausted: exhausted.clone(),
                                        signed_in_again: true,
                                        notice: None,
                                    }),
                                FailureKind::Limit if can_load => {
                                    let mut tried = exhausted.clone();
                                    tried.extend(current.clone());
                                    next_account(&provider, &launch, &tried).and_then(|next| {
                                        let notice = switch_notice(&launch, next, "limit");
                                        let moved = on_account(&provider, &launch, next);
                                        chat.relaunch(&moved).map(|launch| Rebind {
                                            launch,
                                            replay: last_prompt.clone(),
                                            exhausted: tried,
                                            signed_in_again: false,
                                            notice: Some(notice),
                                        })
                                    })
                                }
                                _ => None,
                            };
                            if let Some(rebind) = retry {
                                break Outcome::Rebind(Box::new(rebind));
                            }
                            chat.running.store(false, Ordering::Release);
                            if let Some(response) = failed.response {
                                chat.emit(ChatEventKind::TurnCompleted, response);
                            }
                            chat.emit(ChatEventKind::Error, failed.failure.payload(current.as_deref()));
                            failed_prompt = last_prompt.take();
                            continue;
                        }
                        Some(stalled) = stalled_rx.recv() => {
                            if chat.running.load(Ordering::Acquire) && turn == stalled {
                                chat.emit(
                                    ChatEventKind::Error,
                                    error_message("The agent did not stop, so its session was restarted"),
                                );
                                break Outcome::Ended(SessionEnd::Exited);
                            }
                            continue;
                        }
                    };
                    match command {
                        ChatCommand::Prompt {
                            from,
                            message_id,
                            text,
                            paths,
                            context,
                        } => {
                            if chat.running.swap(true, Ordering::AcqRel) {
                                chat.emit(
                                    ChatEventKind::Error,
                                    error_message("wait for the current turn to finish"),
                                );
                                continue;
                            }
                            exhausted.clear();
                            signed_in_again = false;
                            failed_prompt = None;
                            let replay = Replay {
                                announce: Some(from),
                                message_id,
                                text,
                                paths,
                                context,
                            };
                            // A sign-in made elsewhere since the agent started
                            // is only read by a new agent process.
                            let changed = account::signed_in(&provider, &launch.env);
                            if can_load
                                && changed.is_some()
                                && signed_in_at_start.is_some()
                                && changed != signed_in_at_start
                            {
                                if let Some(launch) = chat.relaunch(&launch) {
                                    break Outcome::Rebind(Box::new(Rebind {
                                        launch,
                                        replay: Some(replay),
                                        exhausted: Vec::new(),
                                        signed_in_again: false,
                                        notice: None,
                                    }));
                                }
                            }
                            let blocks = match prompt_blocks(
                                replay.text.clone(),
                                replay.paths.clone(),
                                replay.context.clone(),
                                embedded_context,
                            ) {
                                Ok(blocks) => {
                                    chat.feed.prompted(from, replay.message_id.as_deref(), &replay.text, &replay.paths);
                                    blocks
                                }
                                Err(error) => {
                                    chat.running.store(false, Ordering::Release);
                                    chat.emit(ChatEventKind::Error, error_message(error));
                                    continue;
                                }
                            };
                            turn += 1;
                            turns.prompt(&connection, &chat, turn, blocks);
                            last_prompt = Some(Replay {
                                announce: None,
                                ..replay
                            });
                        }
                        ChatCommand::SwitchAccount { account } => {
                            if chat.running.load(Ordering::Acquire) {
                                chat.emit(
                                    ChatEventKind::Error,
                                    error_message("Stop the current turn before switching accounts"),
                                );
                                continue;
                            }
                            if !can_load {
                                chat.emit(
                                    ChatEventKind::Error,
                                    error_message("This agent cannot carry a chat to another account"),
                                );
                                continue;
                            }
                            let moves = launch
                                .account
                                .as_ref()
                                .is_none_or(|current| current.id != account.id);
                            let notice = moves.then(|| switch_notice(&launch, &account, "chosen"));
                            let moved = on_account(&provider, &launch, &account);
                            if let Some(launch) = chat.relaunch(&moved) {
                                let replay = failed_prompt.take();
                                if replay.is_some() {
                                    chat.running.store(true, Ordering::Release);
                                }
                                break Outcome::Rebind(Box::new(Rebind {
                                    launch,
                                    replay,
                                    exhausted: Vec::new(),
                                    signed_in_again: false,
                                    notice,
                                }));
                            }
                        }
                        ChatCommand::SetPermissionMode { mode, reply } => {
                            let result = if chat.running.load(Ordering::Acquire) {
                                Err("Stop the current turn before changing permissions".into())
                            } else {
                                match permission_mode_id(&provider, &mode, &setup) {
                                    Ok(Some(mode_id)) => connection
                                        .send_request(SetSessionModeRequest::new(
                                            session_id.clone(),
                                            mode_id,
                                        ))
                                        .block_task()
                                        .await
                                        .map(|_| ())
                                        .map_err(|error| error.to_string()),
                                    Ok(None) => Ok(()),
                                    Err(error) => Err(error),
                                }
                            };
                            if result.is_ok() {
                                chat.set_permission_mode(&mode);
                            }
                            let _ = reply.send(result);
                        }
                        ChatCommand::SetConfig {
                            config_id,
                            value,
                            reply,
                        } => {
                            let result = if chat.running.load(Ordering::Acquire) {
                                Err("Stop the current turn before changing the model".into())
                            } else if config_id == "model" && model_outside_config {
                                connection
                                    .send_request(native::SetSessionModel {
                                        session_id: session_id.clone(),
                                        model_id: value.clone(),
                                    })
                                    .block_task()
                                    .await
                                    .map_err(|error| error.to_string())
                                    .map(|_| {
                                        native::select_model(&mut setup, &value);
                                        json!({ "configOptions": setup["configOptions"] })
                                    })
                            } else {
                                connection
                                    .send_request(SetSessionConfigOptionRequest::new(
                                        session_id.clone(),
                                        config_id,
                                        value.as_str(),
                                    ))
                                    .block_task()
                                    .await
                                    .map_err(|error| error.to_string())
                                    .and_then(|response| {
                                        serde_json::to_value(response)
                                            .map_err(|error| error.to_string())
                                    })
                                    .inspect(|response| {
                                        setup["configOptions"] = response["configOptions"].clone();
                                    })
                            };
                            if result.is_ok() {
                                chat.feed.set_setup(&setup);
                            }
                            let _ = reply.send(result);
                        }
                        ChatCommand::Steer {
                            from,
                            text,
                            paths,
                            context,
                            reply,
                        } => {
                            let result = if !steering {
                                Err("this agent cannot take a message mid-turn".to_string())
                            } else if !chat.running.load(Ordering::Acquire) {
                                Ok("promptRequired".to_string())
                            } else {
                                let said = (text.clone(), paths.clone());
                                match prompt_blocks(text, paths, context, embedded_context) {
                                    // Answered off the loop, so a stop sent right
                                    // after a steer is never queued behind it.
                                    Ok(blocks) => {
                                        chat.feed.prompted(from, None, &said.0, &said.1);
                                        let _ = connection
                                            .send_request(air::Steer::new(
                                                session_id.clone(),
                                                blocks,
                                            ))
                                            .on_receiving_result(async move |result| {
                                                let _ = reply.send(
                                                    result
                                                        .map(|response| response.outcome)
                                                        .map_err(|error| error.to_string()),
                                                );
                                                Ok(())
                                            });
                                        continue;
                                    }
                                    Err(error) => Err(error),
                                }
                            };
                            let _ = reply.send(result);
                        }
                        ChatCommand::StopTask { task_id } => {
                            let stopping = chat.clone();
                            // A background task outlives the turn that spawned
                            // it, so stopping one must not wait on the turn.
                            let sent = connection
                                .send_request(air::StopAsyncTask {
                                    session_id: session_id.clone(),
                                    async_task_id: task_id,
                                })
                                .on_receiving_result(async move |result| {
                                    if let Err(error) = result {
                                        stopping.emit(ChatEventKind::Error, error_message(error));
                                    }
                                    Ok(())
                                });
                            if let Err(error) = sent {
                                chat.emit(ChatEventKind::Error, error_message(error));
                            }
                        }
                        ChatCommand::Edit { reply, .. } => {
                            let _ = reply.send(Err("This agent cannot edit a sent message".into()));
                        }
                        ChatCommand::Cancel => {
                            connection
                                .send_notification(CancelNotification::new(session_id.clone()))?;
                            if chat.running.load(Ordering::Acquire) {
                                cancelled_turn.store(turn, Ordering::Release);
                                let cancelled = turn;
                                let stalled = stalled_tx.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(CANCEL_GRACE).await;
                                    let _ = stalled.send(cancelled);
                                });
                            }
                        }
                    }
                };
                Ok(end)
            }
        })
        .await
        .map_err(|error| error.to_string())
}
