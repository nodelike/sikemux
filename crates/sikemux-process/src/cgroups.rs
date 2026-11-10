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
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use crate::sync::Locked;

    const CGROUPS: &str = "/sys/fs/cgroup";
    const OWN_SCOPE_PREFIX: &str = "sikemux-core-";
    const CORE_LEAF: &str = "core";
    const CHILD_LEAF_PREFIX: &str = "child-";
    const CONTROLLERS: [&str; 2] = ["memory", "pids"];
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
        let Some(procs) = new_child_cgroup() else {
            return;
        };
        let Ok(procs) = CString::new(procs.into_os_string().into_vec()) else {
            return;
        };
        // SAFETY: the hook runs in the forked child before exec and calls only open, write and
        // close, which are async-signal-safe; `procs` was built before the fork.
        unsafe {
            command.pre_exec(move || {
                join(&procs);
                Ok(())
            });
        }
    }

    pub(super) fn move_into_own_cgroup(pid: u32) {
        if let Some(procs) = new_child_cgroup() {
            let _ = fs::write(procs, pid.to_string());
        }
    }

    /// Writing 0 to a cgroup's `cgroup.procs` moves the writing process into it.
    fn join(procs: &CStr) {
        // SAFETY: `procs` is a valid NUL-terminated path, and the fd opened here is closed here.
        unsafe {
            let fd = libc::open(procs.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
            if fd >= 0 {
                libc::write(fd, b"0".as_ptr().cast(), 1);
                libc::close(fd);
            }
        }
    }

    /// Makes an empty cgroup for one process and gives its `cgroup.procs` file.
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
        let procs = path.join("cgroup.procs");
        made.push((path, now));
        Some(procs)
    }

    /// The cgroup of a unit this core may make cgroups in: its own delegated service, or a scope
    /// it starts for itself when something else, such as the desktop, launched it.
    fn divisible_unit() -> Option<PathBuf> {
        let own = own_cgroup()?;
        let mut components: Vec<&str> = own.split('/').filter(|part| !part.is_empty()).collect();
        let manager = components
            .iter()
            .position(|part| part.starts_with("user@"))?;
        let unit = components
            .iter()
            .rposition(|part| part.ends_with(".scope") || part.ends_with(".service"))
            .filter(|&at| at > manager)?;
        let name = components[unit];
        components.truncate(unit + 1);
        let unit_path = Path::new(CGROUPS).join(components.join("/"));
        if name.starts_with(OWN_SCOPE_PREFIX) && name.ends_with(".scope") {
            return Some(unit_path);
        }
        if name.ends_with(".service") {
            return service_is_delegated().then_some(unit_path);
        }
        let slice = components[..unit]
            .last()
            .filter(|part| part.ends_with(".slice"))
            .copied();
        start_own_scope(slice)
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

    fn start_own_scope(slice: Option<&str>) -> Option<PathBuf> {
        let pid = std::process::id().to_string();
        let name = format!("{OWN_SCOPE_PREFIX}{pid}.scope");
        let mut properties = vec![
            "PIDs",
            "au",
            "1",
            &pid,
            "Delegate",
            "b",
            "true",
            "CollectMode",
            "s",
            "inactive-or-failed",
            "Description",
            "s",
            "Sikemux core",
        ];
        if let Some(slice) = slice {
            properties.extend(["Slice", "s", slice]);
        }
        let count = (properties.len() / 3).to_string();
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
        args.extend(properties);
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
        let available = fs::read_to_string(root.join("cgroup.controllers")).unwrap_or_default();
        let wanted: Vec<String> = CONTROLLERS
            .iter()
            .filter(|controller| {
                available
                    .split_whitespace()
                    .any(|have| have == **controller)
            })
            .map(|controller| format!("+{controller}"))
            .collect();
        let _ = fs::write(root.join("cgroup.subtree_control"), wanted.join(" "));
        let enabled = fs::read_to_string(root.join("cgroup.subtree_control")).unwrap_or_default();
        if !enabled
            .split_whitespace()
            .any(|controller| controller == "memory")
        {
            return false;
        }
        remove_empty_children(root);
        true
    }

    fn remove_empty_children(root: &Path) {
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            if entry
                .file_name()
                .as_bytes()
                .starts_with(CHILD_LEAF_PREFIX.as_bytes())
            {
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
