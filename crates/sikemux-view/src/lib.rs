//! What clients read from the core as JSON and draw: a chat's rows, the device view, goals, the
//! inbox, preferences. The phone reads these in TypeScript and never links this crate, so a
//! change here reaches it over the air. A type the phone's Rust acts on belongs in
//! `sikemux-wire` instead.

pub mod audit;
pub mod calls;
pub mod catalog;
pub mod changed;
pub mod chat;
pub mod device;
pub mod goals;
pub mod host_stats;
pub mod inbox;
pub mod layout;
pub mod memory;
pub mod notch;
pub mod owners;
pub mod playbook;
pub mod places;
pub mod plugins;
pub mod prefs;
pub mod push;
pub mod questions;
pub mod share;
pub mod shown;
pub mod slice;
pub mod start;
pub mod theme;
pub mod turn;
pub mod updates;
pub mod workspace;
