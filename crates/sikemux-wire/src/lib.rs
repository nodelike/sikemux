//! What the phone's native code decodes: frames, requests and their answers, events, and joining
//! a host. The phone links this crate, so a change here is a change to its native code and needs
//! a store build. Whatever the phone only passes to TypeScript as JSON lives in `sikemux-view`.

pub mod accounts;
pub mod changed;
pub mod grid;
pub mod protocol;
pub mod pty;
