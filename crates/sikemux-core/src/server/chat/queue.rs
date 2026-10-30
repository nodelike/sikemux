//! Messages the person sent while the agent was at work. They wait in the
//! chat's feed, where every app sees them, and go out together once the turn
//! ends, unless the person steers them in sooner or takes one back.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::protocol::ChatContext;

use super::super::{Core, CoreResult};
use super::{Chat, ChatCommand};

/// At most this many messages wait behind a turn.
pub(crate) const MAX_QUEUED: usize = 20;

/// How long the messages wait after a turn ends, for word that the network
/// cut it short, which holds them until it goes again.
const SETTLE: Duration = Duration::from_millis(150);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Queued {
    pub id: String,
    pub text: String,
    pub paths: Vec<String>,
    pub context: Vec<ChatContext>,
}

/// A message as the person wrote it.
#[derive(Deserialize)]
pub(crate) struct Message {
    text: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    context: Vec<ChatContext>,
}

impl Queued {
    /// Kept under the id the client sent it with, which its prompt then goes
    /// out with, so the client knows its message again.
    pub(crate) fn new(
        message_id: Option<String>,
        text: String,
        paths: Vec<String>,
        context: Vec<ChatContext>,
    ) -> Self {
        Self {
            id: message_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            text,
            paths,
            context,
        }
    }

    fn is_command(&self) -> bool {
        self.text.starts_with('/')
    }
}

/// How many of the first messages go out together. A slash command only
/// works as the whole prompt, so it goes on its own.
pub(crate) fn batch_len(queued: &[Queued]) -> usize {
    match queued.first() {
        None => 0,
        Some(first) if first.is_command() => 1,
        Some(_) => queued
            .iter()
            .position(Queued::is_command)
            .unwrap_or(queued.len()),
    }
}

/// Messages sent as one: their texts in order, and each attachment once.
fn combine(messages: Vec<Queued>) -> (String, Vec<String>, Vec<ChatContext>) {
    let text = messages
        .iter()
        .map(|message| message.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    let mut context = Vec::new();
    for message in messages {
        paths.extend(
            message
                .paths
                .into_iter()
                .filter(|path| seen.insert(format!("path:{path}"))),
        );
        context.extend(
            message
                .context
                .into_iter()
                .filter(|item| seen.insert(format!("uri:{}", item.uri))),
        );
    }
    (text, paths, context)
}

impl Chat {
    /// Sends what waits once the turn has settled, unless another began or
    /// the network cut this one short.
    pub(crate) fn send_queued_later(&self) {
        let core = self.core.clone();
        let agent_id = self.agent_id().to_owned();
        let generation = self.generation;
        tokio::spawn(async move {
            tokio::time::sleep(SETTLE).await;
            let Some(chat) = core.upgrade().and_then(|core| core.chats.get(&agent_id)) else {
                return;
            };
            if chat.generation != generation
                || chat.turn_running()
                || chat.offline_failed.load(Ordering::Acquire)
            {
                return;
            }
            let batch = chat.feed.take_queued(None);
            if !batch.is_empty() {
                send(&chat, batch);
            }
        });
    }
}

fn send(chat: &Chat, batch: Vec<Queued>) {
    let message_id = batch.first().map(|message| message.id.clone());
    let (text, paths, context) = combine(batch);
    let _ = chat.send(ChatCommand::Prompt {
        message_id,
        text,
        paths,
        context,
        shown: false,
    });
}

/// Puts the waiting messages `ids`, and then `message`, into the running turn
/// now, as one, or starts a turn with them when the last one has just ended.
pub(crate) async fn steer(
    core: &Arc<Core>,
    agent_id: &str,
    ids: Vec<String>,
    message: Option<Message>,
) -> CoreResult<()> {
    let chat = core.chats.running(agent_id)?;
    let message = match message {
        Some(mut message) => {
            super::mentions::wake(core, agent_id, &mut message.context).await?;
            Some(message)
        }
        None => None,
    };
    let mut taken = chat.feed.take_queued(Some(&ids));
    taken.extend(message.map(|mut message| {
        super::mentions::fill(core, agent_id, &mut message.context);
        Queued::new(None, message.text, message.paths, message.context)
    }));
    if taken.is_empty() {
        return Ok(());
    }
    let (text, paths, context) = combine(taken.clone());
    if super::steer(core, agent_id, text, paths, context).await? == "promptRequired" {
        send(&chat, taken);
    }
    Ok(())
}

/// Takes back the waiting messages `ids`.
pub(crate) fn remove(core: &Core, agent_id: &str, ids: &[String]) -> CoreResult<()> {
    core.chats.running(agent_id)?.feed.take_queued(Some(ids));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(text: &str, paths: &[&str], uris: &[&str]) -> Queued {
        Queued::new(
            None,
            text.into(),
            paths.iter().map(|path| (*path).to_owned()).collect(),
            uris.iter()
                .map(|uri| ChatContext {
                    uri: (*uri).to_owned(),
                    title: (*uri).to_owned(),
                    text: "body".into(),
                })
                .collect(),
        )
    }

    #[test]
    fn plain_messages_go_together_and_a_command_on_its_own() {
        let plain = |text| queued(text, &[], &[]);
        assert_eq!(batch_len(&[plain("a"), plain("b")]), 2);
        assert_eq!(batch_len(&[plain("/compact"), plain("b")]), 1);
        assert_eq!(
            batch_len(&[plain("a"), plain("b"), plain("/review"), plain("c")]),
            2
        );
    }

    #[test]
    fn combined_messages_join_their_texts_and_name_each_attachment_once() {
        let (text, paths, context) = combine(vec![
            queued("a", &["/x", "/y"], &["u1"]),
            queued("", &[], &[]),
            queued("c", &["/y", "/z"], &["u1", "u2"]),
        ]);
        assert_eq!(text, "a\n\nc");
        assert_eq!(paths, ["/x", "/y", "/z"]);
        let uris: Vec<_> = context.iter().map(|item| item.uri.as_str()).collect();
        assert_eq!(uris, ["u1", "u2"]);
    }
}
