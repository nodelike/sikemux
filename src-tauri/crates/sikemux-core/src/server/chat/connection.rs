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
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::acp::{
    adapter_effort_id, air, native, permission_mode_id, prompt_blocks, turn_signal, SessionEnd,
    TurnSignal,
};
use crate::protocol::{ChatEventKind, ChatStart};

use super::{Chat, ChatCommand};

/// How long a stopped turn may keep running before the agent is killed. The
/// agent only reads a cancel between steps, and a wedged tool never gets there.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

fn servers(chat: &Chat) -> Vec<McpServer> {
    chat.launch
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

fn error_message(message: impl std::fmt::Display) -> Value {
    json!({ "message": message.to_string() })
}

pub(super) async fn run(
    chat: Arc<Chat>,
    mut commands: mpsc::UnboundedReceiver<ChatCommand>,
) -> Result<SessionEnd, String> {
    chat.emit(ChatEventKind::Status, json!({ "state": "starting" }));
    let launch = chat.launch.clone();
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
                let own_session = notification
                    .0
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| event_session.get().is_some_and(|own| own == id));
                let signal = if own_session {
                    notification
                        .0
                        .get("update")
                        .and_then(|update| turn_signal(chat.provider(), update))
                } else {
                    None
                };
                if signal == Some(TurnSignal::Work)
                    && !chat.running.load(Ordering::Acquire)
                    && !chat.unprompted.swap(true, Ordering::AcqRel)
                {
                    chat.emit(ChatEventKind::TurnStarted, json!({}));
                }
                chat.emit(ChatEventKind::SessionUpdate, notification.0);
                if signal == Some(TurnSignal::Closes)
                    && chat.unprompted.swap(false, Ordering::AcqRel)
                {
                    chat.emit(
                        ChatEventKind::TurnCompleted,
                        json!({ "stopReason": "end_turn" }),
                    );
                }
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
                if !chat.hold_permission(request_id, option_ids, responder, payload.clone()) {
                    return Ok(());
                }
                chat.emit(ChatEventKind::PermissionRequest, payload);
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

                let tool_servers = servers(&chat);
                let (session_id, mut setup) = if let Some(existing) = launch.resume_id.clone() {
                    if !initialize.agent_capabilities.load_session {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data("This agent cannot load existing sessions"));
                    }
                    let response = connection
                        .send_request(native::LoadSession(
                            LoadSessionRequest::new(existing.clone(), &launch.cwd)
                                .mcp_servers(tool_servers),
                        ))
                        .block_task()
                        .await?;
                    (existing, response.0)
                } else {
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
                    (session_id, response.0)
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

                if native::arguments(&provider).is_some() {
                    apply_saved_choices(
                        &connection,
                        &session_id,
                        &mut setup,
                        model_outside_config,
                        launch.model.as_deref(),
                        launch.effort.as_deref(),
                    )
                    .await;
                } else {
                    for (config_id, value) in [
                        ("model", launch.model.as_deref()),
                        (adapter_effort_id(&provider), launch.effort.as_deref()),
                    ] {
                        if let Some(value) = value {
                            let response = connection
                                .send_request(SetSessionConfigOptionRequest::new(
                                    session_id.clone(),
                                    config_id,
                                    value,
                                ))
                                .block_task()
                                .await?;
                            setup["configOptions"] =
                                serde_json::to_value(response.config_options)?;
                        }
                    }
                }

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
                let end = loop {
                    let command = tokio::select! {
                        command = commands.recv() => match command {
                            Some(command) => command,
                            None => break SessionEnd::Requested,
                        },
                        () = connection.incoming_closed() => break SessionEnd::Exited,
                        Some(()) = broken_rx.recv() => {
                            chat.emit(
                                ChatEventKind::Error,
                                error_message("The agent failed while stopping, so its session was restarted"),
                            );
                            break SessionEnd::Exited;
                        }
                        Some(stalled) = stalled_rx.recv() => {
                            if chat.running.load(Ordering::Acquire) && turn == stalled {
                                chat.emit(
                                    ChatEventKind::Error,
                                    error_message("The agent did not stop, so its session was restarted"),
                                );
                                break SessionEnd::Exited;
                            }
                            continue;
                        }
                    };
                    match command {
                        ChatCommand::Prompt {
                            from,
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
                            let said = (text.clone(), paths.clone());
                            let blocks = match prompt_blocks(text, paths, context, embedded_context)
                            {
                                Ok(blocks) => {
                                    chat.feed.prompted(from, &said.0, &said.1);
                                    blocks
                                }
                                Err(error) => {
                                    chat.running.store(false, Ordering::Release);
                                    chat.emit(ChatEventKind::Error, error_message(error));
                                    continue;
                                }
                            };
                            turn += 1;
                            chat.unprompted.store(false, Ordering::Release);
                            chat.emit(ChatEventKind::TurnStarted, json!({}));
                            let answering = chat.clone();
                            let response_turn = turn;
                            let response_cancelled = cancelled_turn.clone();
                            let response_broken = broken_tx.clone();
                            let sent = connection
                                .send_request(PromptRequest::new(session_id.clone(), blocks))
                                .on_receiving_result(async move |result| {
                                    answering.running.store(false, Ordering::Release);
                                    answering.unprompted.store(false, Ordering::Release);
                                    answering.cancel_permissions();
                                    match result {
                                        Ok(response) => answering.emit(
                                            ChatEventKind::TurnCompleted,
                                            serde_json::to_value(response)
                                                .unwrap_or_else(|_| json!({})),
                                        ),
                                        // Hermes can crash out of a stopped turn and
                                        // leave its session refusing every prompt after.
                                        Err(_)
                                            if response_cancelled.load(Ordering::Acquire)
                                                == response_turn =>
                                        {
                                            let _ = response_broken.send(());
                                        }
                                        Err(error) => answering
                                            .emit(ChatEventKind::Error, error_message(error)),
                                    }
                                    Ok(())
                                });
                            if let Err(error) = sent {
                                chat.running.store(false, Ordering::Release);
                                chat.emit(ChatEventKind::Error, error_message(error));
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
                                        chat.feed.prompted(from, &said.0, &said.1);
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
                            } else if chat.unprompted.swap(false, Ordering::AcqRel) {
                                // No prompt of ours is open to answer with the
                                // end of a turn the agent started itself.
                                chat.emit(
                                    ChatEventKind::TurnCompleted,
                                    json!({ "stopReason": "cancelled" }),
                                );
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
