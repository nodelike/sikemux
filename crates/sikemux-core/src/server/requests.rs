//! What the core does with each request a client sends, and whether it may: one rule, run once,
//! before anything runs. The connection itself is in `connection.rs`.

use crate::server::activity::ActivityCore;
use crate::server::changed::ChangedCore;
use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::ServerCore;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use sikemux_pty::validate_pty_dimensions;
use tokio::sync::mpsc;

use crate::server::changed::What;
use sikemux_wire::protocol::{ask, encode_lines, Request, RequestId, SessionId};

use super::access::{self, Peer, Rule};
use super::calls;
use super::chat::calls as chat_calls;
use super::connection::{blocking, ClientConn, SHUTDOWN_FLUSH};
use super::launching::spawn;
use super::{
    account, agent, attachments, chat, host, messages, remote, session, upgrade, window, workspace,
    Core, CoreError, CoreResult,
};

/// Refuses what a client may not send now: nothing while the core hands itself over, then what
/// `rule` says of its peer.
pub(super) fn admit(core: &Core, client: &ClientConn, rule: &Rule) -> CoreResult<()> {
    if core.updating().is_frozen() {
        return Err(upgrade::UPDATING.into());
    }
    core.permit(&client.peer, rule.needs)?;
    if client.peer == Peer::Local && rule.only_the_app(core.remote().setup()) {
        return Err(access::APP_ONLY.into());
    }
    Ok(())
}

/// Writes to a session's program in the order writes arrived, and tells
/// its agent's status a line was submitted.
pub(super) fn write_to_program(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    request_id: RequestId,
    target: &session::Session,
    bytes: Vec<u8>,
) {
    if !target.may_type(session::typist(client)) {
        client.respond(request_id, Err(session::SETUP_LOCKED.into()));
        return;
    }
    agent::note_input(core, target, &bytes);
    if let Some(agent) = target
        .agent
        .as_ref()
        .filter(|_| agent::submits_line(&bytes))
    {
        core.harness().questions.typed(agent.agent_id());
        core.ship().person_spoke(agent.agent_id());
        messages::worked(core, agent.agent_id());
    }
    if bytes.is_empty() {
        client.answer::<ask::TerminalInput>(request_id, Ok(()));
        return;
    }
    let reply_to = client.clone();
    session::queue_input(
        target,
        bytes,
        session::typist(client),
        Box::new(move |result| {
            reply_to.answer::<ask::TerminalInput>(request_id, result.map_err(CoreError::from));
        }),
    );
}

fn session_or_missing(core: &Core, id: SessionId) -> CoreResult<Arc<session::Session>> {
    core.session(id)
        .ok_or_else(|| CoreError::from("invalid argument: pty not found"))
}

