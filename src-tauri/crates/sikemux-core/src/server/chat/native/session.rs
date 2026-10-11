//! The part of a chat that is the same for every agent the core speaks to in
//! its own protocol: turns, permission requests, accounts and starting the
//! agent again under the chat.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::acp::account::{self, Failure, FailureKind};
use crate::acp::SessionEnd;
use crate::protocol::{ChatContext, ChatEventKind, ChatLaunch, ChatStart};

use super::super::rebind::{
    error_message, next_account, on_account, switch_notice, Carried, Outcome, Rebind, Replay,
};
use super::super::{Chat, ChatCommand};

/// How long a stopped turn may keep running before the agent is killed.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

/// What the person sent, for the agent to read in its own form.
#[derive(Clone)]
pub(crate) struct Prompt {
    pub message_id: Option<String>,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
}

impl From<&Replay> for Prompt {
    fn from(replay: &Replay) -> Self {
        Self {
            message_id: replay.message_id.clone(),
            text: replay.text.clone(),
            paths: replay.paths.clone(),
            context: replay.context.clone(),
        }
    }
}

/// The most of any one text an update carries. The chat shows only the first
/// few hundred lines of what a tool printed, and a long chat loaded again
/// would otherwise send all of it.
const MAX_TEXT: usize = 128 * 1024;
/// The largest picture an update carries, as base64. The chat keeps none
/// bigger.
const MAX_IMAGE: usize = 2 * 1024 * 1024;

/// Cuts every text in `value` down to what the chat can show.
fn slim(value: &mut Value) {
    match value {
        Value::String(text) if text.len() > MAX_TEXT => {
            let mut end = MAX_TEXT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push('…');
        }
        Value::Array(items) => items.iter_mut().for_each(slim),
        Value::Object(fields) => {
            let picture = fields.get("data").and_then(Value::as_str).map(str::len);
            if picture.is_some_and(|len| len > MAX_IMAGE) {
                fields.remove("data");
            }
            for (key, field) in fields.iter_mut() {
                if key != "data" {
                    slim(field);
                }
            }
        }
        _ => {}
    }
}

/// How a turn this side started ended.
pub(crate) enum TurnEnd {
    /// The `turn_completed` payload, such as `{"stopReason": "end_turn"}`.
    Completed(Value),
    /// The agent could not finish it, and said why.
    Failed {
        failure: Failure,
        response: Option<Value>,
    },
}

/// A turn that failed in a way a new agent process or another account may fix.
pub(crate) struct Failed {
    turn: u64,
    failure: Failure,
    response: Option<Value>,
}

/// What an agent's reader tells the chat. Cheap to clone, and safe to use
/// from any task.
#[derive(Clone)]
pub(crate) struct Sink {
    chat: Arc<Chat>,
    failed: mpsc::UnboundedSender<Failed>,
    account: Option<String>,
    /// Quiet while a session loaded under a running chat replays history the
    /// chat already shows.
    quiet: Arc<std::sync::atomic::AtomicBool>,
}

impl Sink {
    pub fn update(&self, session_id: &str, mut update: Value) {
        if self.quiet.load(Ordering::Acquire) {
            return;
        }
        slim(&mut update);
        self.chat.emit(
            ChatEventKind::SessionUpdate,
            json!({ "sessionId": session_id, "update": update }),
        );
    }

    /// Whether a turn this side prompted is being answered.
    pub fn running(&self) -> bool {
        self.chat.running.load(Ordering::Acquire)
    }

    /// The agent began work nobody here asked for, such as answering a
    /// background task that finished.
    pub fn work_started(&self) {
        if !self.chat.running.load(Ordering::Acquire)
            && !self.chat.unprompted.swap(true, Ordering::AcqRel)
        {
            self.chat.emit(ChatEventKind::TurnStarted, json!({}));
        }
    }

    /// The work nobody here asked for is over.
    pub fn work_ended(&self, stop_reason: &str) {
        if self.chat.unprompted.swap(false, Ordering::AcqRel) {
            self.chat.emit(
                ChatEventKind::TurnCompleted,
                json!({ "stopReason": stop_reason }),
            );
        }
    }

