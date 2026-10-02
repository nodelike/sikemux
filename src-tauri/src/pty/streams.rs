use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

use sikemux_core::protocol::SessionId;
use sikemux_pty::task::TaskProcessExit;
use tauri::ipc::{Channel, Response};

use crate::error::{AppError, AppResult};

/// The core counts this many subscribers per session too, but sees the whole
/// app as one of them.
const MAX_CHANNELS_PER_SESSION: usize = 16;
const MAX_SUB_ID_PROBES: usize = MAX_CHANNELS_PER_SESSION + 1;

/// One attached xterm, and how many bytes it was sent but has not yet
/// reported writing.
struct Subscriber {
    channel: Channel<Response>,
    unacked: usize,
}

/// The app's side of one core session: the webview channels showing it, and
/// the channel waiting for a task's exit.
#[derive(Default)]
struct Streams {
    subscribers: HashMap<u32, Subscriber>,
    pending_attaches: usize,
    /// Whether the core sends this session's output to the app.
    core_subscribed: bool,
    /// What the core believes the app still owes it, so acks can be
    /// forwarded as the slowest channel catches up.
    core_unacked: usize,
    task_exit: Option<Channel<TaskProcessExit>>,
}

impl Streams {
    fn slowest(&self) -> usize {
        self.subscribers
            .values()
            .map(|subscriber| subscriber.unacked)
            .max()
            .unwrap_or(0)
    }

    fn is_unused(&self) -> bool {
        self.subscribers.is_empty()
            && self.pending_attaches == 0
            && !self.core_subscribed
            && self.task_exit.is_none()
    }
}

/// What a caller must tell the core after a channel left.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Release {
    Nothing,
    Ack(usize),
    Detach,
}

pub(super) struct Delivery {
    channels: Vec<(u32, Channel<Response>)>,
}

impl Delivery {
    pub(super) fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Returns the subscribers whose webview is gone.
    pub(super) fn send(&self, bytes: &[u8]) -> Vec<u32> {
        self.channels
            .iter()
            .filter(|(_, channel)| channel.send(Response::new(bytes.to_vec())).is_err())
            .map(|(sub_id, _)| *sub_id)
            .collect()
    }
}

/// Every session this app has a channel for, keyed by core session id.
#[derive(Default)]
pub(super) struct StreamTable {
    sessions: Mutex<HashMap<SessionId, Streams>>,
    next_sub_id: AtomicU32,
}

