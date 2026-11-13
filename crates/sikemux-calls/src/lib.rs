//! The core's methods a client calls by name, as types: each method's marker, what it takes
//! and what it answers. A client in Rust calls them typed without linking the core, which
//! keeps only what each method runs.

#![cfg(unix)]

/// Declares methods as `"method" Marker (Params) -> Answer;`.
macro_rules! calls {
    ($($name:literal $marker:ident ($takes:ty) -> $gives:ty;)*) => {
        $(
            pub struct $marker;

            impl $crate::Call for $marker {
                const METHOD: &'static str = $name;
                type Params = $takes;
                type Answer = $gives;
            }
        )*
    };
}
pub(crate) use calls;

pub mod about;
pub mod account;
pub mod activity_log;
pub mod agent_accounts;
pub mod agent_sign_in;
pub mod agent_updates;
pub mod agent_view;
pub mod attachments;
pub mod board;
pub mod call;
pub mod chat;
pub mod chief;
pub mod cli_install;
pub mod custom_command;
pub mod diagnosis;
pub mod diagrams;
pub mod drafts;
pub mod edits;
pub mod faces;
pub mod git;
pub mod harness;
pub mod host_stats;
pub mod hosts;
pub mod inbox;
pub mod landing;
pub mod launching;
pub mod layout;
pub mod lsp;
pub mod mcp_apps;
pub mod memory;
pub mod messages;
pub mod model_providers;
pub mod mods;
pub mod move_agent;
pub mod names;
pub mod new_project;
pub mod notify;
pub mod open_files;
pub mod outside;
pub mod params;
pub mod pictures;
pub mod places;
pub mod playbook;
pub mod plugins;
pub mod ports;
pub mod preferences;
pub mod project_files;
pub mod project_roots;
pub mod questions;
pub mod remote;
pub mod remote_image;
pub mod saved;
pub mod saved_sessions;
pub mod search;
pub mod seen;
pub mod session;
pub mod settling;
pub mod setup;
pub mod share;
pub mod ship;
pub mod skill_library;
pub mod spare;
pub mod ssh;
pub mod stash;
pub mod streams;
pub mod tasks;
pub mod themes;
pub mod triage;
pub mod triggers;
pub mod turn_state;
pub mod updates;
pub mod usage;
pub mod vault;
pub mod wallpaper;
pub mod workspace;
pub mod worktree;

/// One method of the core.
pub trait Call {
    const METHOD: &'static str;
    type Params;
    type Answer;
}