    /// Settles turn `turn`, or hands a failure another account may fix to
    /// the chat's loop to decide on, with the turn still running.
    pub fn end_turn(&self, turn: u64, end: TurnEnd) {
        let chat = &self.chat;
        chat.unprompted.store(false, Ordering::Release);
        chat.cancel_permissions();
        let settle = || chat.running.store(false, Ordering::Release);
        match end {
            TurnEnd::Completed(payload) => {
                settle();
                chat.answered.store(true, Ordering::Release);
                chat.emit(ChatEventKind::TurnCompleted, payload);
            }
            TurnEnd::Failed { failure, response } if failure.kind != FailureKind::Other => {
                let _ = self.failed.send(Failed {
                    turn,
                    failure,
                    response,
                });
            }
            TurnEnd::Failed { failure, response } => {
                settle();
                chat.emit(
                    ChatEventKind::TurnCompleted,
                    response.unwrap_or_else(|| json!({ "stopReason": "end_turn" })),
                );
                chat.emit(
                    ChatEventKind::Error,
                    failure.payload(self.account.as_deref()),
                );
            }
        }
    }

    /// Asks the person about a tool call. `request` is shaped as an ACP
    /// permission request: the session, the tool call and its `options`.
    /// Answers with the chosen option's id, or none when it was cancelled.
    pub async fn ask(&self, mut request: Value) -> Option<String> {
        let options: Vec<(String, String)> = request
            .get("options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| {
                        Some((
                            option.get("optionId")?.as_str()?.to_owned(),
                            option.get("kind")?.as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if self.chat.approves() {
            let allowed = options
                .iter()
                .find(|(_, kind)| kind == "allow_once")
                .or_else(|| options.iter().find(|(_, kind)| kind == "allow_always"));
            if let Some((id, _)) = allowed {
                return Some(id.clone());
            }
        }
        let request_id = Uuid::new_v4().to_string();
        if let Some(object) = request.as_object_mut() {
            object.insert("requestId".into(), Value::String(request_id.clone()));
        }
        let (answer, answered) = oneshot::channel();
        let ids = options.into_iter().map(|(id, _)| id).collect();
        if self
            .chat
            .hold_permission(request_id, ids, answer, request.clone())
        {
            self.chat.emit(ChatEventKind::PermissionRequest, request);
        }
        answered.await.ok().flatten()
    }
}

/// The session an agent opened, as the chat reads it.
pub(crate) struct Opened {
    pub session_id: String,
    pub capabilities: Value,
    pub setup: Value,
}

/// One agent the core speaks to in its own protocol.
pub(crate) trait Backend: Sized + Send {
    /// Starts the agent and opens the chat's session, or loads the one it
    /// resumes, replaying its history through `sink`.
    fn start(
        launch: &ChatLaunch,
        sink: Sink,
    ) -> impl std::future::Future<Output = Result<(Self, Opened), String>> + Send;

    /// Sends a prompt that begins turn `turn`. How it ends is told to the
    /// sink.
    fn prompt(
        &mut self,
        turn: u64,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Puts a message into the running turn. `injected` once it is in,
    /// `promptRequired` when the turn ended first.
    fn steer(
        &mut self,
        prompt: Prompt,
    ) -> impl std::future::Future<Output = Result<String, String>> + Send;

    /// Asks the running turn to stop.
    fn cancel(&mut self) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Applies one of the app's permission modes.
    fn set_permission_mode(
        &mut self,
        mode: &str,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Changes a session option, answering with the options as they now are.
    fn set_config(
        &mut self,
        config_id: &str,
        value: &str,
    ) -> impl std::future::Future<Output = Result<Value, String>> + Send;

    fn stop_task(
        &mut self,
        task_id: &str,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Takes the conversation back to before the person's message
    /// `message_id`, and the files too when `restore_files`. Answers with the
    /// session's new id when the agent had to start a new one to do it.
    fn rewind(
        &mut self,
        message_id: &str,
        restore_files: bool,
    ) -> impl std::future::Future<Output = Result<Option<String>, String>> + Send;

    /// Resolves once the agent is gone, with why.
    fn closed(&self) -> impl std::future::Future<Output = String> + Send;
}

pub(crate) async fn run<B: Backend>(
    chat: Arc<Chat>,
    launch: ChatLaunch,
    commands: &mut mpsc::UnboundedReceiver<ChatCommand>,
    rebind: Option<Box<Rebind>>,
) -> Result<Outcome, String> {
    chat.emit(ChatEventKind::Status, json!({ "state": "starting" }));
    let provider = launch.provider.clone();
    let signed_in_at_start = account::signed_in(&provider, &launch.env);
    let Carried {
        replay,
        mut exhausted,
        mut signed_in_again,
        notice,
        quiet_load,
    } = Carried::from(rebind);

    let (failed_tx, mut failed_rx) = mpsc::unbounded_channel::<Failed>();
    let quiet = Arc::new(std::sync::atomic::AtomicBool::new(quiet_load));
    let sink = Sink {
        chat: chat.clone(),
        failed: failed_tx,
        account: launch.account.as_ref().map(|account| account.id.clone()),
        quiet: quiet.clone(),
    };
    chat.emit(ChatEventKind::Status, json!({ "state": "initializing" }));
    let (mut backend, opened) = B::start(&launch, sink.clone()).await?;
    quiet.store(false, Ordering::Release);

    let mut session_id = opened.session_id.clone();
    let mut setup = opened.setup;
    let start = ChatStart {
        session_id: session_id.clone(),
        capabilities: opened.capabilities,
        setup: setup.clone(),
    };
    chat.mark_ready(start.clone());
    chat.emit(
        ChatEventKind::Ready,
        serde_json::to_value(&start).unwrap_or_else(|_| json!({})),
    );

    let mut turn: u64 = 0;
    let (stalled_tx, mut stalled_rx) = mpsc::unbounded_channel::<u64>();
    // The prompt the running turn answers, and the one whose turn failed for
    // want of an account, which a switch sends again.
    let mut last_prompt: Option<Replay> = None;
    let mut failed_prompt: Option<Replay> = None;

    if let Some(update) = notice {
        sink.update(&session_id, update);
    }
    if let Some(replay) = replay {
        if let Some(from) = replay.announce {
            chat.feed.prompted(
                from,
                replay.message_id.as_deref(),
                &replay.text,
                &replay.paths,
            );
        }
        chat.running.store(true, Ordering::Release);
        turn += 1;
        begin(&chat, &mut backend, turn, Prompt::from(&replay)).await;
        last_prompt = Some(Replay {
            announce: None,
            ..replay
        });
    }

    let end = loop {
        let command = tokio::select! {
            command = commands.recv() => match command {
                Some(command) => command,
                None => break Outcome::Ended(SessionEnd::Requested),
            },
            reason = backend.closed() => {
                if chat.running.load(Ordering::Acquire) {
                    chat.emit(ChatEventKind::Error, error_message(&reason));
                }
                break Outcome::Ended(SessionEnd::Exited);
            }
            Some(failed) = failed_rx.recv() => {
                if failed.turn != turn {
                    continue;
                }
                let current = launch.account.as_ref().map(|account| account.id.clone());
                let retry = match failed.failure.kind {
                    FailureKind::SignIn if !signed_in_again => chat
                        .relaunch(&launch)
                        .map(|launch| Rebind {
                            launch,
                            replay: last_prompt.clone(),
                            exhausted: exhausted.clone(),
                            signed_in_again: true,
                            notice: None,
                        }),
                    FailureKind::Limit => {
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
                // A sign-in made elsewhere since the agent started is only
                // read by a new agent process.
                let changed = account::signed_in(&provider, &launch.env);
                if changed.is_some()
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
                chat.feed.prompted(
                    from,
                    replay.message_id.as_deref(),
                    &replay.text,
                    &replay.paths,
                );
                turn += 1;
                begin(&chat, &mut backend, turn, Prompt::from(&replay)).await;
                last_prompt = Some(Replay {
                    announce: None,
                    ..replay
                });
            }
            ChatCommand::Edit {
                from,
                message_id,
                text,
                paths,
                context,
                restore_files,
                reply,
            } => {
                if chat.running.load(Ordering::Acquire) {
                    let _ =
                        reply.send(Err("Stop the current turn before editing a message".into()));
                    continue;
                }
                match backend.rewind(&message_id, restore_files).await {
                    Ok(Some(renamed)) => {
                        chat.feed.rewound(from, &session_id, &message_id);
                        session_id = renamed;
                        let start = ChatStart {
                            session_id: session_id.clone(),
                            capabilities: start.capabilities.clone(),
                            setup: setup.clone(),
                        };
                        chat.mark_ready(start.clone());
                        chat.emit(
                            ChatEventKind::Ready,
                            serde_json::to_value(&start).unwrap_or_else(|_| json!({})),
                        );
                    }
                    Ok(None) => chat.feed.rewound(from, &session_id, &message_id),
                    Err(error) => {
                        let _ = reply.send(Err(error));
                        continue;
                    }
                }
                let _ = reply.send(Ok(()));
                exhausted.clear();
                signed_in_again = false;
                failed_prompt = None;
                let replay = Replay {
                    announce: None,
                    message_id: Some(message_id),
                    text,
                    paths,
                    context,
                };
                chat.feed.prompted(
                    from,
                    replay.message_id.as_deref(),
                    &replay.text,
                    &replay.paths,
                );
                chat.running.store(true, Ordering::Release);
                turn += 1;
                begin(&chat, &mut backend, turn, Prompt::from(&replay)).await;
                last_prompt = Some(replay);
            }
            ChatCommand::SwitchAccount { account } => {
                if chat.running.load(Ordering::Acquire) {
                    chat.emit(
                        ChatEventKind::Error,
                        error_message("Stop the current turn before switching accounts"),
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
                    backend.set_permission_mode(&mode).await
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
                } else {
                    backend.set_config(&config_id, &value).await
                };
                if let Ok(options) = &result {
                    setup["configOptions"] = options.clone();
                    chat.feed.set_setup(&setup);
                }
                let _ = reply.send(result.map(|options| json!({ "configOptions": options })));
            }
            ChatCommand::Steer {
                from,
                text,
                paths,
                context,
                reply,
            } => {
                if !chat.running.load(Ordering::Acquire) {
                    let _ = reply.send(Ok("promptRequired".to_owned()));
                    continue;
                }
                let prompt = Prompt {
                    message_id: None,
                    text,
                    paths,
                    context,
                };
                let result = backend.steer(prompt.clone()).await;
                if result.as_deref() == Ok("injected") {
                    chat.feed.prompted(from, None, &prompt.text, &prompt.paths);
                }
                let _ = reply.send(result);
            }
            ChatCommand::StopTask { task_id } => {
                if let Err(error) = backend.stop_task(&task_id).await {
                    chat.emit(ChatEventKind::Error, error_message(error));
                }
            }
            ChatCommand::Cancel => {
                if let Err(error) = backend.cancel().await {
                    chat.emit(ChatEventKind::Error, error_message(error));
                }
                if chat.running.load(Ordering::Acquire) {
                    let cancelled = turn;
                    let stalled = stalled_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(CANCEL_GRACE).await;
                        let _ = stalled.send(cancelled);
                    });
                } else {
                    sink.work_ended("cancelled");
                }
            }
        }
    };
    Ok(end)
}

/// Starts turn `turn`, or says why it could not start.
async fn begin<B: Backend>(chat: &Arc<Chat>, backend: &mut B, turn: u64, prompt: Prompt) {
    chat.unprompted.store(false, Ordering::Release);
    chat.emit(ChatEventKind::TurnStarted, json!({}));
    if let Err(error) = backend.prompt(turn, prompt).await {
        chat.running.store(false, Ordering::Release);
        chat.emit(ChatEventKind::Error, error_message(error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_texts_are_cut_and_pictures_kept_whole() {
        let long = "é".repeat(MAX_TEXT);
        let picture = "A".repeat(MAX_TEXT * 2);
        let mut update = json!({
            "rawOutput": long,
            "content": [{ "type": "image", "data": picture, "mimeType": "image/png" }],
            "huge": [{ "type": "image", "source": { "type": "base64", "data": "A".repeat(MAX_IMAGE + 1) } }],
        });
        slim(&mut update);
        let cut = update["rawOutput"].as_str().unwrap();
        assert!(cut.len() <= MAX_TEXT + '…'.len_utf8());
        assert!(cut.ends_with('…'));
        assert_eq!(
            update["content"][0]["data"].as_str().map(str::len),
            Some(MAX_TEXT * 2)
        );
        assert!(update["huge"][0]["source"].get("data").is_none());
    }
}
