//! The core's local socket as each platform makes it: a Unix socket only its owner may open,
//! or a named pipe, and the signals that ask the core to stop.

#[cfg(unix)]
#[path = "socket/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "socket/windows.rs"]
mod platform;

#[cfg(test)]
pub(crate) use platform::not_listening;
pub(crate) use platform::{
    accepting, bind, listening, private_folder, private_to_owner, Bound, Stopping,
};