impl StreamTable {
    pub(super) fn lock(&self) -> AppResult<StreamGuard<'_>> {
        Ok(StreamGuard {
            sessions: self
                .sessions
                .lock()
                .map_err(|_| AppError::Pty("stream table lock poisoned".into()))?,
            next_sub_id: &self.next_sub_id,
        })
    }

    fn with<T>(&self, work: impl FnOnce(&mut StreamGuard<'_>) -> T) -> Option<T> {
        self.lock().ok().map(|mut guard| work(&mut guard))
    }

    /// Hands one chunk of output to every channel of the session, charging it
    /// against each one's credit.
    pub(super) fn output(&self, id: SessionId, bytes: &[u8]) -> Delivery {
        let channels = self
            .with(|guard| {
                let Some(streams) = guard.sessions.get_mut(&id) else {
                    return Vec::new();
                };
                streams.core_unacked += bytes.len();
                streams
                    .subscribers
                    .iter_mut()
                    .map(|(sub_id, subscriber)| {
                        subscriber.unacked += bytes.len();
                        (*sub_id, subscriber.channel.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Delivery { channels }
    }

    pub(super) fn drop_dead(&self, id: SessionId, dead: &[u32]) {
        self.with(|guard| {
            if let Some(streams) = guard.sessions.get_mut(&id) {
                for sub_id in dead {
                    streams.subscribers.remove(sub_id);
                }
            }
        });
    }

    /// Records a channel's progress. Returns how much the core can be told,
    /// which is how far the slowest channel has come since it was last told.
    pub(super) fn ack(&self, id: SessionId, sub_id: u32, bytes: usize) -> Option<usize> {
        self.with(|guard| {
            let streams = guard.sessions.get_mut(&id)?;
            let subscriber = streams.subscribers.get_mut(&sub_id)?;
            subscriber.unacked = subscriber.unacked.saturating_sub(bytes);
            guard.settle(id)
        })
        .flatten()
    }

    /// Takes the session's exit channel and the channels to tell, then forgets
    /// it if nothing else holds it.
    pub(super) fn exited(&self, id: SessionId) -> (Option<Channel<TaskProcessExit>>, Delivery) {
        self.with(|guard| {
            let Some(streams) = guard.sessions.get_mut(&id) else {
                return (None, Vec::new());
            };
            let task_exit = streams.task_exit.take();
            let channels = streams
                .subscribers
                .iter()
                .map(|(sub_id, subscriber)| (*sub_id, subscriber.channel.clone()))
                .collect();
            guard.forget_if_unused(id);
            (task_exit, channels)
        })
        .map(|(task_exit, channels)| (task_exit, Delivery { channels }))
        .unwrap_or((
            None,
            Delivery {
                channels: Vec::new(),
            },
        ))
    }

    pub(super) fn channels(&self, id: SessionId) -> Delivery {
        let channels = self
            .with(|guard| {
                guard
                    .sessions
                    .get(&id)
                    .map(|streams| {
                        streams
                            .subscribers
                            .iter()
                            .map(|(sub_id, subscriber)| (*sub_id, subscriber.channel.clone()))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        Delivery { channels }
    }

    /// Forgets every session at once, returning what must hear that their
    /// processes are gone.
    pub(super) fn take_all(&self) -> Vec<(Option<Channel<TaskProcessExit>>, Delivery)> {
        self.with(|guard| {
            std::mem::take(&mut *guard.sessions)
                .into_values()
                .map(|streams| {
                    let channels = streams
                        .subscribers
                        .into_iter()
                        .map(|(sub_id, subscriber)| (sub_id, subscriber.channel))
                        .collect();
                    (streams.task_exit, Delivery { channels })
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Forgets every session without telling anyone it ended, returning the
    /// ones the core was streaming to the app.
    pub(super) fn release_all(&self) -> Vec<SessionId> {
        self.with(|guard| {
            std::mem::take(&mut *guard.sessions)
                .into_iter()
                .filter(|(_, streams)| streams.core_subscribed)
                .map(|(id, _)| id)
                .collect()
        })
        .unwrap_or_default()
    }

    /// Sessions the core was streaming to the app.
    pub(super) fn core_subscribed(&self) -> Vec<SessionId> {
        self.with(|guard| {
            guard
                .sessions
                .iter()
                .filter(|(_, streams)| streams.core_subscribed)
                .map(|(id, _)| *id)
                .collect()
        })
        .unwrap_or_default()
    }

    /// Tasks whose exit a channel waits for.
    pub(super) fn watched_tasks(&self) -> Vec<SessionId> {
        self.with(|guard| {
            guard
                .sessions
                .iter()
                .filter(|(_, streams)| streams.task_exit.is_some())
                .map(|(id, _)| *id)
                .collect()
        })
        .unwrap_or_default()
    }

    /// A new connection streams the session afresh and owes nothing yet.
    pub(super) fn restart(&self, id: SessionId) {
        self.with(|guard| {
            if let Some(streams) = guard.sessions.get_mut(&id) {
                streams.core_unacked = 0;
            }
        });
    }

    pub(super) fn subscriber_count(&self) -> usize {
        self.with(|guard| {
            guard
                .sessions
                .values()
                .map(|streams| streams.subscribers.len())
                .sum()
        })
        .unwrap_or(0)
    }
}

/// Held while deciding what to ask the core, and while queueing the request,
/// so the core sees attaches and detaches in the order they were decided.
pub(super) struct StreamGuard<'a> {
    sessions: MutexGuard<'a, HashMap<SessionId, Streams>>,
    next_sub_id: &'a AtomicU32,
}

impl StreamGuard<'_> {
    pub(super) fn register_task(&mut self, id: SessionId, exit: Channel<TaskProcessExit>) {
        self.sessions.entry(id).or_default().task_exit = Some(exit);
    }

    pub(super) fn take_task_exit(&mut self, id: SessionId) -> Option<Channel<TaskProcessExit>> {
        let exit = self.sessions.get_mut(&id)?.task_exit.take();
        self.forget_if_unused(id);
        exit
    }

    pub(super) fn is_core_subscribed(&self, id: SessionId) -> bool {
        self.sessions
            .get(&id)
            .is_some_and(|streams| streams.core_subscribed)
    }

    pub(super) fn begin_attach(&mut self, id: SessionId) -> AppResult<()> {
        let streams = self.sessions.entry(id).or_default();
        let full = streams.subscribers.len() + streams.pending_attaches >= MAX_CHANNELS_PER_SESSION;
        if !full {
            streams.pending_attaches += 1;
            return Ok(());
        }
        self.forget_if_unused(id);
        Err(AppError::Pty("PTY subscriber capacity reached".into()))
    }

    /// The core answered an attach with a replay: from here it sends every
    /// byte after it, and owes the app nothing from before.
    pub(super) fn finish_attach(
        &mut self,
        id: SessionId,
        channel: Channel<Response>,
    ) -> AppResult<u32> {
        let streams = self.sessions.entry(id).or_default();
        streams.pending_attaches = streams.pending_attaches.saturating_sub(1);
        streams.core_unacked = 0;
        self.add_channel(id, channel)
    }

    pub(super) fn cancel_attach(&mut self, id: SessionId) {
        if let Some(streams) = self.sessions.get_mut(&id) {
            streams.pending_attaches = streams.pending_attaches.saturating_sub(1);
        }
        self.forget_if_unused(id);
    }

    /// Adds a channel to a session the core already streams to the app.
    pub(super) fn add_channel(
        &mut self,
        id: SessionId,
        channel: Channel<Response>,
    ) -> AppResult<u32> {
        let streams = self.sessions.entry(id).or_default();
        if streams.subscribers.len() >= MAX_CHANNELS_PER_SESSION {
            return Err(AppError::Pty("PTY subscriber capacity reached".into()));
        }
        streams.core_subscribed = true;
        // A wrapped counter skips zero and every live id rather than replace
        // a channel a late unsubscribe still names.
        for _ in 0..MAX_SUB_ID_PROBES {
            let sub_id = self.next_sub_id.fetch_add(1, Ordering::Relaxed);
            if sub_id == 0 {
                continue;
            }
            if let Entry::Vacant(entry) = streams.subscribers.entry(sub_id) {
                entry.insert(Subscriber {
                    channel,
                    unacked: 0,
                });
                return Ok(sub_id);
            }
        }
        Err(AppError::Pty("PTY subscriber id capacity exhausted".into()))
    }

    /// Removes a channel. The last one out, with no attach on its way, stops
    /// the core streaming the session to the app.
    pub(super) fn unsubscribe(&mut self, id: SessionId, sub_id: u32) -> Release {
        let Some(streams) = self.sessions.get_mut(&id) else {
            return Release::Nothing;
        };
        streams.subscribers.remove(&sub_id);
        if !streams.subscribers.is_empty() || streams.pending_attaches > 0 {
            return self.settle(id).map_or(Release::Nothing, Release::Ack);
        }
        let detach = std::mem::take(&mut streams.core_subscribed);
        streams.core_unacked = 0;
        self.forget_if_unused(id);
        if detach {
            Release::Detach
        } else {
            Release::Nothing
        }
    }

    fn settle(&mut self, id: SessionId) -> Option<usize> {
        let streams = self.sessions.get_mut(&id)?;
        let slowest = streams.slowest();
        let forward = streams.core_unacked.checked_sub(slowest)?;
        if forward == 0 {
            return None;
        }
        streams.core_unacked = slowest;
        Some(forward)
    }

    fn forget_if_unused(&mut self, id: SessionId) {
        if self.sessions.get(&id).is_some_and(Streams::is_unused) {
            self.sessions.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{Release, StreamTable, MAX_CHANNELS_PER_SESSION};

    type Received = Arc<Mutex<Vec<Vec<u8>>>>;

    fn channel() -> (tauri::ipc::Channel<tauri::ipc::Response>, Received) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = received.clone();
        let channel = tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Raw(bytes) = body {
                sink.lock().expect("received").push(bytes);
            }
            Ok(())
        });
        (channel, received)
    }

    fn attached(table: &StreamTable, id: u64) -> (u32, Received) {
        let (channel, received) = channel();
        let mut guard = table.lock().expect("lock");
        guard.begin_attach(id).expect("begin");
        let sub_id = guard.finish_attach(id, channel).expect("finish");
        (sub_id, received)
    }

    #[test]
    fn output_reaches_every_channel_and_acks_follow_the_slowest() {
        let table = StreamTable::default();
        let (fast, fast_bytes) = attached(&table, 7);
        let (slow, slow_bytes) = attached(&table, 7);
        assert!(table.output(7, b"hello").send(b"hello").is_empty());
        for received in [fast_bytes, slow_bytes] {
            assert_eq!(
                received.lock().expect("bytes").as_slice(),
                [b"hello".to_vec()]
            );
        }

        assert_eq!(table.ack(7, fast, 5), None, "the slow channel still owes");
        assert_eq!(table.ack(7, slow, 3), Some(3));
        assert_eq!(table.ack(7, slow, 2), Some(2));
        assert_eq!(table.ack(7, slow, 2), None);
    }

    #[test]
    fn the_last_channel_out_detaches_unless_an_attach_is_on_its_way() {
        let table = StreamTable::default();
        let (first, _) = attached(&table, 1);
        let (second, _) = attached(&table, 1);
        let mut guard = table.lock().expect("lock");
        assert_eq!(guard.unsubscribe(1, first), Release::Nothing);
        guard.begin_attach(1).expect("begin");
        assert_eq!(guard.unsubscribe(1, second), Release::Nothing);
        assert!(guard.is_core_subscribed(1));
        let (channel, _) = channel();
        let third = guard.finish_attach(1, channel).expect("finish");
        assert_eq!(guard.unsubscribe(1, third), Release::Detach);
        assert!(!guard.is_core_subscribed(1));
        assert_eq!(guard.unsubscribe(1, third), Release::Nothing);
    }

    #[test]
    fn a_leaving_slow_channel_releases_what_it_held() {
        let table = StreamTable::default();
        let (fast, _) = attached(&table, 3);
        let (slow, _) = attached(&table, 3);
        table.output(3, &[0; 10]);
        assert_eq!(table.ack(3, fast, 10), None);
        assert_eq!(
            table.lock().expect("lock").unsubscribe(3, slow),
            Release::Ack(10)
        );
    }

    #[test]
    fn a_task_exit_is_delivered_once_and_the_session_is_then_forgotten() {
        let table = StreamTable::default();
        let (exit, _) = {
            let received = Arc::new(Mutex::new(0));
            let counter = received.clone();
            (
                tauri::ipc::Channel::new(move |_| {
                    *counter.lock().expect("count") += 1;
                    Ok(())
                }),
                received,
            )
        };
        table.lock().expect("lock").register_task(9, exit);
        let (task_exit, delivery) = table.exited(9);
        assert!(task_exit.is_some());
        assert!(delivery.is_empty());
        let (again, _) = table.exited(9);
        assert!(again.is_none());
        assert!(table.take_all().is_empty());
    }

    #[test]
    fn releasing_everything_names_only_what_the_core_streams_to_the_app() {
        let table = StreamTable::default();
        let _ = attached(&table, 4);
        table.lock().expect("lock").register_task(
            8,
            tauri::ipc::Channel::<sikemux_pty::task::TaskProcessExit>::new(|_| Ok(())),
        );
        assert_eq!(table.release_all(), vec![4]);
        assert!(table.take_all().is_empty());
    }

    #[test]
    fn a_watched_task_exit_is_taken_once() {
        let table = StreamTable::default();
        let mut guard = table.lock().expect("lock");
        guard.register_task(
            6,
            tauri::ipc::Channel::<sikemux_pty::task::TaskProcessExit>::new(|_| Ok(())),
        );
        assert!(guard.take_task_exit(6).is_some());
        assert!(guard.take_task_exit(6).is_none());
        drop(guard);
        assert!(table.take_all().is_empty());
    }

    #[test]
    fn a_reconnect_finds_what_the_core_streamed_and_the_tasks_still_watched() {
        let table = StreamTable::default();
        let (fast, _) = attached(&table, 2);
        let _ = attached(&table, 4);
        table.lock().expect("lock").register_task(
            9,
            tauri::ipc::Channel::<sikemux_pty::task::TaskProcessExit>::new(|_| Ok(())),
        );
        table.output(2, &[0; 10]);
        let mut streamed = table.core_subscribed();
        streamed.sort_unstable();
        assert_eq!(streamed, vec![2, 4]);
        assert_eq!(table.watched_tasks(), vec![9]);

        table.restart(2);
        assert_eq!(
            table.ack(2, fast, 10),
            None,
            "a new connection is owed nothing from before it"
        );
    }

    #[test]
    fn channel_capacity_counts_attaches_in_flight() {
        let table = StreamTable::default();
        let mut guard = table.lock().expect("lock");
        for _ in 0..MAX_CHANNELS_PER_SESSION {
            guard.begin_attach(5).expect("below the cap");
        }
        assert!(guard.begin_attach(5).is_err());
        guard.cancel_attach(5);
        guard.begin_attach(5).expect("a slot came back");
    }

    #[test]
    fn wrapped_subscription_ids_skip_zero_and_live_ids() {
        let table = StreamTable::default();
        table
            .next_sub_id
            .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
        let mut guard = table.lock().expect("lock");
        let (first, _) = channel();
        let (second, _) = channel();
        assert_eq!(guard.add_channel(2, first).expect("first"), u32::MAX);
        assert_eq!(guard.add_channel(2, second).expect("second"), 1);
    }
}
