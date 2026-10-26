//! How busy this host is, for the rows clients show it in. It rides in the view while a client
//! that shows it is attached, and the view goes out again when what those rows show moves.

use std::sync::Arc;
use std::time::Duration;

use super::Core;
use crate::protocol::HostLoad;

/// How often the load is looked at again while a client shows it.
const LOOK_EVERY: Duration = Duration::from_secs(30);

/// What a row shows of `load`: its CPU share, and its memory to a tenth of a gigabyte.
pub(crate) fn shown(load: &HostLoad) -> (u32, Option<u64>) {
    let tenths = |bytes: u64| (bytes as f64 / (1u64 << 30) as f64 * 10.).round() as u64;
    (load.cpu, load.memory_used.map(tenths))
}

/// Sends the view again whenever what the rows show of the load moved since it last went out.
pub(crate) async fn watch(core: Arc<Core>) {
    let mut ticker = tokio::time::interval(LOOK_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if core.shows_load() && core.load_moved(&now()) {
            core.view_changed();
        }
    }
}

pub(crate) fn now() -> HostLoad {
    let (read, averages) = load_average();
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get()) as f64;
    let cpu = if read == 1 {
        (averages[0] / cores * 100.0).round().clamp(0.0, 100.0) as u32
    } else {
        0
    };
    HostLoad {
        cpu,
        memory_used: memory_used(),
    }
}

#[cfg(unix)]
fn load_average() -> (i32, [f64; 3]) {
    let mut averages = [0f64; 3];
    // SAFETY: getloadavg writes at most the count it is given into the array.
    let read = unsafe { libc::getloadavg(averages.as_mut_ptr(), 1) };
    (read, averages)
}

/// Windows keeps no load average, so this reports the share of CPU time busy since the last
/// call as that many cores' worth, which is the load average's unit.
#[cfg(windows)]
fn load_average() -> (i32, [f64; 3]) {
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    static LAST: Mutex<Option<(u64, u64)>> = Mutex::new(None);
    let ticks = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut idle, mut kernel, mut user) = (zero, zero, zero);
    // SAFETY: the three out-pointers are live locals.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return (0, [0.0; 3]);
    }
    // Kernel time includes idle time.
    let (idle, total) = (ticks(idle), ticks(kernel) + ticks(user));
    let Ok(mut last) = LAST.lock() else {
        return (0, [0.0; 3]);
    };
    let previous = last.replace((idle, total));
    let Some((last_idle, last_total)) = previous else {
        return (0, [0.0; 3]);
    };
    let span = total.saturating_sub(last_total);
    if span == 0 {
        return (0, [0.0; 3]);
    }
    let busy = span.saturating_sub(idle.saturating_sub(last_idle)) as f64 / span as f64;
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get()) as f64;
    (1, [busy * cores, 0.0, 0.0])
}

#[cfg(target_os = "linux")]
fn memory_used() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = info.lines().find(|line| line.starts_with(name))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib * 1024)
    };
    Some(field("MemTotal:")?.saturating_sub(field("MemAvailable:")?))
}

#[cfg(windows)]
fn memory_used() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    // SAFETY: the struct is zeroed except for its length, which the call requires, and is
    // written only within that length.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a live local of the size it declares.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0)
        .then(|| status.ullTotalPhys.saturating_sub(status.ullAvailPhys))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn memory_used() -> Option<u64> {
    None
}
