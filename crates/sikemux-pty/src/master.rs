//! The core's side of a terminal: what the session reads its program's output from, writes
//! typing to and resizes.

#[cfg(unix)]
#[path = "master/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "master/windows.rs"]
mod platform;

pub use platform::Master;

pub enum Read {
    Bytes(usize),
    Eof,
}
