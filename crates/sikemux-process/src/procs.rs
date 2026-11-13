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

    pub(super) fn program(pid: u32) -> Option<std::path::PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        let mut buffer = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer's length is passed alongside it.
        let length = unsafe {
            libc::proc_pidpath(
                pid as libc::c_int,
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len() as u32,
            )
        };
        let length = usize::try_from(length).ok().filter(|length| *length > 0)?;
        Some(std::ffi::OsStr::from_bytes(&buffer[..length]).into())
    }

    fn folder(pid: u32) -> Option<std::path::PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
        let size = size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        // SAFETY: `info` is a writable `proc_vnodepathinfo` of `size` bytes, and the kernel
        // writes at most that many.
        let written = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if written != size {
            return None;
        }
        // SAFETY: the kernel filled the whole struct when it wrote `size` bytes.
        let info = unsafe { info.assume_init() };
        let path: Vec<u8> = (info.pvi_cdir.vip_path.iter().flatten())
            .take_while(|char| **char != 0)
            .map(|char| *char as u8)
            .collect();
        (!path.is_empty()).then(|| std::ffi::OsStr::from_bytes(&path).into())
    }

    pub(super) fn places() -> Option<Vec<super::Place>> {
        let (pids, whole) = listed_pids();
        if !whole || pids.is_empty() {
            return None;
        }
        let mut buffer = vec![0u8; argmax()?];
        let mut found = Vec::new();
        for pid in pids {
            let arguments = procargs_into(pid, &mut buffer)
                .and_then(|size| arguments_from_procargs(&buffer[..size]))
                .unwrap_or_default();
            let paths = [program(pid), folder(pid)]
                .into_iter()
                .flatten()
                .chain(super::argument_paths(&arguments));
            found.extend(paths.map(|path| super::Place { pid, path }));
        }
        Some(found)
    }

    fn all_pids() -> Vec<u32> {
        listed_pids().0
    }

    /// Every pid, and whether the list is whole: more processes may start between asking how
    /// many there are and listing them, past the room left for them.
    fn listed_pids() -> (Vec<u32>, bool) {
        // SAFETY: a null buffer asks only for how many pids there are.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let Ok(count) = usize::try_from(count) else {
            return (Vec::new(), false);
        };
        let mut pids = vec![0 as libc::pid_t; count + 64];
        let size = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `pids` is a writable buffer of `size` bytes.
        let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
        let listed = usize::try_from(listed).unwrap_or(0).min(pids.len());
        let listed_pids = pids[..listed]
            .iter()
            .filter_map(|pid| u32::try_from(*pid).ok())
            .collect();
        (listed_pids, listed > 0 && listed < pids.len())
    }

    pub(super) fn processes() -> Vec<(u32, u32, u64)> {
        all_pids()
            .into_iter()
            .filter_map(|pid| {
                let info = bsd_info(pid)?;
                Some((pid, info.pbi_ppid, start_of(&info)))
            })
            .collect()
    }

    pub(super) fn footprint(pid: u32) -> Option<u64> {
        rusage(pid).map(|usage| usage.ri_phys_footprint)
    }

    fn rusage(pid: u32) -> Option<libc::rusage_info_v2> {
        let mut usage = MaybeUninit::<libc::rusage_info_v2>::zeroed();
        // SAFETY: `usage` is a writable `rusage_info_v2`, the flavor the kernel is asked for.
        let status = unsafe {
            libc::proc_pid_rusage(
                pid as libc::c_int,
                libc::RUSAGE_INFO_V2,
                usage.as_mut_ptr().cast(),
            )
        };
        // SAFETY: the kernel filled it when it answered 0.
        (status == 0).then(|| unsafe { usage.assume_init() })
    }

    /// Nanoseconds in one unit of the kernel's CPU times, which are not nanoseconds on Apple
    /// silicon.
    fn timebase() -> (u64, u64) {
        static TIMEBASE: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
        *TIMEBASE.get_or_init(|| {
            #[repr(C)]
            struct Timebase {
                numer: u32,
                denom: u32,
            }
            extern "C" {
                fn mach_timebase_info(info: *mut Timebase) -> libc::c_int;
            }
            let mut timebase = Timebase { numer: 1, denom: 1 };
            // SAFETY: `timebase` is a writable struct of the shape the call fills.
            let status = unsafe { mach_timebase_info(&mut timebase) };
            if status != 0 || timebase.denom == 0 {
                return (1, 1);
            }
            (u64::from(timebase.numer), u64::from(timebase.denom))
        })
    }

    fn text(chars: &[libc::c_char]) -> String {
        let bytes: Vec<u8> = chars
            .iter()
            .take_while(|char| **char != 0)
            .map(|char| *char as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Only this user's processes: the kernel keeps others' usage to their owners.
    pub(super) fn usages() -> Vec<super::Usage> {
        let (numer, denom) = timebase();
        all_pids()
            .into_iter()
            .filter_map(|pid| {
                let info = bsd_info(pid)?;
                let usage = rusage(pid)?;
                let name = match text(&info.pbi_name) {
                    name if name.is_empty() => text(&info.pbi_comm),
                    name => name,
                };
                let cpu_time = u128::from(usage.ri_user_time + usage.ri_system_time)
                    * u128::from(numer)
                    / u128::from(denom);
                Some(super::Usage {
                    pid,
                    parent: info.pbi_ppid,
                    name,
                    cpu_ns: cpu_time as u64,
                    memory: usage.ri_phys_footprint,
                    started_ms: start_of(&info) / 1_000,
                })
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

    /// What `pid` was started with: argc, the executable path, padding, argv, then the
    /// environment.
    fn procargs(pid: u32) -> Option<Vec<u8>> {
        let mut buffer = vec![0u8; argmax()?];
        let size = procargs_into(pid, &mut buffer)?;
        buffer.truncate(size);
        Some(buffer)
    }

    fn argmax() -> Option<usize> {
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
        usize::try_from(argmax).ok()
    }

    /// Reads `pid`'s `KERN_PROCARGS2` into `buffer`, answering how much of it was written.
    fn procargs_into(pid: u32, buffer: &mut [u8]) -> Option<usize> {
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
        (status == 0).then_some(size)
    }

    /// macOS hides the environment of Apple's own binaries. Agents are not
    /// those, so theirs stays readable.
    pub(super) fn environment_value(pid: u32, key: &str) -> Option<String> {
        environment_from_procargs(&procargs(pid)?, key)
    }

    pub(super) fn command_line(pid: u32) -> Option<Vec<String>> {
        arguments_from_procargs(&procargs(pid)?)
    }

    fn arguments_from_procargs(bytes: &[u8]) -> Option<Vec<String>> {
        let argc = usize::try_from(i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?)).ok()?;
        let rest = bytes.get(4..)?;
        let rest = &rest[rest.iter().position(|byte| *byte == 0)?..];
        let rest = &rest[rest.iter().position(|byte| *byte != 0)?..];
        Some(
            rest.split(|byte| *byte == 0)
                .take(argc)
                .map(|argument| String::from_utf8_lossy(argument).into_owned())
                .collect(),
        )
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

    pub(super) fn footprint(pid: u32) -> Option<u64> {
        let statm = fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let resident: u64 = statm.split(' ').nth(1)?.parse().ok()?;
        // SAFETY: sysconf only reads the system's configuration.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
        Some(resident * page)
    }

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

    pub(super) fn places() -> Option<Vec<super::Place>> {
        use std::io::Read;
        let mut found = Vec::new();
        let mut line = Vec::new();
        for entry in fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
            else {
                continue;
            };
            let folder = entry.path();
            line.clear();
            let read = fs::File::open(folder.join("cmdline"))
                .and_then(|mut file| file.read_to_end(&mut line));
            let arguments: Vec<String> = match read {
                Ok(_) => (line.split(|byte| *byte == 0))
                    .map(|argument| String::from_utf8_lossy(argument).into_owned())
                    .collect(),
                Err(_) => Vec::new(),
            };
            let paths = ["exe", "cwd"]
                .into_iter()
                .filter_map(|link| fs::read_link(folder.join(link)).ok())
                .chain(super::argument_paths(&arguments));
            found.extend(paths.map(|path| super::Place { pid, path }));
        }
        Some(found)
    }

    /// Every process's usage, from `/proc/<pid>/stat`: CPU time in clock ticks, resident
    /// memory in pages and the start in ticks since boot.
    pub(super) fn usages() -> Vec<super::Usage> {
        // SAFETY: sysconf only reads the system's configuration.
        let (tick, page) = unsafe {
            (
                libc::sysconf(libc::_SC_CLK_TCK).max(1) as u64,
                libc::sysconf(libc::_SC_PAGESIZE).max(1) as u64,
            )
        };
        let booted_ms = fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|stat| {
                let line = stat.lines().find(|line| line.starts_with("btime "))?;
                line[6..].trim().parse::<u64>().ok()
            })
            .unwrap_or(0)
            * 1_000;
        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        let mut stat = String::new();
        entries
            .filter_map(|entry| {
                let pid: u32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
                stat.clear();
                std::io::Read::read_to_string(
                    &mut fs::File::open(format!("/proc/{pid}/stat")).ok()?,
                    &mut stat,
                )
                .ok()?;
                let close = stat.rfind(')')?;
                let name = stat.get(stat.find('(')? + 1..close)?.to_owned();
                let fields: Vec<&str> = stat.get(close + 2..)?.split(' ').collect();
                let number = |index: usize| fields.get(index)?.parse::<u64>().ok();
                Some(super::Usage {
                    pid,
                    parent: number(1)? as u32,
                    name,
                    cpu_ns: (number(11)? + number(12)?) * 1_000_000_000 / tick,
                    memory: number(21)? * page,
                    started_ms: booted_ms + number(19)? * 1_000 / tick,
                })
            })
            .collect()
    }

    pub(super) fn command_line(pid: u32) -> Option<Vec<String>> {
        let line = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(
            line.split(|byte| *byte == 0)
                .filter(|argument| !argument.is_empty())
                .map(|argument| String::from_utf8_lossy(argument).into_owned())
                .collect(),
        )
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
    pub(super) fn footprint(_pid: u32) -> Option<u64> {
        None
    }

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

    pub(super) fn usages() -> Vec<super::Usage> {
        Vec::new()
    }

    pub(super) fn command_line(_pid: u32) -> Option<Vec<String>> {
        None
    }

    pub(super) fn places() -> Option<Vec<super::Place>> {
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

/// The program `pid` runs, by its path.
#[cfg(target_os = "macos")]
pub fn program(pid: u32) -> Option<std::path::PathBuf> {
    system::program(pid)
}

/// A file or folder a process runs from: its program, its working folder, or a path among its
/// arguments.
#[derive(Clone, Debug)]
pub struct Place {
    pub pid: u32,
    pub path: std::path::PathBuf,
}

/// Where every process this user may look at runs from, or `None` where the system cannot say.
pub fn places() -> Option<Vec<Place>> {
    system::places()
}

/// The absolute paths among `arguments`, alone or after an `=`, as in `--manifest-path=/a/b`.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn argument_paths(arguments: &[String]) -> impl Iterator<Item = std::path::PathBuf> + '_ {
    arguments
        .iter()
        .map(|argument| {
            argument
                .split_once('=')
                .map_or(argument.as_str(), |(_, value)| value)
        })
        .filter(|value| value.starts_with('/'))
        .map(std::path::PathBuf::from)
}

/// Every process: its pid, its parent and when it started.
pub fn processes() -> Vec<(u32, u32, u64)> {
    system::processes()
}

pub fn environment_value(pid: u32, key: &str) -> Option<String> {
    system::environment_value(pid, key)
}

/// What one process uses now.
#[derive(Clone, Debug)]
pub struct Usage {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    /// The CPU time it has used since it started, in nanoseconds.
    pub cpu_ns: u64,
    /// Its memory, in bytes: what it alone holds where the system says, else what is resident.
    pub memory: u64,
    /// When it started, in Unix milliseconds.
    pub started_ms: u64,
}

/// What every process this user may look at uses.
pub fn usages() -> Vec<Usage> {
    system::usages()
}

/// The memory `pid` holds, in bytes: what it alone holds where the system says, else what is
/// resident.
pub fn footprint(pid: u32) -> Option<u64> {
    system::footprint(pid)
}

/// The arguments `pid` was started with, its program first.
pub fn command_line(pid: u32) -> Option<Vec<String>> {
    system::command_line(pid)
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

/// Puts the calling thread at the lowest priority, which throttles its disk reads too.
pub fn lowest_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: changes only this thread's priority, which throttles its disk reads too.
    unsafe {
        libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG);
    }
    #[cfg(target_os = "linux")]
    // SAFETY: on Linux a thread's nice value is its own; 0 names the calling thread.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
}
