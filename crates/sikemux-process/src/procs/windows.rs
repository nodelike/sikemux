use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessBasicInformation, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, FALSE, FILETIME, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING,
};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

/// Where a 64-bit process keeps its environment, which no public header lays out: the
/// parameters' address in its PEB, then the block's address and size in those parameters.
const PEB_PARAMETERS: usize = 0x20;
const PARAMETERS_ENVIRONMENT: usize = 0x80;
const PARAMETERS_ENVIRONMENT_SIZE: usize = 0x3F0;

/// 1970 in the 100 ns ticks since 1601 that Windows counts time in.
const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

struct Entry {
    pid: u32,
    parent: u32,
    name: String,
}

/// Every process, from one snapshot.
fn entries() -> Vec<Entry> {
    // SAFETY: takes plain values; the result is a handle we own, or INVALID_HANDLE_VALUE.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut entries = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: the snapshot is open and `entry` has its size set, as both calls require.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let name = (entry.szExeFile.split(|&unit| unit == 0).next()).unwrap_or_default();
        entries.push(Entry {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(name),
        });
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot is open and owned here.
    unsafe { CloseHandle(snapshot) };
    entries
}

fn entry(pid: u32) -> Option<Entry> {
    entries().into_iter().find(|entry| entry.pid == pid)
}

/// A process opened to read about it.
struct Process(HANDLE);

impl Process {
    fn open(pid: u32) -> Option<Self> {
        // Reading memory needs PROCESS_VM_READ, which a protected process refuses.
        [
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            PROCESS_QUERY_LIMITED_INFORMATION,
        ]
        .into_iter()
        // SAFETY: OpenProcess takes plain values and returns a handle we own, or null.
        .map(|access| unsafe { OpenProcess(access, FALSE, pid) })
        .find(|handle| !handle.is_null())
        .map(Self)
    }

    /// When it started, and the CPU time it has used, both in 100 ns ticks.
    fn times(&self) -> Option<(u64, u64)> {
        let mut times = [FILETIME::default(); 4];
        let [created, exited, kernel, user] = &mut times;
        // SAFETY: the handle is open with PROCESS_QUERY_LIMITED_INFORMATION and every
        // pointer is valid for a write.
        if unsafe { GetProcessTimes(self.0, created, exited, kernel, user) } == 0 {
            return None;
        }
        let ticks =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        Some((ticks(times[0]), ticks(times[2]) + ticks(times[3])))
    }

