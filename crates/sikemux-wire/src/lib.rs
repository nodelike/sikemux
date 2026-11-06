//! What the core and its clients send each other: frames, requests and their answers, events, and
//! joining a host. The phone links this crate, so a change here is a change to its native code and
//! needs a store build. Whatever it only passes to TypeScript as JSON lives in `sikemux-view`, and
//! a terminal's frames in `sikemux-grid`. The phone's Rust acts on only part of what is left: the
//! Mac's requests and events decode typed here too, since moving those would not earn its cost.

pub mod accounts;
pub mod protocol;
pub mod pty;
pub mod terminal;
