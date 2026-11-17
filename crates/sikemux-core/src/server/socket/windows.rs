use std::fs::{File, OpenOptions};
use std::path::Path;

use sikemux_client::local::Listener;
use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_shutdown};
use tokio::signal::windows::{CtrlBreak, CtrlC, CtrlClose, CtrlShutdown};

use super::super::Listening;

/// A named pipe, which admits only the signed-in user.
pub(crate) type Bound = Listener;

pub(crate) fn private_folder(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn private_to_owner(options: &mut OpenOptions) -> &mut OpenOptions {
    options
}

pub(crate) fn bind(socket: &Path) -> std::io::Result<Bound> {
    Listener::bind(socket)
}

pub(crate) fn accepting(bound: Bound) -> std::io::Result<Listener> {
    Ok(bound)
}

pub(crate) fn listening(_bound: &Bound, _lock: &File) -> Listening {
    Listening
}

#[cfg(test)]
pub(crate) fn not_listening() -> Listening {
    Listening
}

pub(crate) struct Stopping {
    ctrl_c: CtrlC,
    ctrl_break: CtrlBreak,
    close: CtrlClose,
    shutdown: CtrlShutdown,
}

impl Stopping {
    pub(crate) fn new() -> std::io::Result<Self> {
        Ok(Self {
            ctrl_c: ctrl_c()?,
            ctrl_break: ctrl_break()?,
            close: ctrl_close()?,
            shutdown: ctrl_shutdown()?,
        })
    }

    /// What asked the core to stop, once something has.
    pub(crate) async fn asked(&mut self) -> &'static str {
        tokio::select! {
            _ = self.ctrl_c.recv() => "Ctrl-C",
            _ = self.ctrl_break.recv() => "Ctrl-Break",
            _ = self.close.recv() => "its console closing",
            _ = self.shutdown.recv() => "Windows shutting down",
        }
    }
}
