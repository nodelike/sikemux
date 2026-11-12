//! The core itself, below everything that hangs its state on it: when it started, how to
//! reach it again, what it listens on, and the parts each feature keeps on it.

#![cfg(unix)]

use std::any::Any;
use std::sync::{Arc, Weak};
use std::time::Instant;

use serde_json::Value;
use sikemux_pty::error::PtyError;
use sikemux_wire::protocol::ErrorKind;
use tokio::sync::watch;

#[derive(Debug)]
pub struct CoreError {
    pub message: String,
    pub kind: Option<ErrorKind>,
    pub detail: Option<Value>,
    /// The network failed, not the other side, so asking again later may well work.
    pub unreachable: bool,
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl CoreError {
    pub fn pty(error: impl std::fmt::Display) -> Self {
        format!("pty: {error}").into()
    }

    /// A failure a client branches on.
    pub fn of(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: Some(kind),
            detail: None,
            unreachable: false,
        }
    }

    pub fn with_detail(self, detail: Value) -> Self {
        Self {
            detail: Some(detail),
            ..self
        }
    }

    /// The network failed on the way to `what`, with every cause it gave.
    pub fn unreachable(what: &str, error: &dyn std::error::Error) -> Self {
        let mut message = format!("{what}: {error}");
        let mut cause = error.source();
        while let Some(inner) = cause {
            message.push_str(": ");
            message.push_str(&inner.to_string());
            cause = inner.source();
        }
        Self {
            unreachable: true,
            ..Self::from(message)
        }
    }

    pub fn is_unreachable(&self) -> bool {
        self.unreachable
    }

    pub fn unknown_method(method: &str) -> Self {
        Self::of(
            ErrorKind::UnknownMethod,
            format!("this host does not know {method}"),
        )
    }
}

impl From<&str> for CoreError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<String> for CoreError {
    fn from(message: String) -> Self {
        Self {
            message,
            kind: None,
            detail: None,
            unreachable: false,
        }
    }
}

impl From<sikemux_git::files::Error> for CoreError {
    fn from(error: sikemux_git::files::Error) -> Self {
        Self {
            message: error.message,
            kind: error.kind,
            detail: None,
            unreachable: false,
        }
    }
}

impl From<PtyError> for CoreError {
    fn from(error: PtyError) -> Self {
        error.to_string().into()
    }
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        format!("io: {error}").into()
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        format!("json: {error}").into()
    }
}

pub type CoreResult<T> = Result<T, CoreError>;

/// What this core listens on, which a hand-over passes to its replacement.
pub struct Listening {
    pub listener_fd: std::os::fd::RawFd,
    pub lock_fd: std::os::fd::RawFd,
}

/// What the features keep on a core, as the crate that builds the core made it.
pub type Parts = Box<dyn Any + Send + Sync>;

pub struct Core {
    me: Weak<Core>,
    /// When this core's clock reads zero, which session times count from.
    started: Instant,
    shutdown: watch::Sender<bool>,
    pub listening: Listening,
    parts: Parts,
}

impl Core {
    /// A core whose clock read zero at `started`, keeping the parts `parts` makes; `parts` is
    /// handed the core it will be part of, for parts that reach back to it.
    pub fn build(
        started: Instant,
        listening: Listening,
        parts: impl FnOnce(&Weak<Core>) -> Parts,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            started,
            shutdown: watch::channel(false).0,
            listening,
            parts: parts(me),
        })
    }

    /// The parts this core was built with, which are a `T`.
    pub fn kept<T: 'static>(&self) -> &T {
        self.parts
            .downcast_ref()
            .expect("a core is read as the parts it was built with")
    }

    /// This core, for work that outlives the call it starts in.
    pub fn arc(&self) -> Option<Arc<Core>> {
        self.me.upgrade()
    }

    /// Milliseconds on this core's clock, which session times are stamped in.
    pub fn uptime_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// This core, for work that may outlive it.
    pub fn weak(&self) -> Weak<Core> {
        self.me.clone()
    }

    /// Says true once, when the core starts to stop.
    pub fn shutdown(&self) -> &watch::Sender<bool> {
        &self.shutdown
    }
}
