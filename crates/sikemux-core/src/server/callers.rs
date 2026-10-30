//! Which agent made a tool call, told from the process tree rather than from
//! anything the caller says. The core starts every agent, so it records the
//! process it started for each. A call comes from an agent when the process
//! holding the connection descends from that agent's process; one that has
//! left the tree, such as a daemon re-parented to launchd or init, is no agent.
//!
//! An agent's process is a child of this process until it is collected, and
//! an update runs the new core in this same process, so its pid cannot be
//! taken by another process while it is recorded. The start time recorded
//! beside it still guards the moment between collecting it and forgetting it.
//!
//! A `git commit` the core runs itself is no agent, but its hooks may ask once
//! for the commit's footer while it runs.
//!
//! A Sikemux started from an agent's shell, such as a build of it being worked
//! on, runs agents of its own, and their calls are not the outer agent's.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use sikemux_process::sync::Locked;

/// Processes that made calls lately, looked at first: an agent's tool server
/// makes every call it sends from the one process.
const RECENT_CALLERS: usize = 32;
/// Deeper than any agent's tree, short of a loop in a table read mid-change.
const MAX_DEPTH: usize = 64;
/// The names Sikemux's app and core run under.
const SIKEMUX_PROCESSES: [&str; 2] = ["sikemux", "sikemux-host"];

struct Root {
    started: u64,
    agent_id: String,
}

struct Grant {
    started: u64,
    commit: CommitGrant,
    until: Instant,
}

/// Whom a commit the core runs names: `agents`, whose lines were staged as it
/// began, else `fallback`.
pub(crate) struct CommitGrant {
    pub agents: Vec<String>,
    pub fallback: Option<String>,
}

#[derive(Default)]
struct Known {
    roots: HashMap<u32, Root>,
    recent: VecDeque<(u32, u64)>,
    commits: HashMap<u32, Grant>,
}

static KNOWN: LazyLock<Mutex<Known>> = LazyLock::new(Mutex::default);

fn known() -> std::sync::MutexGuard<'static, Known> {
    KNOWN.locked()
}

/// Records `pid`, a child of this process not yet collected, as `agent_id`'s.
pub(crate) fn started(pid: u32, agent_id: &str) {
    if agent_id.is_empty() {
        return;
    }
    let Some(started) = platform::started(pid) else {
        return;
    };
    let mut known = known();
    known
        .roots
        .retain(|pid, root| platform::started(*pid) == Some(root.started));
    known.roots.insert(
        pid,
        Root {
            started,
            agent_id: agent_id.to_owned(),
        },
    );
}

/// When `pid` started, which tells it apart from a later process given the
/// same pid.
pub(crate) fn start_time(pid: u32) -> Option<u64> {
    platform::started(pid)
}

/// Lets the `git commit` at `pid`, a child of this process not yet collected,
/// ask for its footer once within `lasting`.
pub(crate) fn grant_commit(
    pid: u32,
    agents: Vec<String>,
    fallback: Option<&str>,
    lasting: Duration,
) {
    let Some(started) = platform::started(pid) else {
        return;
    };
    let mut known = known();
    let now = Instant::now();
    known.commits.retain(|_, grant| grant.until > now);
    known.commits.insert(
        pid,
        Grant {
            started,
            commit: CommitGrant {
                agents,
                fallback: fallback.map(str::to_owned),
            },
            until: now + lasting,
        },
    );
}

/// The commit at `pid` ended; whatever it did not ask for goes.
pub(crate) fn end_commit(pid: u32) {
    known().commits.remove(&pid);
}

