//! Devices on an agent's desk, iOS simulators and Android emulators and phones: the pane that
//! picks and drives one, its device frame, the live screen it streams, and pairing a phone.

pub mod frame;
pub mod pair;
pub mod pane;
#[cfg(target_os = "macos")]
pub mod screen;
#[cfg(not(target_os = "macos"))]
#[path = "screen_elsewhere.rs"]
pub mod screen;
