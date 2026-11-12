//! The agents the core runs, apart from running them: their catalog, accounts and sign-in,
//! how each is launched, the sessions they leave on disk, and what guards their git and
//! their actions that leave this machine.

pub mod acp;
#[cfg(unix)]
pub mod agent_accounts;
#[rustfmt::skip]
pub mod agent_catalog;
pub mod agent_ops;
#[cfg(unix)]
pub mod agent_shell;
#[cfg(unix)]
pub mod callers;
#[cfg(unix)]
pub mod git;
#[cfg(unix)]
pub mod git_guard;
pub mod harness;
#[cfg(unix)]
pub mod launch;
#[cfg(unix)]
pub mod sessions;
pub mod ship;
pub mod skills;

#[cfg(unix)]
use sikemux_base::http;
#[cfg(unix)]
use sikemux_corekit::{accounts, android_sdk, cli, flights, tasks, text, timeout, trailers};