/// Requests from one client run in order, except the slow ones (spawn, kill,
/// task output, shutdown), which run alongside so they hold nothing up.
pub(super) async fn run(
    core: &Arc<Core>,
    client: &Arc<ClientConn>,
    mut queue: mpsc::UnboundedReceiver<(RequestId, Request)>,
) {
    while let Some((request_id, request)) = queue.recv().await {
        let core = core.clone();
        let client = client.clone();
        if let Err(refused) = admit(&core, &client, &access::rule(&request)) {
            client.respond(request_id, Err(refused));
            continue;
        }
        match request {
            Request::Spawn(ask::Spawn { launch, target }) => {
                tokio::spawn(async move {
                    let started_by = client.peer.device_id();
                    let launch = match started_by {
                        Some(_) => core.launch_identity(),
                        None => launch,
                    };
                    match spawn(core.clone(), launch, target, started_by).await {
                        Ok(pending) => {
                            let id = pending.id();
                            client.answer::<ask::Spawn>(request_id, Ok(id));
                            let stopped = pending
                                .task()
                                .is_some_and(|task| core.harness().session_started(id, task));
                            pending.start(&core);
                            if stopped {
                                if let Some(target) = core.take_session_for_kill(id) {
                                    let killing = core.clone();
                                    let _ = blocking(move || {
                                        session::kill(&killing, &target);
                                        Ok(())
                                    })
                                    .await;
                                }
                            }
                        }
                        Err(error) => client.respond(request_id, Err(error)),
                    }
                });
            }
            Request::Kill(ask::Kill { id }) => {
                tokio::spawn(async move {
                    let result = match core.take_session_for_kill(id) {
                        Some(target) => {
                            let core = core.clone();
                            blocking(move || {
                                session::kill(&core, &target);
                                Ok(())
                            })
                            .await
                        }
                        None => Ok(()),
                    };
                    client.answer::<ask::Kill>(request_id, result);
                });
            }
            Request::Shutdown(ask::Shutdown { stop_all }) => {
                tokio::spawn(shutdown(core, client, request_id, stop_all));
            }
            Request::RegisterWindow => {
                core.window().register(client.clone());
                core.hosts().wake.now();
                super::notch::follow(&core);
                core.usage_wake().notify_one();
                crate::waiting::wake(&core.config.socket);
                core.changed(What::Agents);
                client.answer::<ask::RegisterWindow>(request_id, Ok(()));
            }
            Request::Attach(ask::Attach { id }) => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => {
                        let subscriber = client.clone();
                        blocking(move || session::attach(&target, &subscriber, request_id)).await
                    }
                    Err(error) => Err(error),
                };
                match result {
                    Ok(()) => core.changed(What::View),
                    Err(error) => client.respond(request_id, Err(error)),
                }
            }
            Request::ResetModes(ask::ResetModes { id }) => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => {
                        let resetting = core.clone();
                        blocking(move || session::reset_modes(&resetting, &target)).await
                    }
                    Err(error) => Err(error),
                };
                client.answer::<ask::ResetModes>(request_id, result);
            }
            Request::TerminalInput(ask::TerminalInput { id, input }) => {
                match session_or_missing(&core, id) {
                    Ok(target) => {
                        let bytes = session::encode(&target, &input);
                        write_to_program(&core, &client, request_id, &target, bytes);
                    }
                    Err(error) => client.respond(request_id, Err(error)),
                }
            }
            Request::TerminalLines(ask::TerminalLines { id, start, count }) => {
                match session_or_missing(&core, id) {
                    Ok(target) => {
                        tokio::spawn(async move {
                            let lines =
                                blocking(move || Ok(session::lines(&target, start, count))).await;
                            match lines {
                                Ok(lines) => {
                                    client.send(encode_lines(request_id, &lines).into());
                                }
                                Err(error) => client.respond(request_id, Err(error)),
                            }
                        });
                    }
                    Err(error) => client.respond(request_id, Err(error)),
                }
            }
            Request::TerminalText(ask::TerminalText { id, range }) => {
                let result = match session_or_missing(&core, id) {
                    Ok(target) => blocking(move || Ok(session::text(&target, range))).await,
                    Err(error) => Err(error),
                };
                client.answer::<ask::TerminalText>(request_id, result);
            }
            Request::TerminalFind(ask::TerminalFind {
                id,
                query,
                options,
                from,
                backwards,
                inclusive,
            }) => match session_or_missing(&core, id) {
                Ok(target) => {
                    tokio::spawn(async move {
                        let result = blocking(move || {
                            session::find(&target, &query, &options, from, backwards, inclusive)
                        })
                        .await;
                        client.answer::<ask::TerminalFind>(request_id, result);
                    });
                }
                Err(error) => client.respond(request_id, Err(error)),
            },
            Request::TerminalClear(ask::TerminalClear { id }) => {
                let result =
                    session_or_missing(&core, id).map(|target| session::clear(&core, &target));
                client.answer::<ask::TerminalClear>(request_id, result);
            }
            Request::Detach(ask::Detach { stream }) => {
                if let Some(target) = client
                    .stream_session(stream)
                    .and_then(|id| core.session(id))
                {
                    let _ = blocking(move || {
                        session::detach(&target, stream);
                        Ok(())
                    })
                    .await;
                }
                client.forget_stream(stream);
                core.changed(What::View);
                client.answer::<ask::Detach>(request_id, Ok(()));
            }
            Request::Resize(ask::Resize { id, cols, rows }) => {
                if cols < sikemux_grid::MIN_TERMINAL_COLUMNS {
                    eprintln!("sikemux core: a client asked for {cols} columns in terminal {id}");
                } else {
                    core.spares().note_size(cols, rows);
                }
                let result = match validate_pty_dimensions(cols, rows) {
                    Ok(()) => match session_or_missing(&core, id) {
                        Ok(target) => blocking(move || session::resize(&target, cols, rows)).await,
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error.into()),
                };
                if result.is_ok() {
                    core.changed(What::View);
                }
                client.answer::<ask::Resize>(request_id, result);
            }
            Request::List => {
                client.answer::<ask::List>(request_id, Ok(core.session_infos()));
            }
            Request::AcpStart(ask::AcpStart { launch }) => {
                match chat::begin(&core, *launch, Some(&client), None, false, false) {
                    Ok(started) => {
                        tokio::spawn(async move {
                            let result = chat::until_started(&core, started).await;
                            client.answer::<ask::AcpStart>(request_id, result);
                        });
                    }
                    Err(error) => client.respond(request_id, Err(error)),
                }
            }
            Request::AcpAttach(ask::AcpAttach { agent_id, since }) => {
                tokio::spawn(async move {
                    chat::attach(&core, &client, request_id, &agent_id, since).await;
                });
            }
            Request::AcpHistory(ask::AcpHistory {
                agent_id,
                feed,
                before,
                turns,
            }) => {
                tokio::spawn(async move {
                    let reading = core.clone();
                    let page = blocking(move || {
                        let page = chat::history(&reading, &agent_id, &feed, before, turns)?;
                        Ok(serde_json::to_value(page)?)
                    })
                    .await;
                    client.answer::<ask::AcpHistory>(request_id, page);
                });
            }
            Request::AcpDetach(ask::AcpDetach { agent_id }) => {
                chat::detach(&core, client.id, &agent_id);
                client.answer::<ask::AcpDetach>(request_id, Ok(()));
            }
            Request::AcpWake(ask::AcpWake { agent_id }) => {
                tokio::spawn(async move {
                    let result = super::launching::wake_chat(&core, agent_id).await;
                    client.answer::<ask::AcpWake>(request_id, result);
                });
            }
            Request::AcpList => {
                let chats = if client.peer.is_local() {
                    core.chats().list()
                } else {
                    core.workspaces().listed(core.chats().list())
                };
                client.answer::<ask::AcpList>(request_id, Ok(chats));
            }
            Request::AcpPrompt(ask::AcpPrompt {
                agent_id,
                message_id,
                text,
                paths,
                context,
            }) => {
                core.ship().person_spoke(&agent_id);
                let params = chat_calls::PromptChatParams {
                    agent_id,
                    message_id,
                    text,
                    paths,
                    context,
                };
                if chat_calls::must_wake(&core, &params) {
                    tokio::spawn(async move {
                        let result = chat_calls::prompt_chat(&core, params).await;
                        client.answer::<ask::AcpPrompt>(request_id, result);
                    });
                } else {
                    let result = chat_calls::prompt_awake(&core, params);
                    client.answer::<ask::AcpPrompt>(request_id, result);
                }
            }
            Request::AcpEdit(ask::AcpEdit {
                agent_id,
                message_id,
                text,
                paths,
                context,
            }) => {
                tokio::spawn(async move {
                    let result =
                        chat_calls::edit_with(&core, agent_id, message_id, text, paths, context)
                            .await;
                    client.answer::<ask::AcpEdit>(request_id, result);
                });
            }
            Request::AcpSteer(ask::AcpSteer {
                agent_id,
                text,
                paths,
                context,
            }) => {
                if core.workspaces().in_terminal(&agent_id) {
                    let result = chat_calls::steer_terminal(&core, &agent_id, text);
                    client.answer::<ask::AcpSteer>(request_id, result);
                } else {
                    tokio::spawn(async move {
                        let result =
                            chat_calls::steer_chat(&core, &agent_id, text, paths, context).await;
                        client.answer::<ask::AcpSteer>(request_id, result);
                    });
                }
            }
            Request::AttachFile(ask::AttachFile {
                agent_id,
                name,
                mime,
                data,
            }) => {
                let dir = core
                    .config
                    .attachment_dir
                    .clone()
                    .ok_or_else(|| CoreError::from("this host has nowhere to keep files"));
                let open = core.chats().running(&agent_id).map(drop).or_else(|error| {
                    core.open_agents()
                        .contains(&agent_id)
                        .then_some(())
                        .ok_or(error)
                });
                match open.and(dir) {
                    Ok(dir) => {
                        tokio::spawn(async move {
                            let result =
                                blocking(move || attachments::save(&dir, &name, &mime, &data))
                                    .await;
                            client.answer::<ask::AttachFile>(request_id, result);
                        });
                    }
                    Err(error) => client.respond(request_id, Err(error)),
                }
            }
            Request::Call(ask::Call { method, params }) => {
                core.activity_log().call_used(
                    calls::row(&method).counted,
                    &method,
                    &params,
                    &client.peer,
                );
                let device = client.peer.device_id();
                tokio::spawn(super::layout::asked_by(device, async move {
                    let result = calls::call(core, &client, &method, params).await;
                    client.answer::<ask::Call>(request_id, result);
                }));
            }
            Request::AcpCancel(ask::AcpCancel { agent_id }) => {
                let result = chat::cancel(&core, &agent_id);
                client.answer::<ask::AcpCancel>(request_id, result);
            }
            Request::AcpStopTask(ask::AcpStopTask { agent_id, task_id }) => {
                let result = chat::stop_task(&core, &agent_id, task_id);
                client.answer::<ask::AcpStopTask>(request_id, result);
            }
            Request::AcpPermissionReply(ask::AcpPermissionReply {
                agent_id,
                request_id: permission,
                option_id,
            }) => {
                let result =
                    chat::reply_permission(&core, &agent_id, &permission, option_id, &client.peer);
                client.answer::<ask::AcpPermissionReply>(request_id, result);
            }
            Request::AcpStop(ask::AcpStop { agent_id }) => {
                if let Some(chat) = core.chats().get(&agent_id) {
                    core.workspaces().note_stopping(&chat);
                }
                if core.chats().stop(&agent_id) {
                    core.note_agent_gone(&agent_id);
                }
                client.answer::<ask::AcpStop>(request_id, Ok(()));
            }
            Request::AcpSetPermissionMode(ask::AcpSetPermissionMode { agent_id, mode }) => {
                tokio::spawn(async move {
                    let result = chat::set_permission_mode(&core, &agent_id, mode).await;
                    client.answer::<ask::AcpSetPermissionMode>(request_id, result);
                });
            }
            Request::RemoteStatus => {
                let status = Box::new(core.remote().status());
                client.answer::<ask::RemoteStatus>(request_id, Ok(status));
            }
            Request::SetRemoteAccess(ask::SetRemoteAccess { enabled }) => {
                tokio::spawn(async move {
                    let result = remote::set_enabled(&core, enabled).await;
                    client.answer::<ask::SetRemoteAccess>(request_id, result.map(Box::new));
                });
            }
            Request::SetDeviceAccess(ask::SetDeviceAccess { id, access }) => {
                let result = remote::set_access(&core, &id, access);
                client.answer::<ask::SetDeviceAccess>(request_id, result.map(Box::new));
            }
            Request::BackdropImage => {
                let data_url = core.workspaces().backdrop_image();
                client.answer::<ask::BackdropImage>(request_id, Ok(data_url));
            }
            Request::PublishOnScreen(ask::PublishOnScreen { agent_ids, focused }) => {
                super::launching::show_terminals(&core, &agent_ids);
                super::chief::on_screen(&core, &agent_ids, focused);
                core.seen().show(client.id, agent_ids, focused);
                core.changed(What::View);
                client.answer::<ask::PublishOnScreen>(request_id, Ok(()));
            }
            Request::FocusAgent(ask::FocusAgent { agent_id }) => {
                let shown = core.window().focus(agent_id);
                let result = if shown {
                    Ok(())
                } else {
                    Err(window::not_open("show that agent").into())
                };
                client.answer::<ask::FocusAgent>(request_id, result);
            }
            Request::SetNotifications(ask::SetNotifications { key_id, key, prefs }) => {
                let result = match &client.peer {
                    Peer::Device { id } => core.remote().set_notifications(id, key_id, &key, prefs),
                    Peer::App | Peer::Local | Peer::Share { .. } => {
                        Err("only a paired phone can ask for notifications".into())
                    }
                };
                if result.is_ok() {
                    remote::announce(&core);
                }
                client.answer::<ask::SetNotifications>(request_id, result);
            }
            Request::ClearNotifications => {
                let result = match &client.peer {
                    Peer::Device { id } => core.remote().clear_notifications(id),
                    Peer::App | Peer::Local | Peer::Share { .. } => {
                        Err("only a paired phone has notifications to stop".into())
                    }
                };
                if result.is_ok() {
                    remote::announce(&core);
                }
                client.answer::<ask::ClearNotifications>(request_id, result);
            }
            Request::SetForeground(ask::SetForeground { foreground }) => {
                client.foreground.store(foreground, Ordering::Release);
                client.answer::<ask::SetForeground>(request_id, Ok(()));
            }
            Request::WatchView => {
                client.watches_view.store(true, Ordering::Release);
                core.view_watched();
                client.answer::<ask::WatchView>(request_id, Ok(()));
                core.send_device_view(&client);
            }
            Request::Host => {
                tokio::spawn(async move {
                    let access = match &client.peer {
                        Peer::Device { id } => core.remote().standing_of(id).map(|it| it.access),
                        _ => None,
                    };
                    let host = blocking(move || {
                        Ok(serde_json::to_value(sikemux_view::device::HostInfo {
                            access,
                            ..host::info()
                        })?)
                    })
                    .await;
                    client.answer::<ask::Host>(request_id, host);
                });
            }
            Request::StartChat(ask::StartChat {
                launcher,
                project,
                permission_mode,
                model,
                effort,
            }) => {
                let choice = workspace::ChatChoice {
                    launcher,
                    project,
                    permission_mode,
                    model,
                    effort,
                    resume_id: None,
                    cwd: None,
                    title: None,
                    terminal: false,
                };
                super::launching::start_chat(&core, &client, request_id, choice);
            }
            Request::ResumeChat(ask::ResumeChat {
                recent,
                permission_mode,
                model,
                effort,
            }) => {
                let choice = workspace::ResumeChoice {
                    recent,
                    permission_mode,
                    model,
                    effort,
                };
                super::launching::resume_chat(&core, &client, request_id, choice);
            }
            Request::AnswerPairing(ask::AnswerPairing { id, allow, access }) => {
                let result = core
                    .remote()
                    .answer(&id, allow.then_some(access))
                    .map(|()| remote::announce(&core));
                client.answer::<ask::AnswerPairing>(request_id, result.map(Box::new));
            }
            Request::AllowDevices(ask::AllowDevices { devices }) => {
                let result = core
                    .remote()
                    .allow(devices)
                    .map(|()| remote::announce(&core));
                client.answer::<ask::AllowDevices>(request_id, result.map(Box::new));
            }
            Request::Unpair => {
                let Peer::Device { id } = &client.peer else {
                    client.respond(
                        request_id,
                        Err("only a paired device can unpair itself".into()),
                    );
                    continue;
                };
                let id = id.clone();
                tokio::spawn(async move {
                    client.answer::<ask::Unpair>(request_id, Ok(()));
                    client.wait_flushed(SHUTDOWN_FLUSH).await;
                    if let Err(error) = remote::forget_leaving(&core, &id) {
                        eprintln!("sikemux core: could not unpair {id}: {error}");
                    }
                });
            }
            Request::SignRegistration(ask::SignRegistration { nonce, user_id }) => {
                tokio::spawn(async move {
                    let result =
                        account::host_registration(&core, &nonce, &user_id, &client.peer).await;
                    client.answer::<ask::SignRegistration>(request_id, result);
                });
            }
            Request::SetOwner(ask::SetOwner { owner }) => {
                tokio::spawn(async move {
                    let result = remote::set_owner(&core, owner, &client.peer).await;
                    client.answer::<ask::SetOwner>(request_id, result.map(Box::new));
                });
            }
            Request::RevokeDevice(ask::RevokeDevice { id }) => {
                let result = remote::revoke(&core, &id);
                client.answer::<ask::RevokeDevice>(request_id, result.map(Box::new));
            }
            Request::AcpSetConfig(ask::AcpSetConfig {
                agent_id,
                config_id,
                value,
            }) => {
                tokio::spawn(async move {
                    let result = chat::set_config(&core, &agent_id, config_id, value).await;
                    client.answer::<ask::AcpSetConfig>(request_id, result);
                });
            }
            Request::AcpSwitchAccount(ask::AcpSwitchAccount { agent_id, account }) => {
                tokio::spawn(async move {
                    let result = chat::move_chat(&core, &agent_id, account).await;
                    client.answer::<ask::AcpSwitchAccount>(request_id, result);
                });
            }
        }
    }
}

async fn shutdown(core: Arc<Core>, client: Arc<ClientConn>, request_id: RequestId, stop_all: bool) {
    if stop_all {
        core.stop_everything().await;
    } else {
        let running = core.running_sessions() + core.chats().count();
        if running > 0 {
            client.respond(
                request_id,
                Err(format!("the core still has {running} running sessions").into()),
            );
            return;
        }
    }
    eprintln!(
        "sikemux core: stopping{}, as the client with pid {} asked",
        if stop_all { " everything it runs" } else { "" },
        client
            .pid
            .map_or_else(|| "unknown".to_owned(), |pid| pid.to_string())
    );
    client.answer::<ask::Shutdown>(request_id, Ok(()));
    client.wait_flushed(SHUTDOWN_FLUSH).await;
    core.begin_shutdown();
}
