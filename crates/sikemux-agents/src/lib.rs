//! The agents the core runs, apart from running them: their catalog, accounts and sign-in,
//! how each is launched, the sessions they leave on disk, and what guards their git and
//! their actions that leave this machine.

pub mod acp;
pub mod agent_accounts;
pub mod agent_catalog;
pub mod agent_ops;
pub mod agent_shell;
pub mod callers;
pub mod custom_agents;
pub mod git;
pub mod git_guard;
pub mod harness;
pub mod launch;
#[cfg(unix)]
#[path = "os/unix.rs"]
pub mod os;
#[cfg(windows)]
#[path = "os/windows.rs"]
pub mod os;
pub mod sessions;
pub mod ship;
pub mod skills;
#[cfg(windows)]
pub mod tcp_table;

use sikemux_base::http;
use sikemux_corekit::{accounts, android_sdk, cli, flights, tasks, text, timeout, trailers};

/// Caps Rayon's global thread pool so the core does not keep one unnamed worker thread per CPU core.
pub fn bound_rayon_pool() {
    let threads = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4)
        .clamp(2, 4);
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|index| format!("sikemux-rayon-{index}"))
        .build_global();
}
