//! iOS simulators on an agent's desk: the pane that picks and drives one, its device frame, and
//! the live screen it streams.

pub mod frame;
pub mod pane;
#[cfg(target_os = "macos")]
pub mod screen;
#[cfg(not(target_os = "macos"))]
#[path = "screen_elsewhere.rs"]
pub mod screen;
