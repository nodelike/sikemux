use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use sikemux_core::protocol::{SessionId, StreamId};
use sikemux_process::sync::Locked;
use tauri::ipc::{Channel, Response};

/// One pane showing a terminal: the webview channel it draws from, and the
/// core's stream of frames for it. The stream changes when the core comes
/// back after an update; the pane keeps its id.
struct Pane {
    session: SessionId,
    channel: Channel<Response>,
    stream: StreamId,
}

#[derive(Default)]
struct Panes {
    panes: HashMap<u32, Pane>,
    by_stream: HashMap<StreamId, u32>,
}

/// Every pane this app shows a terminal in, keyed by the id the page holds.
#[derive(Default)]
pub(crate) struct StreamTable {
    panes: Mutex<Panes>,
    next_pane: AtomicU32,
}

impl StreamTable {
    pub(crate) fn add(
        &self,
        session: SessionId,
        channel: Channel<Response>,
        stream: StreamId,
    ) -> u32 {
        let mut panes = self.panes.locked();
        let pane = loop {
            let pane = self.next_pane.fetch_add(1, Ordering::Relaxed);
            if pane != 0 && !panes.panes.contains_key(&pane) {
                break pane;
            }
        };
        panes.by_stream.insert(stream, pane);
        panes.panes.insert(
            pane,
            Pane {
                session,
                channel,
                stream,
            },
        );
        pane
    }

    /// Hands a frame to the pane it is for. Answers the stream when that
    /// pane's webview is gone, for the caller to end.
    pub(crate) fn deliver(&self, stream: StreamId, frame: &[u8]) -> Option<StreamId> {
        let channel = {
            let panes = self.panes.locked();
            let pane = panes.by_stream.get(&stream)?;
            panes.panes.get(pane)?.channel.clone()
        };
        if channel.send(Response::new(frame.to_vec())).is_ok() {
            return None;
        }
        let mut panes = self.panes.locked();
        let pane = panes.by_stream.remove(&stream)?;
        panes.panes.remove(&pane);
        Some(stream)
    }

    pub(crate) fn stream(&self, pane: u32) -> Option<StreamId> {
        self.panes.locked().panes.get(&pane).map(|pane| pane.stream)
    }

    pub(crate) fn remove(&self, pane: u32) -> Option<StreamId> {
        let mut panes = self.panes.locked();
        let removed = panes.panes.remove(&pane)?;
        panes.by_stream.remove(&removed.stream);
        Some(removed.stream)
    }

    /// The channels of every pane showing `session`.
    pub(crate) fn channels(&self, session: SessionId) -> Vec<Channel<Response>> {
        self.panes
            .locked()
            .panes
            .values()
            .filter(|pane| pane.session == session)
            .map(|pane| pane.channel.clone())
            .collect()
    }

    /// Forgets every pane, answering their channels.
    pub(crate) fn take_all(&self) -> Vec<Channel<Response>> {
        let mut panes = self.panes.locked();
        panes.by_stream.clear();
        panes.panes.drain().map(|(_, pane)| pane.channel).collect()
    }

    /// Forgets every pane without telling it anything, answering the streams
    /// the core was sending.
    pub(crate) fn release_all(&self) -> Vec<StreamId> {
        let mut panes = self.panes.locked();
        panes.by_stream.clear();
        panes.panes.drain().map(|(_, pane)| pane.stream).collect()
    }

    /// Every pane and the session it shows.
    pub(crate) fn panes(&self) -> Vec<(u32, SessionId)> {
        self.panes
            .locked()
            .panes
            .iter()
            .map(|(id, pane)| (*id, pane.session))
            .collect()
    }

    /// The pane gets its frames from `stream` now, sent the screen afresh.
    pub(crate) fn restream(&self, pane: u32, stream: StreamId, frame: &[u8]) {
        let mut panes = self.panes.locked();
        let Some(entry) = panes.panes.get_mut(&pane) else {
            return;
        };
        let old = std::mem::replace(&mut entry.stream, stream);
        let channel = entry.channel.clone();
        panes.by_stream.remove(&old);
        panes.by_stream.insert(stream, pane);
        drop(panes);
        let _ = channel.send(Response::new(frame.to_vec()));
    }

    pub(crate) fn subscriber_count(&self) -> usize {
        self.panes.locked().panes.len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::StreamTable;

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

    #[test]
    fn frames_reach_the_pane_of_their_stream_and_follow_it_to_a_new_one() {
        let table = StreamTable::default();
        let (first, first_frames) = channel();
        let (second, second_frames) = channel();
        let pane = table.add(7, first, 11);
        table.add(7, second, 12);
        assert_eq!(table.deliver(11, b"a"), None);
        assert_eq!(table.deliver(12, b"b"), None);
        assert_eq!(
            first_frames.lock().expect("frames").as_slice(),
            [b"a".to_vec()]
        );
        assert_eq!(
            second_frames.lock().expect("frames").as_slice(),
            [b"b".to_vec()]
        );

        table.restream(pane, 21, b"fresh");
        assert_eq!(table.deliver(11, b"stale"), None);
        assert_eq!(table.deliver(21, b"c"), None);
        assert_eq!(
            first_frames.lock().expect("frames").as_slice(),
            [b"a".to_vec(), b"fresh".to_vec(), b"c".to_vec()]
        );
        assert_eq!(table.stream(pane), Some(21));
        assert_eq!(table.channels(7).len(), 2);
    }

    #[test]
    fn releasing_everything_names_the_streams_and_forgets_the_panes() {
        let table = StreamTable::default();
        let (channel, _) = channel();
        table.add(4, channel, 9);
        assert_eq!(table.release_all(), vec![9]);
        assert!(table.take_all().is_empty());
        assert_eq!(table.subscriber_count(), 0);
    }
}
