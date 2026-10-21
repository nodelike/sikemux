//! How busy this host is, for the rows clients show it in.

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Load {
    /// The last minute's load as a share of the host's cores, in percent.
    cpu: u32,
    /// Memory in use, in bytes, where the host says.
    memory_used: Option<u64>,
}

pub(crate) fn now() -> Load {
    let mut averages = [0f64; 3];
    // SAFETY: getloadavg writes at most the count it is given into the array.
    let read = unsafe { libc::getloadavg(averages.as_mut_ptr(), 1) };
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get()) as f64;
    let cpu = if read == 1 {
        (averages[0] / cores * 100.0).round().clamp(0.0, 100.0) as u32
    } else {
        0
    };
    Load {
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
