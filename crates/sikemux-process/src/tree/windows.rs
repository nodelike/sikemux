use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Once, PoisonError};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, TRUE};
use windows_sys::Win32::System::Console::{
    AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleProcessList,
    SetConsoleCtrlHandler, CTRL_C_EVENT,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    JobObjectBasicProcessIdList, QueryInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECTINFOCLASS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE,
};

/// The exit code of a process stopped by force, as Task Manager leaves it.
const KILLED: u32 = 1;

struct Handle(HANDLE);

// SAFETY: a kernel handle may be used and closed from any thread.
unsafe impl Send for Handle {}
// SAFETY: as above; the calls made with it are thread-safe.
unsafe impl Sync for Handle {}

impl Handle {
    fn new(raw: HANDLE) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }

    fn process(pid: u32, access: u32) -> Option<Self> {
        // SAFETY: OpenProcess takes plain values and returns a handle we own, or null.
        Self::new(unsafe { OpenProcess(access, FALSE, pid) })
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is open and owned by this value alone.
        unsafe { CloseHandle(self.0) };
    }
}

/// A job holding a tree. Its head stays open, so Windows gives the head's pid to no other
/// process while the tree is known by it.
struct Tree {
    job: Handle,
    _head: Handle,
}

impl Tree {
    fn query<T>(&self, class: JOBOBJECTINFOCLASS, into: &mut T) -> bool {
        // SAFETY: `into` is valid for writes of its full size.
        unsafe {
            QueryInformationJobObject(
                self.job.0,
                class,
                (into as *mut T).cast(),
                size_of::<T>() as u32,
                std::ptr::null_mut(),
            ) != 0
        }
    }

    fn running(&self) -> bool {
        // SAFETY: the struct is plain integers, for which zero is valid.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        !self.query(JobObjectBasicAccountingInformation, &mut info) || info.ActiveProcesses > 0
    }

    fn members(&self) -> Vec<u32> {
        // Two counts fill the first word, then one pid per word.
        let mut list = [0_usize; 65];
        if !self.query(JobObjectBasicProcessIdList, &mut list) {
            return Vec::new();
        }
        let listed = list[0] >> 32;
        list[1..=listed.min(64)]
            .iter()
            .map(|&pid| pid as u32)
            .collect()
    }
}

static TREES: LazyLock<Mutex<HashMap<u32, Arc<Tree>>>> = LazyLock::new(Default::default);

fn trees() -> std::sync::MutexGuard<'static, HashMap<u32, Arc<Tree>>> {
    TREES.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn own_tree(pid: u32) {
    let access = PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION;
    let Some(head) = Handle::process(pid, access) else {
        return;
    };
    // The job is not killed when its last handle closes: a tree outlives the core that
    // started it, as a Unix process group does, so the next core can adopt it.
    // SAFETY: both arguments may be null; the result is a handle we own, or null.
    let Some(job) = Handle::new(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })
    else {
        return;
    };
    // SAFETY: both handles are open, and the process handle has PROCESS_SET_QUOTA and
    // PROCESS_TERMINATE.
    if unsafe { AssignProcessToJobObject(job.0, head.0) } == 0 {
        return;
    }
    let mut trees = trees();
    trees.retain(|_, tree| tree.running());
    trees.insert(pid, Arc::new(Tree { job, _head: head }));
}

pub(super) fn terminate_tree(pid: u32, force: bool) {
    if pid == 0 {
        return;
    }
    if !force {
        let members = trees()
            .get(&pid)
            .cloned()
            .map(|tree| tree.members())
            .unwrap_or_default();
        interrupt(std::iter::once(pid).chain(members));
        return;
    }
    let owned = trees().remove(&pid);
    match owned {
        // SAFETY: the job handle is open.
        Some(tree) => unsafe {
            TerminateJobObject(tree.job.0, KILLED);
        },
        None => terminate_descendants(pid),
    }
}

/// Stops `head` and every process below it, for a tree started before this process owned
/// it, such as one a previous core handed over. A process that started before the one
/// it names as its parent took a pid Windows had freed, so it is not part of the tree.
fn terminate_descendants(head: u32) {
    let processes = crate::procs::processes();
    let started: HashMap<u32, u64> = processes
        .iter()
        .map(|&(pid, _, started)| (pid, started))
        .collect();
    let mut tree = HashSet::from([head]);
    let mut queue = vec![head];
    while let Some(parent) = queue.pop() {
        let parent_started = started.get(&parent).copied();
        for &(pid, _, child_started) in processes.iter().filter(|(_, of, _)| *of == parent) {
            if parent_started.is_some_and(|parent| child_started < parent) {
                continue;
            }
            if tree.insert(pid) {
                queue.push(pid);
            }
        }
    }
    for pid in tree {
        if let Some(process) = Handle::process(pid, PROCESS_TERMINATE) {
            // SAFETY: the handle is open with PROCESS_TERMINATE.
            unsafe { TerminateProcess(process.0, KILLED) };
        }
    }
}

/// Ctrl-C events this process raised into another console and has yet to hear itself.
static RAISED: AtomicU32 = AtomicU32::new(0);
/// A process has one console at a time, so one interrupt at a time borrows it.
static CONSOLE: Mutex<()> = Mutex::new(());

unsafe extern "system" fn swallow_raised(event: u32) -> BOOL {
    let raised = event == CTRL_C_EVENT
        && RAISED
            .try_update(Ordering::AcqRel, Ordering::Acquire, |left| left.checked_sub(1))
            .is_ok();
    if raised {
        TRUE
    } else {
        FALSE
    }
}

/// Presses Ctrl-C in the console of the first of `candidates` that has one, which every
/// console program in it hears. Windows sends the event only within a console, so this
/// process joins that console for the moment it takes, then returns to its own.
fn interrupt(candidates: impl IntoIterator<Item = u32>) {
    static HANDLER: Once = Once::new();
    // SAFETY: the handler is a plain function that lives as long as the process.
    HANDLER.call_once(|| unsafe {
        SetConsoleCtrlHandler(Some(swallow_raised), TRUE);
    });
    let _console = CONSOLE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut sharing = [0_u32; 64];
    // SAFETY: the buffer is valid for the count given.
    let count = unsafe { GetConsoleProcessList(sharing.as_mut_ptr(), sharing.len() as u32) };
    let count = count as usize;
    let own = std::process::id();
    let back_to = if count == 0 {
        None
    } else {
        // Leaving a console no other process holds would close it for good.
        let Some(&other) = sharing
            .get(..count)
            .and_then(|sharing| sharing.iter().find(|&&pid| pid != own))
        else {
            return;
        };
        // SAFETY: takes no arguments; this process holds a console.
        unsafe { FreeConsole() };
        Some(other)
    };
    for candidate in candidates {
        // Ctrl-C in this process's own console would reach whatever else holds it.
        if sharing[..count.min(sharing.len())].contains(&candidate) {
            continue;
        }
        // SAFETY: AttachConsole takes a pid and fails harmlessly for one without a console.
        if unsafe { AttachConsole(candidate) } == 0 {
            continue;
        }
        RAISED.fetch_add(1, Ordering::AcqRel);
        // SAFETY: group 0 is every process in the console this process now shares.
        if unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) } == 0 {
            RAISED.fetch_sub(1, Ordering::AcqRel);
        }
        // SAFETY: this process is attached to the console it just joined.
        unsafe { FreeConsole() };
        break;
    }
    if let Some(other) = back_to {
        // SAFETY: rejoins the console `other` still holds.
        unsafe { AttachConsole(other) };
    }
}