    /// The memory it alone holds, in bytes.
    fn memory(&self) -> Option<u64> {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        // SAFETY: `counters` is valid for writes of the size it states.
        let read = unsafe {
            GetProcessMemoryInfo(
                self.0,
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX)
                    .cast::<PROCESS_MEMORY_COUNTERS>(),
                counters.cb,
            )
        } != 0;
        read.then_some(counters.PrivateUsage as u64)
    }

    fn command_line(&self) -> Option<Vec<String>> {
        let mut needed = 0_u32;
        // SAFETY: a null buffer of length 0 only asks for the size needed.
        unsafe {
            NtQueryInformationProcess(
                self.0,
                ProcessCommandLineInformation,
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        if (needed as usize) < size_of::<UNICODE_STRING>() {
            return None;
        }
        // Words, so the string's header at its start is aligned.
        let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
        // SAFETY: the buffer is valid for writes of the length given.
        let status = unsafe {
            NtQueryInformationProcess(
                self.0,
                ProcessCommandLineInformation,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut needed,
            )
        };
        if status < 0 {
            return None;
        }
        // SAFETY: on success the buffer starts with the string's header, whose text follows it
        // within the buffer.
        let mut line: Vec<u16> = unsafe {
            let header = &*buffer.as_ptr().cast::<UNICODE_STRING>();
            std::slice::from_raw_parts(header.Buffer, usize::from(header.Length) / 2).to_vec()
        };
        line.push(0);
        split_command_line(&line)
    }
}

impl Process {
    /// Its environment block: `NAME=value` strings, each ending in a nul.
    fn environment(&self) -> Option<Vec<u16>> {
        let mut basic = [0_usize; 6];
        let mut written = 0_u32;
        // SAFETY: `basic` has the size and layout of PROCESS_BASIC_INFORMATION and is valid
        // for writes of that size.
        let status = unsafe {
            NtQueryInformationProcess(
                self.0,
                ProcessBasicInformation,
                basic.as_mut_ptr().cast(),
                size_of_val(&basic) as u32,
                &mut written,
            )
        };
        if status < 0 {
            return None;
        }
        let peb = basic[1];
        let parameters = self.read_word(peb.checked_add(PEB_PARAMETERS)?)?;
        let block = self.read_word(parameters.checked_add(PARAMETERS_ENVIRONMENT)?)?;
        let size = self.read_word(parameters.checked_add(PARAMETERS_ENVIRONMENT_SIZE)?)?;
        let mut environment = vec![0_u16; size / 2];
        self.read(
            block,
            environment.as_mut_ptr().cast(),
            environment.len() * 2,
        )?;
        Some(environment)
    }

    fn read_word(&self, address: usize) -> Option<usize> {
        let mut word = 0_usize;
        self.read(
            address,
            (&mut word as *mut usize).cast(),
            size_of::<usize>(),
        )?;
        Some(word)
    }

    fn read(&self, address: usize, into: *mut core::ffi::c_void, length: usize) -> Option<()> {
        let mut copied = 0_usize;
        // SAFETY: `into` is valid for writes of `length` bytes; Windows checks the address
        // in the other process and fails rather than faulting.
        let read =
            unsafe { ReadProcessMemory(self.0, address as *const _, into, length, &mut copied) }
                != 0;
        (read && copied == length).then_some(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle is open and owned by this value alone.
        unsafe { CloseHandle(self.0) };
    }
}

/// `line`, ending in a nul, split into arguments the way a program's own startup splits it.
fn split_command_line(line: &[u16]) -> Option<Vec<String>> {
    let mut count = 0;
    // SAFETY: `line` ends in a nul; the result is an array Windows allocated, or null.
    let words = unsafe { CommandLineToArgvW(line.as_ptr(), &mut count) };
    if words.is_null() {
        return None;
    }
    let arguments = (0..usize::try_from(count).unwrap_or(0))
        .map(|index| {
            // SAFETY: the array holds `count` nul-terminated strings.
            unsafe {
                let word = *words.add(index);
                let length = (0..).take_while(|&at| *word.add(at) != 0).count();
                String::from_utf16_lossy(std::slice::from_raw_parts(word, length))
            }
        })
        .collect();
    // SAFETY: the array came from CommandLineToArgvW, which LocalFree releases.
    unsafe { LocalFree(words.cast()) };
    Some(arguments)
}

/// The program's file name without `.exe`, as Unix names a process.
fn program_name(name: String) -> String {
    let split = (name.len().checked_sub(4)).and_then(|stem| name.split_at_checked(stem));
    match split {
        Some((stem, extension)) if extension.eq_ignore_ascii_case(".exe") => stem.to_string(),
        _ => name,
    }
}

pub(super) fn footprint(pid: u32) -> Option<u64> {
    Process::open(pid)?.memory()
}

/// When `pid` was created, in 100 ns ticks since 1601.
pub(super) fn started(pid: u32) -> Option<u64> {
    Some(Process::open(pid)?.times()?.0)
}

pub(super) fn parent(pid: u32) -> Option<(u32, u64)> {
    Some((entry(pid)?.parent, started(pid)?))
}

pub(super) fn name(pid: u32) -> Option<String> {
    Some(program_name(entry(pid)?.name))
}

/// Every process with its parent and start; one this user may not look at started at 0.
pub(super) fn processes() -> Vec<(u32, u32, u64)> {
    entries()
        .into_iter()
        .map(|entry| (entry.pid, entry.parent, started(entry.pid).unwrap_or(0)))
        .collect()
}

/// Names match without regard to case, as Windows looks them up. A name may start with `=`,
/// as the hidden per-drive folders do, so the separator is the first `=` after that.
pub(super) fn environment_value(pid: u32, key: &str) -> Option<String> {
    let environment = Process::open(pid)?.environment()?;
    environment
        .split(|&unit| unit == 0)
        .take_while(|entry| !entry.is_empty())
        .find_map(|entry| {
            let entry = String::from_utf16_lossy(entry);
            let separator = 1 + entry.get(1..)?.find('=')?;
            let (name, value) = entry.split_at_checked(separator)?;
            let value = value.strip_prefix('=')?;
            name.eq_ignore_ascii_case(key).then(|| value.to_string())
        })
}

pub(super) fn usages() -> Vec<super::Usage> {
    entries()
        .into_iter()
        .filter_map(|entry| {
            let process = Process::open(entry.pid)?;
            let (started, cpu) = process.times()?;
            Some(super::Usage {
                pid: entry.pid,
                parent: entry.parent,
                name: program_name(entry.name),
                cpu_ns: cpu * 100,
                memory: process.memory().unwrap_or(0),
                started_ms: started.saturating_sub(UNIX_EPOCH_TICKS) / 10_000,
            })
        })
        .collect()
}

pub(super) fn command_line(pid: u32) -> Option<Vec<String>> {
    Process::open(pid)?.command_line()
}

pub(super) fn places() -> Option<Vec<super::Place>> {
    None
}
