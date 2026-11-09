//! How busy this host is, for its page on the Hosts tab.

use super::{load, Core};
use sikemux_view::host_stats::{HostAbout, HostMark, HostSample, HostStats, HostStatsParams};

pub(crate) fn read(_core: &Core, _params: HostStatsParams) -> HostStats {
    let load = load::now();
    HostStats {
        about: HostAbout {
            model: None,
            system: std::env::consts::OS.to_owned(),
            mark: if cfg!(target_os = "macos") {
                HostMark::Apple
            } else {
                HostMark::Linux
            },
            cores: std::thread::available_parallelism().map_or(1, |cores| cores.get()) as u32,
            memory_total: 0,
            disk_total: 0,
            gpu: None,
            core_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        now: HostSample {
            at: sikemux_process::clock::epoch_ms(),
            cpu: (load.cpu * 10) as u16,
            memory_used: load.memory_used.unwrap_or(0),
            ..HostSample::default()
        },
        history: Vec::new(),
        agents: Vec::new(),
        rest: None,
        top: Vec::new(),
        disk_parts: Vec::new(),
        alerts: Vec::new(),
        events: Vec::new(),
    }
}
