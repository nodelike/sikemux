//! Answers that arrive in parts, such as search results, sent as `Stream`
//! events on the connection that asked. The client names each stream with a
//! number of its own, so it is ready for the parts before the call answers.
//! Parts go in batches, and a producer waits while its client is behind
//! rather than anything piling up or being dropped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use sikemux_process::sync::Locked;
use tokio::runtime::RuntimeFlavor;

use super::connection::{ClientConn, ClientId};
use super::{CoreError, CoreResult};
use crate::protocol::{Event, ServerMessage};

const BATCH_ITEMS: usize = 200;
const BATCH_AGE: Duration = Duration::from_millis(16);
/// How far behind a client may fall before a producer waits for it.
const SLOW_BYTES: usize = 512 * 1024;
const MAX_OPEN_PER_CLIENT: usize = 16;

#[derive(Default)]
pub(crate) struct Streams {
    open: Mutex<HashMap<(ClientId, u64), Arc<AtomicBool>>>,
}

impl Streams {
    /// Opens `stream` on `client`'s connection.
    pub(crate) fn open(&self, client: Arc<ClientConn>, stream: u64) -> CoreResult<Arc<Stream>> {
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut open = self.open.locked();
            open.retain(|_, stop| !stop.load(Ordering::Acquire));
            if open.contains_key(&(client.id, stream)) {
                return Err(CoreError::from("that stream is already open"));
            }
            if open.keys().filter(|(id, _)| *id == client.id).count() >= MAX_OPEN_PER_CLIENT {
                return Err(CoreError::from("too many streams are open at once"));
            }
            open.insert((client.id, stream), stop.clone());
        }
        let stream = Arc::new(Stream {
            client,
            id: stream,
            stop,
            batch: Mutex::new(Vec::new()),
        });
        tokio::spawn(flush_while_open(stream.clone()));
        Ok(stream)
    }

    /// Stops `stream` on `client`'s connection, or every one of its streams
    /// when none is named; their producers end them.
    pub(crate) fn cancel(&self, client: ClientId, stream: Option<u64>) {
        self.open.locked().retain(|(owner, id), stop| {
            let cancelled = *owner == client && stream.is_none_or(|stream| stream == *id);
            if cancelled {
                stop.store(true, Ordering::Release);
            }
            !cancelled
        });
    }
}

pub(crate) struct Stream {
    client: Arc<ClientConn>,
    id: u64,
    stop: Arc<AtomicBool>,
    batch: Mutex<Vec<Value>>,
}

impl Stream {
    /// Whether the client cancelled the stream or went away.
    pub(crate) fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire) || self.client.is_closed()
    }

    /// Adds a part, sending the batch once it is full, and waits while the
    /// client is behind. Answers whether to go on.
    pub(crate) fn push(&self, item: Value) -> bool {
        {
            let mut batch = self.batch.locked();
            batch.push(item);
            if batch.len() >= BATCH_ITEMS {
                self.send(std::mem::take(&mut *batch), false, None);
            }
        }
        self.wait_for_client();
        !self.stopped()
    }

    /// Waits while the client is behind, without holding up the runtime's
    /// other tasks when the producer runs on one of its threads.
    fn wait_for_client(&self) {
        let wait = || {
            while !self.stopped() && self.client.backlog() > SLOW_BYTES {
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) if runtime.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(wait)
            }
            _ => wait(),
        }
    }

    /// Sends what is left, with `last` when there is one, and ends the stream.
    pub(crate) fn finish(&self, last: Option<Value>) {
        self.end(last, None);
    }

    /// Sends what is left and ends the stream with `error`.
    pub(crate) fn fail(&self, error: Value) {
        self.end(None, Some(error));
    }

    /// Sends under the batch's lock, so no batch goes out after the last.
    fn end(&self, last: Option<Value>, error: Option<Value>) {
        let mut batch = self.batch.locked();
        let mut items = std::mem::take(&mut *batch);
        items.extend(last);
        self.stop.store(true, Ordering::Release);
        self.send(items, true, error);
    }

    /// Sends what has waited, unless the stream already ended.
    fn flush(&self) {
        let mut batch = self.batch.locked();
        if !self.stopped() {
            self.send(std::mem::take(&mut *batch), false, None);
        }
    }

    fn send(&self, items: Vec<Value>, done: bool, error: Option<Value>) {
        if items.is_empty() && !done {
            return;
        }
        self.client.send_message(&ServerMessage::Event {
            event: Event::Stream {
                stream: self.id,
                items,
                done,
                error,
            },
        });
    }
}

/// Sends a batch that has waited a frame, so a slow producer's parts still
/// arrive promptly.
async fn flush_while_open(stream: Arc<Stream>) {
    let mut started = Instant::now();
    loop {
        tokio::time::sleep_until((started + BATCH_AGE).into()).await;
        started = Instant::now();
        if stream.stopped() {
            return;
        }
        if stream.client.backlog() <= SLOW_BYTES {
            stream.flush();
        }
    }
}