/// The grant of the commit whose processes hold the loopback connection from
/// `peer` to this core's `port`, used up by this ask. Blocks while it reads
/// the process table.
pub(crate) fn take_commit_grant(peer: SocketAddr, port: u16) -> Option<CommitGrant> {
    if !peer.ip().is_loopback() {
        return None;
    }
    let live: Vec<(u32, u64)> = {
        let mut known = known();
        let now = Instant::now();
        known.commits.retain(|pid, grant| {
            grant.until > now && platform::started(*pid) == Some(grant.started)
        });
        known
            .commits
            .iter()
            .map(|(pid, grant)| (*pid, grant.started))
            .collect()
    };
    if live.is_empty() {
        return None;
    }
    let socket = platform::Socket::find(peer.port(), port)?;
    let (_, commit) = descendants(live)
        .into_iter()
        .find(|(pid, _)| socket.held_by(*pid))?;
    known().commits.remove(&commit).map(|grant| grant.commit)
}

/// The agent whose process holds the client end of the loopback connection
/// from `peer` to this core's `port`. Blocks while it reads the process table.
pub(crate) fn identify(peer: SocketAddr, port: u16) -> Option<String> {
    if !peer.ip().is_loopback() {
        return None;
    }
    let socket = platform::Socket::find(peer.port(), port)?;
    let roots: HashMap<u32, (u64, String)> = {
        let mut known = known();
        known
            .roots
            .retain(|pid, root| platform::started(*pid) == Some(root.started));
        known
            .roots
            .iter()
            .map(|(pid, root)| (*pid, (root.started, root.agent_id.clone())))
            .collect()
    };
    if roots.is_empty() {
        return None;
    }
    let recent: Vec<(u32, u64)> = known().recent.iter().copied().collect();
    let holder = recent
        .into_iter()
        .find(|(pid, started)| platform::started(*pid) == Some(*started) && socket.held_by(*pid))
        .map(|(pid, _)| pid)
        .or_else(|| {
            descendants(roots.iter().map(|(pid, (started, _))| (*pid, *started)))
                .into_iter()
                .map(|(pid, _)| pid)
                .find(|pid| socket.held_by(*pid))
        })?;
    let agent = agent_above(holder, &roots);
    if agent.is_some() {
        remember(holder);
    }
    agent
}

/// The agent whose process tree `pid` belongs to, if any. Blocks while it
/// reads the process table.
pub(crate) fn agent_of(pid: u32) -> Option<String> {
    let roots: HashMap<u32, (u64, String)> = {
        let mut known = known();
        known
            .roots
            .retain(|pid, root| platform::started(*pid) == Some(root.started));
        known
            .roots
            .iter()
            .map(|(pid, root)| (*pid, (root.started, root.agent_id.clone())))
            .collect()
    };
    agent_above(pid, &roots)
}

fn remember(pid: u32) {
    let Some(started) = platform::started(pid) else {
        return;
    };
    let mut known = known();
    known.recent.retain(|(recent, _)| *recent != pid);
    if known.recent.len() == RECENT_CALLERS {
        known.recent.pop_back();
    }
    known.recent.push_front((pid, started));
}

/// The nearest agent process at or above `pid`, short of a Sikemux between
/// them. `pid` itself may be one, as an agent's tool server is.
fn agent_above(mut pid: u32, roots: &HashMap<u32, (u64, String)>) -> Option<String> {
    let caller = pid;
    for _ in 0..MAX_DEPTH {
        let (parent, started) = platform::parent(pid)?;
        if let Some((root_started, agent_id)) = roots.get(&pid) {
            return (*root_started == started).then(|| agent_id.clone());
        }
        if pid != caller
            && platform::name(pid).is_some_and(|name| SIKEMUX_PROCESSES.contains(&name.as_str()))
        {
            return None;
        }
        if parent <= 1 {
            return None;
        }
        pid = parent;
    }
    None
}

/// Every process at or below each of `roots`, given with its start time,
/// nearest first, each with the root it is under.
fn descendants(roots: impl IntoIterator<Item = (u32, u64)>) -> Vec<(u32, u32)> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut started = HashMap::new();
    for (pid, parent, start) in platform::processes() {
        children.entry(parent).or_default().push(pid);
        started.insert(pid, start);
    }
    let mut queue: VecDeque<(u32, u32)> = roots
        .into_iter()
        .filter(|(pid, root_started)| started.get(pid) == Some(root_started))
        .map(|(pid, _)| (pid, pid))
        .collect();
    let mut seen: HashSet<u32> = queue.iter().map(|(pid, _)| *pid).collect();
    let mut found = Vec::new();
    while let Some((pid, root)) = queue.pop_front() {
        found.push((pid, root));
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(*child) {
                queue.push_back((*child, root));
            }
        }
    }
    found
}

