//! What the operating system says of processes: who a process's parent is, when it started,
//! what it is called, what runs below it, and its environment.

use std::collections::HashMap;

/// Past this many processes below one, the rest are left out.
const MAX_DESCENDANTS: usize = 4_096;

#[cfg(target_os = "macos")]
mod system {
    use std::ffi::c_void;
    use std::mem::{size_of, MaybeUninit};

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
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer's length is passed alongside it.
        let length = unsafe {
            libc::proc_name(
                pid as i32,
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len() as u32,
            )
        };
        (length > 0).then(|| String::from_utf8_lossy(&buffer[..length as usize]).into_owned())
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

    /// `pid`'s children, asked of the kernel directly, which is quicker than the whole table.
    pub(super) fn child_pids(pid: u32) -> Vec<u32> {
        let mut capacity = 64usize;
        loop {
            let mut buffer = vec![0 as libc::pid_t; capacity];
            // SAFETY: the buffer holds `capacity` pids and its byte length is passed.
            let count = unsafe {
                libc::proc_listchildpids(
                    pid as libc::pid_t,
                    buffer.as_mut_ptr().cast::<c_void>(),
                    (capacity * size_of::<libc::pid_t>()) as libc::c_int,
                )
            };
            if count <= 0 {
                return Vec::new();
            }
            let count = count as usize;
            if count < capacity {
                buffer.truncate(count);
                return buffer
                    .into_iter()
                    .filter(|child| *child > 0)
                    .map(|child| child as u32)
                    .collect();
            }
            capacity *= 4;
        }
    }

    /// macOS hides the environment of Apple's own binaries. Agents are not
    /// those, so theirs stays readable.
    pub(super) fn environment_value(pid: u32, key: &str) -> Option<String> {
        let mut argmax: libc::c_int = 0;
        let mut size = size_of::<libc::c_int>();
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        // SAFETY: `argmax` is a c_int and `size` says so.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&mut argmax as *mut libc::c_int).cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 || argmax <= 0 {
            return None;
        }
        let mut buffer = vec![0u8; argmax as usize];
        let mut size = buffer.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
        // SAFETY: `size` is the buffer's length; the kernel writes at most that much.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buffer.as_mut_ptr().cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 {
            return None;
        }
        environment_from_procargs(&buffer[..size.min(buffer.len())], key)
    }

    /// Reads one variable out of a `KERN_PROCARGS2` buffer: argc, the executable
    /// path, padding, argv, then the environment.
    pub(super) fn environment_from_procargs(bytes: &[u8], key: &str) -> Option<String> {
        let argc = usize::try_from(i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?)).ok()?;
        let rest = bytes.get(4..)?;
        let rest = &rest[rest.iter().position(|byte| *byte == 0)?..];
        let rest = &rest[rest.iter().position(|byte| *byte != 0)?..];
        let mut entries = rest.split(|byte| *byte == 0);
        for _ in 0..argc {
            entries.next()?;
        }
        entries
            .take_while(|entry| !entry.is_empty())
            .find_map(|entry| {
                entry
                    .strip_prefix(key.as_bytes())?
                    .strip_prefix(b"=")
                    .map(|value| String::from_utf8_lossy(value).into_owned())
            })
    }
}

#[cfg(target_os = "linux")]
mod system {
    use std::fs;

    /// `pid`'s parent and start time, from `/proc/<pid>/stat`, whose name may hold spaces and
    /// parentheses.
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

    pub(super) fn environment_value(pid: u32, key: &str) -> Option<String> {
        let environment = fs::read(format!("/proc/{pid}/environ")).ok()?;
        environment.split(|byte| *byte == 0).find_map(|entry| {
            entry
                .strip_prefix(key.as_bytes())?
                .strip_prefix(b"=")
                .map(|value| String::from_utf8_lossy(value).into_owned())
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod system {
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

    pub(super) fn environment_value(_pid: u32, _key: &str) -> Option<String> {
        None
    }
}

/// When `pid` started, in the system's own terms, which tells it apart from a later process
/// given the same pid.
pub fn started(pid: u32) -> Option<u64> {
    system::started(pid)
}

/// `pid`'s parent, with `pid`'s start time.
pub fn parent(pid: u32) -> Option<(u32, u64)> {
    system::parent(pid)
}

pub fn name(pid: u32) -> Option<String> {
    system::name(pid)
}

/// Every process: its pid, its parent and when it started.
pub fn processes() -> Vec<(u32, u32, u64)> {
    system::processes()
}

/// The variable `key` in `pid`'s environment.
pub fn environment_value(pid: u32, key: &str) -> Option<String> {
    system::environment_value(pid, key)
}

/// Every process below `root`, mapped to its parent.
pub fn descendants(root: u32) -> HashMap<u32, u32> {
    #[cfg(target_os = "macos")]
    let children_of = |pid: u32| system::child_pids(pid);
    #[cfg(not(target_os = "macos"))]
    let children_of = {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, parent, _) in processes() {
            children.entry(parent).or_default().push(pid);
        }
        move |pid: u32| children.get(&pid).cloned().unwrap_or_default()
    };
    let mut found = HashMap::new();
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        for child in children_of(parent) {
            if found.len() >= MAX_DESCENDANTS {
                return found;
            }
            if found.insert(child, parent).is_none() {
                queue.push(child);
            }
        }
    }
    found
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn procargs_environment_skips_the_arguments() {
        let mut bytes = 2i32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(b"/usr/bin/node\0\0\0\0node\0SIKEMUX_AGENT_ID=argv\0");
        bytes.extend_from_slice(b"HOME=/Users/me\0SIKEMUX_AGENT_ID=agent-4\0\0junk=1\0");
        assert_eq!(
            system::environment_from_procargs(&bytes, crate::vars::AGENT_ID).as_deref(),
            Some("agent-4")
        );
        assert_eq!(system::environment_from_procargs(&bytes, "PATH"), None);
        assert_eq!(system::environment_from_procargs(&bytes[..3], "HOME"), None);
    }

    #[test]
    fn the_kernel_reports_a_child_process() {
        let mut child = crate::user_environment::command("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let parents = descendants(std::process::id());
        let name = name(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(parents.get(&child.id()), Some(&std::process::id()));
        assert_eq!(name.as_deref(), Some("sleep"));
    }
}
