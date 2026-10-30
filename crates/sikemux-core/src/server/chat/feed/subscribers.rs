//! The clients hearing a chat: each change goes out once a frame, and a client that falls
//! behind is caught up with how the chat stands once it has read what it has.

use std::sync::Arc;
use std::time::Duration;

use sikemux_wire::chat::ChatChange;

use super::super::super::connection::{ClientConn, ClientId};
use super::super::history::History;
use super::{Feed, Inner, Standing};
use crate::protocol::{
    ask, encode_control, fits, ChatAttachment, ChatMark, Event, RequestId, ServerMessage,
};
use sikemux_fold::Fold;
use sikemux_process::sync::Locked;

/// A client hearing the chat, and the last change it was sent when it fell
/// behind: a slow client is sent only how the chat stands once it catches up.
pub(super) struct Subscriber {
    pub(super) client: Arc<ClientConn>,
    behind: Option<u64>,
}

/// Past this much waiting to go out to a client, it is not sent each change
/// but caught up once it has read what it has.
const SLOW_BYTES: usize = 512 * 1024;

/// How often a client that fell behind is looked at again.
const CATCH_UP: Duration = Duration::from_millis(50);

/// One frame's worth of streamed updates travels as a single event. Each
/// notification on its own costs a script eval in the webview, and an adapter
/// sends one per token.
const FLUSH: Duration = Duration::from_millis(16);

impl Feed {
    fn mark(&self, inner: &Inner) -> ChatMark {
        ChatMark {
            feed: self.id.clone(),
            seq: inner.replay.seq,
        }
    }

    pub(super) fn frame(&self, seq: u64, changes: &[ChatChange]) -> Option<Arc<[u8]>> {
        let frame = encode_control(&ServerMessage::Event {
            event: Event::Chat {
                agent_id: self.agent_id.clone(),
                seq,
                changes: serde_json::to_value(changes).ok()?,
            },
        })
        .ok()?;
        if !fits(&frame) {
            eprintln!(
                "sikemux core: a chat change of {} bytes is too large to send",
                frame.len()
            );
            return None;
        }
        Some(frame.into())
    }

    /// Whether the chat's events hold turns before the fold's first row.
    pub(super) fn holds_older(inner: &Inner) -> bool {
        if inner.has_earlier {
            return true;
        }
        let Some(floor) = inner.fold.as_ref().and_then(Fold::floor) else {
            return false;
        };
        let oldest = inner
            .replay
            .history
            .as_ref()
            .and_then(History::first)
            .or_else(|| inner.replay.entries.front().map(|entry| entry.index));
        oldest.is_some_and(|oldest| oldest < floor)
    }

    /// Sends every client what changed since the last flush. A client behind
    /// on reading is left to catch up once it has.
    pub(super) fn flush_locked(&self, inner: &mut Inner) {
        let Some(fold) = inner.fold.as_mut() else {
            return;
        };
        if !fold.is_dirty() {
            return;
        }
        let changes = fold.flush();
        if inner.holding || changes.is_empty() {
            return;
        }
        let seq = inner.replay.delivered();
        fold.number(seq + 1);
        let frame = self.frame(seq, &changes);
        let mut behind = false;
        inner.subscribers.retain(|_, subscriber| {
            if subscriber.behind.is_none() && subscriber.client.backlog() <= SLOW_BYTES {
                return frame
                    .as_ref()
                    .is_none_or(|frame| subscriber.client.send(frame.clone()));
            }
            subscriber.behind.get_or_insert(seq - 1);
            behind = true;
            !subscriber.client.is_closed()
        });
        if behind {
            self.catch_up_later(inner);
        }
    }

