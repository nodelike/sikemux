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
    let mut averages = [0f64; 3];
    // SAFETY: getloadavg writes at most the count it is given into the array.
    let read = unsafe { libc::getloadavg(averages.as_mut_ptr(), 1) };
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

#[cfg(not(target_os = "linux"))]
fn memory_used() -> Option<u64> {
    None
}
