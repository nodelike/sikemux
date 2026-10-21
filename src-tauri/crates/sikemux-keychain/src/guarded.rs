//! Secrets in `/var/lib/sikemux`, which only the group `sikemux` may enter.
//! Sikemux's installer makes the core setgid to that group, so the binary
//! starts holding it however it is run. Every run drops it at once except the
//! core's, which keeps it on one thread that does nothing but reach these
//! files, so nothing else the core does or starts, agents included, holds
//! the group. Being setgid also keeps other programs out of the core's memory
//! and the loader from taking libraries an agent names.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use sikemux_process::sync::Locked;

use crate::{KeychainError, KeychainResult};

const ROOT: &str = "/var/lib/sikemux";

type Job = Box<dyn FnOnce(&Path) + Send>;

static KEEPER: OnceLock<Option<Mutex<mpsc::Sender<Job>>>> = OnceLock::new();

/// Credentials belong to each thread, so this changes only the calling one;
/// libc's `setresgid` would change every thread at once.
fn drop_group_on_this_thread() {
    // SAFETY: getgid takes nothing and cannot fail.
    let gid = unsafe { libc::getgid() };
    // SAFETY: setresgid takes three plain ids and only changes this
    // thread's credentials.
    let dropped = unsafe { libc::syscall(libc::SYS_setresgid, gid, gid, gid) } == 0;
    if !dropped {
        eprintln!("sikemux: could not give up the group that guards its secrets");
        std::process::abort();
    }
}

pub(crate) fn settle(keep: bool) {
    KEEPER.get_or_init(|| {
        // SAFETY: getgid and getegid take nothing and cannot fail.
        if unsafe { libc::getegid() == libc::getgid() } {
            return None;
        }
        let keeper = keep.then(|| {
            // SAFETY: getuid takes nothing and cannot fail.
            let root = PathBuf::from(ROOT).join(unsafe { libc::getuid() }.to_string());
            let (send, jobs) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("sikemux-secrets".into())
                .spawn(move || jobs.iter().for_each(|job| job(&root)))
                .ok()
                .map(|_| Mutex::new(send))
        });
        drop_group_on_this_thread();
        keeper.flatten()
    });
}

pub(crate) fn active() -> bool {
    KEEPER.get().is_some_and(Option::is_some)
}

pub(crate) fn run<T: Send + 'static>(
    work: impl FnOnce(&Path) -> KeychainResult<T> + Send + 'static,
) -> KeychainResult<T> {
    let gone = || KeychainError::Failed("the core's secrets are out of reach".into());
    let keeper = KEEPER.get().and_then(Option::as_ref).ok_or_else(gone)?;
    let (answer, answered) = mpsc::channel();
    keeper
        .locked()
        .send(Box::new(move |root| {
            let _ = answer.send(work(root));
        }))
        .map_err(|_| gone())?;
    answered.recv().map_err(|_| gone())?
}
