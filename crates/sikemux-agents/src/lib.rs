//! The agents the core runs, apart from running them: their catalog, accounts and sign-in,
//! how each is launched, the sessions they leave on disk, and what guards their git and
//! their actions that leave this machine.

pub mod acp;
pub mod agent_accounts;
#[rustfmt::skip]
pub mod agent_catalog;
pub mod agent_ops;
pub mod agent_shell;
pub mod callers;
pub mod git;
pub mod git_guard;
pub mod harness;
pub mod launch;
#[cfg(unix)]
#[path = "os/unix.rs"]
mod os;
#[cfg(windows)]
#[path = "os/windows.rs"]
mod os;
pub mod sessions;
pub mod ship;
pub mod skills;

use sikemux_base::http;
use sikemux_corekit::{accounts, android_sdk, cli, flights, tasks, text, timeout, trailers};
