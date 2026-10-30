//! One pool of build jobs for the whole machine: a GNU make jobserver that
//! cargo, rustc and the `cc` crate draw from, owned by whichever core locks it first.

use std::path::PathBuf;

const VARIABLE: &str = "CARGO_MAKEFLAGS";

fn pool() -> Option<PathBuf> {
    Some(crate::paths::config_dir()?.join("jobs"))
}

/// The variable that points a launched process at the pool, while the pool
/// exists and this process did not inherit a jobserver of its own.
pub fn variable() -> Option<(&'static str, String)> {
    let inherited = [VARIABLE, "MAKEFLAGS"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|flags| flags.contains("--jobserver")));
    if inherited || !cfg!(any(target_os = "macos", target_os = "linux")) {
        return None;
    }
    let pool = pool()?;
    let pool = pool.to_str()?;
    if pool.contains(char::is_whitespace) || !is_pool(pool) {
        return None;
    }
    Some((VARIABLE, format!("-j --jobserver-auth=fifo:{pool}")))
}

#[cfg(unix)]
fn is_pool(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.file_type().is_fifo())
}

#[cfg(not(unix))]
fn is_pool(_: &str) -> bool {
    false
}

/// Owns the pool from a thread of its own once no other process does.
pub fn own() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let _ = std::thread::Builder::new()
            .name("sikemux-jobs".into())
            .spawn(|| {
                if let Err(error) = owner::run() {
                    eprintln!("sikemux: the shared build pool is unavailable: {error}");
                }
            });
    }
}

