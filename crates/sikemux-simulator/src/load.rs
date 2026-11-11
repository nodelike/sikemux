//! How many simulators this Mac can run at once, counting every one that runs,
//! whoever started it: Sikemux, Xcode, `xcodebuild` or `simctl`.

use std::collections::HashSet;
use std::path::PathBuf;

/// A booted simulator keeps about this many cores busy while it settles and an app runs on it.
const CORES_PER_SIMULATOR: usize = 3;
/// About what a booted iPhone simulator holds in memory with an app running.
const BYTES_PER_SIMULATOR: u64 = 5 << 29;
/// Past this many runnable threads per core, the Mac is too busy to start another simulator.
const BUSY_LOAD_PER_CORE: f64 = 1.5;
/// The program each running simulator's own launchd runs as, named for its device's folder.
const SIMULATOR_LAUNCHD: &str = "launchd_sim";

#[derive(Clone, Copy, Debug)]
pub(crate) struct Machine {
    pub cores: usize,
    pub free_memory: Option<u64>,
    /// The one-minute load average.
    pub load: Option<f64>,
}

impl Machine {
    pub fn now() -> Self {
        Self {
            cores: std::thread::available_parallelism().map_or(1, |cores| cores.get()),
            free_memory: sikemux_process::machine::memory()
                .map(|memory| memory.total.saturating_sub(memory.used)),
            load: load_average(),
        }
    }

    /// How many simulators may run beside the `running` ones: as many as the cores and
    /// free memory take, none more while the Mac is too busy, and always one.
    pub fn limit(&self, running: usize) -> usize {
        let by_cores = (self.cores / CORES_PER_SIMULATOR).max(1);
        let by_memory = self.free_memory.map_or(by_cores, |free| {
            running + (free / BYTES_PER_SIMULATOR) as usize
        });
        let limit = by_cores.min(by_memory);
        let limit = if self.busy() {
            limit.min(running)
        } else {
            limit
        };
        limit.max(1)
    }

    pub fn busy(&self) -> bool {
        self.load
            .is_some_and(|load| load > self.cores as f64 * BUSY_LOAD_PER_CORE)
    }
}

/// Why a simulator may not start now.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Wait {
    Full {
        running: usize,
        limit: usize,
    },
    /// Another simulator is starting, and a first boot goes alone.
    Starting,
}

/// Whether one more simulator may start, `cold` when it was never booted.
pub(crate) fn room(running: usize, starting: usize, cold: bool, limit: usize) -> Result<(), Wait> {
    if running >= limit {
        return Err(Wait::Full { running, limit });
    }
    if cold && starting > 0 {
        return Err(Wait::Starting);
    }
    Ok(())
}

fn load_average() -> Option<f64> {
    let mut loads = [0f64; 3];
    // SAFETY: the buffer holds the one sample asked for.
    let read = unsafe { libc::getloadavg(loads.as_mut_ptr(), 1) };
    (read == 1).then_some(loads[0])
}

/// The udids of every simulator running or starting on this Mac, from each one's launchd.
pub(crate) fn running() -> HashSet<String> {
    sikemux_process::procs::processes()
        .into_iter()
        .filter(|(pid, _, _)| {
            sikemux_process::procs::name(*pid).as_deref() == Some(SIMULATOR_LAUNCHD)
        })
        .filter_map(|(pid, _, _)| {
            sikemux_process::procs::command_line(pid)?
                .into_iter()
                .find_map(|word| device_of(&word))
        })
        .collect()
}

/// The udid of the device whose folder `path` is inside.
fn device_of(path: &str) -> Option<String> {
    let mut parts = path.split('/').skip_while(|part| *part != "Devices");
    parts.next()?;
    let udid = parts.next()?;
    (udid.len() == 36
        && udid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-'))
    .then(|| udid.to_owned())
}

/// Whether the iOS simulator `udid` was ever booted: its first boot lays out the
/// app containers, and is the slowest and heaviest boot it has.
pub(crate) fn booted_before(udid: &str) -> bool {
    devices_folder().is_none_or(|devices| devices.join(udid).join("data/Containers").is_dir())
}

pub(crate) fn devices_folder() -> Option<PathBuf> {
    Some(sikemux_process::paths::home()?.join("Library/Developer/CoreSimulator/Devices"))
}