#[cfg(target_os = "macos")]
mod platform {
    use std::mem::{size_of, MaybeUninit};

    const PROC_PIDFDSOCKETINFO: libc::c_int = 3;
    const SOCKINFO_TCP: i32 = 2;
    /// `struct socket_fdinfo` from `<sys/proc_info.h>`, read by offset.
    const SOCKET_FDINFO: usize = 792;
    const SOI_KIND: usize = 256;
    const INSI_FPORT: usize = 264;
    const INSI_LPORT: usize = 268;

    #[repr(C, align(8))]
    struct SocketFdInfo([u8; SOCKET_FDINFO]);

    fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
        let pid = libc::c_int::try_from(pid).ok()?;
        let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `info` is a writable `proc_bsdinfo` of `size` bytes, and
        // the kernel writes at most that many.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        // SAFETY: the kernel filled the whole struct when it wrote `size` bytes.
        (written == size).then(|| unsafe { info.assume_init() })
    }

    fn start_of(info: &libc::proc_bsdinfo) -> u64 {
        info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec
    }

    pub(super) fn started(pid: u32) -> Option<u64> {
        bsd_info(pid).map(|info| start_of(&info))
    }

    pub(super) fn parent(pid: u32) -> Option<(u32, u64)> {
        bsd_info(pid).map(|info| (info.pbi_ppid, start_of(&info)))
    }

    pub(super) fn name(pid: u32) -> Option<String> {
        let info = bsd_info(pid)?;
        let bytes: Vec<u8> = info
            .pbi_comm
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        String::from_utf8(bytes).ok()
    }

    pub(super) fn processes() -> Vec<(u32, u32, u64)> {
        // SAFETY: a null buffer asks only for how many pids there are.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let Ok(count) = usize::try_from(count) else {
            return Vec::new();
        };
        let mut pids = vec![0 as libc::pid_t; count + 64];
        let size = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `pids` is a writable buffer of `size` bytes.
        let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
        let listed = usize::try_from(listed).unwrap_or(0).min(pids.len());
        pids[..listed]
            .iter()
            .filter_map(|pid| {
                let pid = u32::try_from(*pid).ok()?;
                let info = bsd_info(pid)?;
                Some((pid, info.pbi_ppid, start_of(&info)))
            })
            .collect()
    }

    pub(super) struct Socket {
        local: u16,
        remote: u16,
    }

    impl Socket {
        pub(super) fn find(local: u16, remote: u16) -> Option<Self> {
            Some(Self { local, remote })
        }

        pub(super) fn held_by(&self, pid: u32) -> bool {
            let Ok(pid) = libc::c_int::try_from(pid) else {
                return false;
            };
            fds(pid)
                .into_iter()
                .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
                .any(|fd| self.is(pid, fd.proc_fd))
        }

        fn is(&self, pid: libc::c_int, fd: i32) -> bool {
            let mut info = SocketFdInfo([0; SOCKET_FDINFO]);
            // SAFETY: `info` is a writable buffer of `SOCKET_FDINFO` bytes,
            // the size of `socket_fdinfo`, and the kernel writes at most that.
            let written = unsafe {
                libc::proc_pidfdinfo(
                    pid,
                    fd,
                    PROC_PIDFDSOCKETINFO,
                    info.0.as_mut_ptr().cast(),
                    SOCKET_FDINFO as libc::c_int,
                )
            };
            if written != SOCKET_FDINFO as libc::c_int {
                return false;
            }
            let word = |at: usize| {
                i32::from_ne_bytes([info.0[at], info.0[at + 1], info.0[at + 2], info.0[at + 3]])
            };
            let port = |at: usize| u16::from_be_bytes([info.0[at], info.0[at + 1]]);
            word(SOI_KIND) == SOCKINFO_TCP
                && port(INSI_LPORT) == self.local
                && port(INSI_FPORT) == self.remote
        }
    }

    fn fds(pid: libc::c_int) -> Vec<libc::proc_fdinfo> {
        // SAFETY: a null buffer asks only for how many bytes the list needs.
        let needed =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        let Ok(needed) = usize::try_from(needed) else {
            return Vec::new();
        };
        let mut fds = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0
            };
            needed / size_of::<libc::proc_fdinfo>() + 16
        ];
        let size = (fds.len() * size_of::<libc::proc_fdinfo>()) as libc::c_int;
        // SAFETY: `fds` is a writable buffer of `size` bytes.
        let written = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), size)
        };
        let count = usize::try_from(written).unwrap_or(0) / size_of::<libc::proc_fdinfo>();
        fds.truncate(count);
        fds
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;

    /// `pid`'s parent and start time, from `/proc/<pid>/stat`.
    fn stat(pid: u32) -> Option<(u32, u64)> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let mut fields = stat.get(stat.rfind(')')? + 2..)?.split(' ');
        let parent = fields.nth(1)?.parse().ok()?;
        let started = fields.nth(17)?.parse().ok()?;
        Some((parent, started))
    }

    pub(super) fn started(pid: u32) -> Option<u64> {
        stat(pid).map(|(_, started)| started)
    }

    pub(super) fn parent(pid: u32) -> Option<(u32, u64)> {
        stat(pid)
    }

    pub(super) fn name(pid: u32) -> Option<String> {
        fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|name| name.trim_end().to_owned())
    }

    pub(super) fn processes() -> Vec<(u32, u32, u64)> {
        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| {
                let pid = entry.ok()?.file_name().to_str()?.parse().ok()?;
                let (parent, started) = stat(pid)?;
                Some((pid, parent, started))
            })
            .collect()
    }

    pub(super) struct Socket {
        link: String,
    }

    impl Socket {
        /// The client end's inode, from the kernel's table of TCP sockets.
        pub(super) fn find(local: u16, remote: u16) -> Option<Self> {
            ["/proc/net/tcp", "/proc/net/tcp6"]
                .into_iter()
                .filter_map(|table| fs::read_to_string(table).ok())
                .find_map(|table| {
                    table.lines().skip(1).find_map(|line| {
                        let fields: Vec<&str> = line.split_whitespace().collect();
                        let port = |address: &str| {
                            u16::from_str_radix(address.rsplit(':').next()?, 16).ok()
                        };
                        (port(fields.get(1)?)? == local && port(fields.get(2)?)? == remote)
                            .then(|| fields.get(9).map(|inode| (*inode).to_owned()))
                            .flatten()
                    })
                })
                .filter(|inode| inode != "0")
                .map(|inode| Self {
                    link: format!("socket:[{inode}]"),
                })
        }

        pub(super) fn held_by(&self, pid: u32) -> bool {
            let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else {
                return false;
            };
            fds.filter_map(Result::ok).any(|fd| {
                fs::read_link(fd.path())
                    .is_ok_and(|target| target.as_os_str() == self.link.as_str())
            })
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    pub(super) fn started(_pid: u32) -> Option<u64> {
        None
    }

    pub(super) fn parent(_pid: u32) -> Option<(u32, u64)> {
        None
    }

    pub(super) fn name(_pid: u32) -> Option<String> {
        None
    }

    pub(super) fn processes() -> Vec<(u32, u32, u64)> {
        Vec::new()
    }

    pub(super) struct Socket;

    impl Socket {
        pub(super) fn find(_local: u16, _remote: u16) -> Option<Self> {
            None
        }

        pub(super) fn held_by(&self, _pid: u32) -> bool {
            false
        }
    }
}