/// Removes the pool if this process owns it, so builds started later do not
/// wait on a pool nobody fills.
pub fn leave() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    owner::leave();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod owner {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    use crate::memory_pressure::{self, Pressure};

    const TOKEN: u8 = b'+';
    const CHECK_EVERY: Duration = Duration::from_secs(30);
    const TAKE_BACK_EVERY: Duration = Duration::from_secs(1);

    static OWNED: AtomicBool = AtomicBool::new(false);

    pub(super) fn run() -> std::io::Result<()> {
        let path = super::pool().ok_or_else(|| std::io::Error::other("no home folder"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path.with_extension("lock"))?;
        // SAFETY: `lock` is an open file for as long as this call runs.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let _ = std::fs::remove_file(&path);
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
        // SAFETY: `name` is a NUL-terminated string that outlives the call.
        if unsafe { libc::mkfifo(name.as_ptr(), 0o600) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        OWNED.store(true, Ordering::Release);
        let writer = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)?;
        let reader = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)?;
        let tokens = std::thread::available_parallelism()
            .map_or(1, |cores| cores.get().saturating_sub(1).max(1));
        (&writer).write_all(&vec![TOKEN; tokens])?;
        let mut pool = Pool {
            path,
            reader,
            writer,
            tokens,
            held_back: 0,
            hold_back: 0,
        };
        let (tell, told) = mpsc::channel();
        memory_pressure::watch(move |pressure| {
            let _ = tell.send(pressure);
        });
        loop {
            let wait = if pool.held_back < pool.hold_back {
                TAKE_BACK_EVERY
            } else {
                CHECK_EVERY
            };
            match told.recv_timeout(wait) {
                Ok(pressure) => pool.press(pressure),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => std::thread::sleep(wait),
            }
            pool.settle();
        }
    }

    pub(super) fn leave() {
        if OWNED.load(Ordering::Acquire) {
            if let Some(pool) = super::pool() {
                let _ = std::fs::remove_file(pool);
            }
        }
    }

    struct Pool {
        path: PathBuf,
        reader: File,
        writer: File,
        tokens: usize,
        /// Jobs taken out of the pool while memory is short, so fewer
        /// compilers run at once.
        held_back: usize,
        hold_back: usize,
    }

    impl Pool {
        fn press(&mut self, pressure: Pressure) {
            let hold_back = match pressure {
                Pressure::Normal => 0,
                Pressure::Warn => self.tokens / 2,
                Pressure::Critical => self.tokens - 1,
            };
            if hold_back == self.hold_back {
                return;
            }
            self.hold_back = hold_back;
            eprintln!(
                "sikemux: memory pressure is {pressure:?}, builds share {} jobs",
                self.tokens - self.hold_back
            );
        }

        fn settle(&mut self) {
            let waiting = self.waiting();
            if self.held_back > self.hold_back {
                self.put(self.held_back - self.hold_back);
                self.held_back = self.hold_back;
            } else if self.held_back < self.hold_back {
                self.held_back += self.take(waiting.min(self.hold_back - self.held_back));
            } else if waiting + self.held_back > self.tokens {
                self.take(waiting + self.held_back - self.tokens);
            } else if self.hold_back == 0 {
                self.refill(self.tokens - waiting);
            }
        }

        /// Puts back the jobs a build took and never returned, as one killed
        /// mid-build does. A job is lost when more are out than the processes
        /// still holding the pool could account for.
        fn refill(&self, out: usize) {
            if out == 0 {
                return;
            }
            let Some(holders) = super::platform::holders(&self.path) else {
                return;
            };
            let accounted: usize = holders
                .into_iter()
                .filter(|pid| *pid != std::process::id())
                .map(held_by)
                .sum();
            self.put(out.saturating_sub(accounted));
        }

        fn put(&self, count: usize) {
            if count > 0 {
                let _ = (&self.writer).write(&vec![TOKEN; count]);
            }
        }

        fn take(&self, count: usize) -> usize {
            let mut taken = vec![0; count];
            (&self.reader).read(&mut taken).unwrap_or(0)
        }

        fn waiting(&self) -> usize {
            let mut count: libc::c_int = 0;
            // SAFETY: FIONREAD writes one int through the pointer, which is valid.
            let read = unsafe { libc::ioctl(self.reader.as_raw_fd(), libc::FIONREAD, &mut count) };
            if read == 0 {
                count.max(0) as usize
            } else {
                0
            }
        }
    }

    /// The most jobs `pid` can hold: one for each child it runs past the one
    /// it runs on its own job, or for a process with no children, one for
    /// each thread a compiler adds while it generates code.
    fn held_by(pid: u32) -> usize {
        const COMPILER_THREADS_AT_REST: usize = 3;
        match super::platform::children(pid) {
            0 => super::platform::threads(pid).saturating_sub(COMPILER_THREADS_AT_REST),
            children => children - 1,
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_int, c_void, CString};
    use std::path::Path;

    const PROC_ALL_PIDS: u32 = 1;
    const MOST_HOLDERS: usize = 4_096;
    const MOST_CHILDREN: usize = 1_024;

    extern "C" {
        fn proc_listpidspath(
            kind: u32,
            info: u32,
            path: *const c_char,
            flags: u32,
            buffer: *mut c_void,
            size: c_int,
        ) -> c_int;
    }

    /// The processes with the pool open.
    pub(super) fn holders(pool: &Path) -> Option<Vec<u32>> {
        let path = CString::new(pool.as_os_str().as_encoded_bytes()).ok()?;
        let mut pids = vec![0 as libc::pid_t; MOST_HOLDERS];
        // SAFETY: `pids` has room for the byte length passed, and `path` is
        // NUL-terminated and outlives the call.
        let bytes = unsafe {
            proc_listpidspath(
                PROC_ALL_PIDS,
                0,
                path.as_ptr(),
                0,
                pids.as_mut_ptr().cast(),
                (pids.len() * size_of::<libc::pid_t>()) as c_int,
            )
        };
        if bytes < 0 {
            return None;
        }
        pids.truncate(bytes as usize / size_of::<libc::pid_t>());
        Some(
            pids.into_iter()
                .filter(|pid| *pid > 0)
                .map(|pid| pid as u32)
                .collect(),
        )
    }

    pub(super) fn children(pid: u32) -> usize {
        let mut pids = vec![0 as libc::pid_t; MOST_CHILDREN];
        // SAFETY: `pids` has room for the byte length passed.
        let count = unsafe {
            libc::proc_listchildpids(
                pid as libc::pid_t,
                pids.as_mut_ptr().cast(),
                (pids.len() * size_of::<libc::pid_t>()) as c_int,
            )
        };
        count.max(0) as usize
    }

    pub(super) fn threads(pid: u32) -> usize {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
        let size = size_of::<libc::proc_taskinfo>() as c_int;
        // SAFETY: `info` is a writable `proc_taskinfo` of `size` bytes.
        let read = unsafe {
            libc::proc_pidinfo(
                pid as c_int,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if read != size {
            return 0;
        }
        // SAFETY: proc_pidinfo filled all `size` bytes.
        unsafe { info.assume_init() }.pti_threadnum.max(0) as usize
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::Path;

    /// The processes with the pool open.
    pub(super) fn holders(pool: &Path) -> Option<Vec<u32>> {
        let processes = std::fs::read_dir("/proc").ok()?;
        Some(
            processes
                .flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
                .filter(|pid| {
                    std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|fds| {
                        fds.flatten()
                            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|to| to == pool))
                    })
                })
                .collect(),
        )
    }

    pub(super) fn children(pid: u32) -> usize {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return 0;
        };
        tasks
            .flatten()
            .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
            .map(|children| children.split_whitespace().count())
            .sum()
    }

    pub(super) fn threads(pid: u32) -> usize {
        std::fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("Threads:"))
                    .and_then(|count| count.trim().parse().ok())
            })
            .unwrap_or(0)
    }
}