    fn catch_up_later(&self, inner: &mut Inner) {
        if inner.catch_up_scheduled {
            return;
        }
        inner.catch_up_scheduled = true;
        let feed = self.me.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(CATCH_UP).await;
            if let Some(feed) = feed.upgrade() {
                feed.catch_up();
            }
        });
    }

    /// Sends each client that fell behind how the chat stands, once it has
    /// read what was waiting for it.
    fn catch_up(&self) {
        let mut inner = self.inner.locked();
        inner.catch_up_scheduled = false;
        self.flush_locked(&mut inner);
        let seq = inner.replay.seq;
        let mut still = false;
        let ready: Vec<(ClientId, u64)> = inner
            .subscribers
            .iter()
            .filter_map(|(id, subscriber)| {
                let since = subscriber.behind?;
                if subscriber.client.backlog() > SLOW_BYTES / 2 {
                    still = true;
                    return None;
                }
                Some((*id, since))
            })
            .collect();
        for (id, since) in ready {
            let changes = {
                let older = Self::holds_older(&inner);
                let fold = self.fold(&mut inner);
                fold.catch_up(since).unwrap_or_else(|| {
                    vec![ChatChange::View {
                        view: Box::new(fold.view(older)),
                    }]
                })
            };
            let frame = self.frame(seq, &changes);
            if let Some(subscriber) = inner.subscribers.get_mut(&id) {
                subscriber.behind = None;
                if let Some(frame) = frame {
                    subscriber.client.send(frame);
                }
            }
        }
        if still {
            self.catch_up_later(&mut inner);
        }
    }

    fn flush(&self) {
        let mut inner = self.inner.locked();
        inner.flush_scheduled = false;
        self.flush_locked(&mut inner);
        self.let_go(&mut inner);
    }

    pub(super) fn flush_later(self: &Arc<Self>, inner: &mut Inner) {
        if inner.flush_scheduled {
            return;
        }
        inner.flush_scheduled = true;
        let feed = self.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(FLUSH).await;
            feed.flush();
        });
    }

    /// Lets go of the fold of a chat nobody watches and no turn runs in. It is
    /// built again from the chat's events when a client comes.
    pub(super) fn let_go(&self, inner: &mut Inner) {
        if inner.subscribers.is_empty()
            && !inner.replay.in_turn
            && !inner.holding
            && inner.fold.as_ref().is_some_and(|fold| !fold.is_dirty())
        {
            inner.fold = None;
        }
    }

    pub(crate) fn subscribe(&self, client: &Arc<ClientConn>) {
        self.viewed();
        let mut inner = self.inner.locked();
        self.fold(&mut inner);
        inner.subscribers.insert(
            client.id,
            Subscriber {
                client: client.clone(),
                behind: None,
            },
        );
    }

    pub(crate) fn has_subscriber(&self, client: ClientId) -> bool {
        self.inner.locked().subscribers.contains_key(&client)
    }

    pub(crate) fn unsubscribe(&self, client: ClientId) {
        self.viewed();
        let mut inner = self.inner.locked();
        inner.subscribers.remove(&client);
        self.let_go(&mut inner);
    }

    /// Answers the client with the chat as it stands, or what changed since
    /// `since`, and adds it to the listeners in the same step, so it hears
    /// every later change exactly once.
    pub(crate) fn attach(
        &self,
        client: &Arc<ClientConn>,
        request_id: RequestId,
        standing: Standing,
        since: Option<ChatMark>,
    ) {
        self.viewed();
        let mut inner = self.inner.locked();
        if inner.closed || (inner.start.is_none() && !inner.waits_for_person) {
            client.answer::<ask::AcpAttach>(request_id, Ok(ChatAttachment::Missing));
            return;
        }
        let fold = self.fold(&mut inner);
        fold.set_running(standing.running);
        fold.set_turned(standing.turned);
        fold.set_draft(standing.draft);
        self.flush_locked(&mut inner);
        let resumed = since
            .filter(|since| since.feed == self.id && since.seq <= inner.replay.seq)
            .and_then(|since| self.fold(&mut inner).catch_up(since.seq));
        let mark = self.mark(&inner);
        let attachment = match resumed {
            Some(changes) => ChatAttachment::Resumed {
                changes: serde_json::to_value(changes).unwrap_or_default(),
                mark,
            },
            None => {
                let older = Self::holds_older(&inner);
                let view = self.fold(&mut inner).view(older);
                ChatAttachment::Live {
                    view: serde_json::to_value(view).unwrap_or_default(),
                    mark,
                }
            }
        };
        client.answer::<ask::AcpAttach>(request_id, Ok(attachment));
        inner.subscribers.insert(
            client.id,
            Subscriber {
                client: client.clone(),
                behind: None,
            },
        );
    }
}
