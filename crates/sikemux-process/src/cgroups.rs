//! Gives each agent, task and terminal a cgroup of its own inside the core's systemd unit. When
//! memory runs out, systemd-oomd stops the cgroup using the most, so a runaway build then takes
//! down only what started it, not the app, the core and every other agent with it.

use std::process::Command;

/// Puts the core in a systemd unit it may divide, and itself in that unit's `core` cgroup.
/// Call once at startup, before anything is spawned. Does nothing outside a systemd user session.
pub fn take_own_unit() {
    platform::take_own_unit();
}

/// Starts `command`'s process in a cgroup of its own.
pub fn give_own_cgroup(command: &mut Command) {
    platform::give_own_cgroup(command);
}

/// Moves a process that was just started into a cgroup of its own, for spawns that offer no way
/// to run code before `exec`.
pub fn move_into_own_cgroup(pid: u32) {
    platform::move_into_own_cgroup(pid);
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::{CStr, CString};
    use std::fs;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use crate::sync::Locked;

    const CGROUPS: &str = "/sys/fs/cgroup";
    const OWN_SCOPE_PREFIX: &str = "sikemux-core-";
    const CORE_LEAF: &str = "core";
    const CHILD_LEAF_PREFIX: &str = "child-";
    const CONTROLLER: &str = "memory";
    /// A child cgroup still empty this long after it was made had its process exit, or was never used.
    const UNUSED_AFTER: Duration = Duration::from_secs(60);

    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    static NEXT_CHILD: AtomicU64 = AtomicU64::new(0);
    static MADE: Mutex<Vec<(PathBuf, Instant)>> = Mutex::new(Vec::new());

    pub(super) fn take_own_unit() {
        ROOT.get_or_init(|| {
            let root = divisible_unit()?;
            settle_in(&root).then_some(root)
        });
    }

    pub(super) fn give_own_cgroup(command: &mut Command) {
        let Some(cgroup) = new_child_cgroup() else {
            return;
        };
        let procs = cgroup.join("cgroup.procs");
        let (Ok(cgroup), Ok(procs)) = (
            CString::new(cgroup.into_os_string().into_vec()),
            CString::new(procs.into_os_string().into_vec()),
        ) else {
            return;
        };
        // SAFETY: the hook runs in the forked child before exec and calls only mkdir, open, write
        // and close, which are async-signal-safe; both paths were built before the fork.
        unsafe {
            command.pre_exec(move || {
                join(&cgroup, &procs);
                Ok(())
            });
        }
    }

    pub(super) fn move_into_own_cgroup(pid: u32) {
        if let Some(cgroup) = new_child_cgroup() {
            let _ = fs::write(cgroup.join("cgroup.procs"), pid.to_string());
        }
    }

    /// Writing 0 to a cgroup's `cgroup.procs` moves the writing process into it. The cgroup is
    /// made again first in case it was cleared as unused before the command was spawned.
    fn join(cgroup: &CStr, procs: &CStr) {
        // SAFETY: both are valid NUL-terminated paths, and the fd opened here is closed here.
        unsafe {
            libc::mkdir(cgroup.as_ptr(), 0o755);
            let fd = libc::open(procs.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
            if fd >= 0 {
                libc::write(fd, b"0".as_ptr().cast(), 1);
                libc::close(fd);
            }
        }
    }

    /// Makes an empty cgroup for one process.
    fn new_child_cgroup() -> Option<PathBuf> {
        let root = ROOT.get()?.as_ref()?;
        let now = Instant::now();
        let mut made = MADE.locked();
        made.retain(|(path, at)| {
            now.duration_since(*at) < UNUSED_AFTER
                || fs::remove_dir(path)
                    .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
        });
        let number = NEXT_CHILD.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            "{CHILD_LEAF_PREFIX}{}-{number}",
            std::process::id()
        ));
        fs::create_dir(&path).ok()?;
        made.push((path.clone(), now));
        Some(path)
    }

    /// The cgroup of a unit this core may make cgroups in: its own delegated service, or a scope
    /// it starts for itself when something else, such as the desktop or another core's agent,
    /// launched it.
    fn divisible_unit() -> Option<PathBuf> {
        let own = own_cgroup()?;
        let components: Vec<&str> = own.split('/').filter(|part| !part.is_empty()).collect();
        let manager = components
            .iter()
            .position(|part| part.starts_with("user@"))?;
        let unit = components
            .iter()
            .rposition(|part| part.ends_with(".scope") || part.ends_with(".service"))
            .filter(|&at| at > manager)?;
        let name = components[unit];
        let inside = &components[unit + 1..];
        let at_top = inside.is_empty() || inside == [CORE_LEAF];
        let unit_path = Path::new(CGROUPS).join(components[..=unit].join("/"));
        if at_top && name.starts_with(&own_scope_prefix()) && name.ends_with(".scope") {
            return Some(unit_path);
        }
        if at_top && name.ends_with(".service") && service_is_delegated() {
            return Some(unit_path);
        }
        let slice = components[..unit]
            .iter()
            .rev()
            .find(|part| part.ends_with(".slice"))
            .copied();
        start_own_scope(slice)
    }

    fn own_scope_prefix() -> String {
        format!("{OWN_SCOPE_PREFIX}{}-", std::process::id())
    }

    fn own_cgroup() -> Option<String> {
        let line = fs::read_to_string("/proc/self/cgroup").ok()?;
        Some(
            line.lines()
                .find_map(|line| line.strip_prefix("0::"))?
                .to_owned(),
        )
    }

    fn busctl(args: &[&str]) -> Option<String> {
        let output = crate::user_environment::command("busctl")
            .arg("--user")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn service_is_delegated() -> bool {
        delegate_property().is_some_and(|delegate| delegate.trim() == "b true")
    }

    fn delegate_property() -> Option<String> {
        let pid = std::process::id().to_string();
        let unit = busctl(&[
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "GetUnitByPID",
            "u",
            &pid,
        ])?;
        let unit = unit.trim().strip_prefix("o ")?.trim_matches('"').to_owned();
        busctl(&[
            "get-property",
            "org.freedesktop.systemd1",
            &unit,
            "org.freedesktop.systemd1.Service",
            "Delegate",
        ])
    }

    /// The name carries the start time as well as the pid, since agents of a core that crashed
    /// can keep its scope alive until a later core is given the same pid.
    fn start_own_scope(slice: Option<&str>) -> Option<PathBuf> {
        let pid = std::process::id().to_string();
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let name = format!("{}{started}.scope", own_scope_prefix());
        let mut properties: Vec<Vec<&str>> = vec![
            vec!["PIDs", "au", "1", &pid],
            vec!["Delegate", "b", "true"],
            vec!["CollectMode", "s", "inactive-or-failed"],
            vec!["Description", "s", "Sikemux core"],
        ];
        if let Some(slice) = slice {
            properties.push(vec!["Slice", "s", slice]);
        }
        let count = properties.len().to_string();
        let mut args = vec![
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            &name,
            "fail",
            &count,
        ];
        args.extend(properties.iter().flatten().copied());
        args.push("0");
        busctl(&args)?;
        let suffix = format!("/{name}");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let own = own_cgroup()?;
            if own.ends_with(&suffix) {
                return Some(Path::new(CGROUPS).join(own.trim_start_matches('/')));
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Moves the core into the unit's `core` cgroup, since a cgroup that hands controllers down
    /// to cgroups below it may hold no processes itself, then hands down the memory controller
    /// systemd-oomd reads.
    fn settle_in(root: &Path) -> bool {
        let core = root.join(CORE_LEAF);
        if !core.is_dir() && fs::create_dir(&core).is_err() {
            return false;
        }
        if fs::write(core.join("cgroup.procs"), std::process::id().to_string()).is_err() {
            return false;
        }
        let has_memory = |file: &str| {
            fs::read_to_string(root.join(file))
                .unwrap_or_default()
                .split_whitespace()
                .any(|controller| controller == CONTROLLER)
        };
        if !has_memory("cgroup.controllers") {
            return false;
        }
        let _ = fs::write(
            root.join("cgroup.subtree_control"),
            format!("+{CONTROLLER}"),
        );
        if !has_memory("cgroup.subtree_control") {
            return false;
        }
        remove_children_of_gone_cores(root);
        true
    }

    /// Clears the empty child cgroups cores that ran here before left behind, leaving those of
    /// any core still running.
    fn remove_children_of_gone_cores(root: &Path) {
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        let own = std::process::id().to_string();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(core) = name
                .to_str()
                .and_then(|name| name.strip_prefix(CHILD_LEAF_PREFIX))
                .and_then(|rest| rest.split('-').next())
            else {
                continue;
            };
            if core == own || !Path::new("/proc").join(core).exists() {
                let _ = fs::remove_dir(entry.path());
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use std::process::Command;

    pub(super) fn take_own_unit() {}

    pub(super) fn give_own_cgroup(_command: &mut Command) {}

    pub(super) fn move_into_own_cgroup(_pid: u32) {}
}
