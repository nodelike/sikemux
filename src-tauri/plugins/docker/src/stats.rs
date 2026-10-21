//! How hard each running container is working: CPU, memory, network and disk.

use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::containers::{self, ListParams};
use crate::engine::{segment, Engine};
use crate::error::DockerResult;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub id: String,
    pub name: String,
    /// Percent of one CPU, so a container busy on two CPUs reads 200, as `docker stats` shows it.
    pub cpu_percent: f64,
    /// Bytes in use, leaving out the file cache the kernel can take back.
    pub memory: u64,
    pub memory_limit: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub disk_read: u64,
    pub disk_write: u64,
    pub pids: u64,
}

mod raw {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct Sample {
        pub cpu_stats: Cpu,
        pub precpu_stats: Cpu,
        pub memory_stats: Memory,
        pub networks: Option<BTreeMap<String, Network>>,
        pub blkio_stats: Blkio,
        pub pids_stats: Pids,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct Cpu {
        pub cpu_usage: CpuUsage,
        pub system_cpu_usage: Option<u64>,
        pub online_cpus: Option<u32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct CpuUsage {
        pub total_usage: u64,
        pub percpu_usage: Option<Vec<u64>>,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct Memory {
        pub usage: u64,
        pub limit: u64,
        pub stats: Option<BTreeMap<String, u64>>,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct Network {
        pub rx_bytes: u64,
        pub tx_bytes: u64,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct Blkio {
        pub io_service_bytes_recursive: Option<Vec<Io>>,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct Io {
        pub op: String,
        pub value: u64,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub struct Pids {
        pub current: Option<u64>,
    }
}

fn cpu_percent(sample: &raw::Sample) -> f64 {
    let now = &sample.cpu_stats;
    let before = &sample.precpu_stats;
    let cpu_delta = now
        .cpu_usage
        .total_usage
        .saturating_sub(before.cpu_usage.total_usage);
    let system_delta = now
        .system_cpu_usage
        .unwrap_or(0)
        .saturating_sub(before.system_cpu_usage.unwrap_or(0));
    if cpu_delta == 0 || system_delta == 0 {
        return 0.0;
    }
    let cpus = now.online_cpus.map(f64::from).unwrap_or_else(|| {
        now.cpu_usage
            .percpu_usage
            .as_ref()
            .map_or(1.0, |each| each.len().max(1) as f64)
    });
    let percent = cpu_delta as f64 / system_delta as f64 * cpus * 100.0;
    (percent * 10.0).round() / 10.0
}

/// cgroup v2 calls the reclaimable cache `inactive_file`, v1 `total_inactive_file`.
fn memory_in_use(memory: &raw::Memory) -> u64 {
    let cache = memory
        .stats
        .as_ref()
        .and_then(|stats| {
            stats
                .get("inactive_file")
                .or_else(|| stats.get("total_inactive_file"))
                .copied()
        })
        .unwrap_or(0);
    memory.usage.saturating_sub(cache)
}

fn stats_of(id: String, name: String, sample: &raw::Sample) -> Stats {
    let (net_rx, net_tx) = sample
        .networks
        .iter()
        .flatten()
        .fold((0, 0), |(rx, tx), (_, network)| {
            (rx + network.rx_bytes, tx + network.tx_bytes)
        });
    let io = sample
        .blkio_stats
        .io_service_bytes_recursive
        .as_deref()
        .unwrap_or_default();
    let total = |op: &str| {
        io.iter()
            .filter(|entry| entry.op.eq_ignore_ascii_case(op))
            .map(|entry| entry.value)
            .sum()
    };
    Stats {
        id,
        name,
        cpu_percent: cpu_percent(sample),
        memory: memory_in_use(&sample.memory_stats),
        memory_limit: sample.memory_stats.limit,
        net_rx,
        net_tx,
        disk_read: total("read"),
        disk_write: total("write"),
        pids: sample.pids_stats.current.unwrap_or(0),
    }
}

async fn sample(engine: &Engine, id: &str) -> DockerResult<raw::Sample> {
    let id = segment(id)?;
    engine
        .get(&format!("/containers/{id}/stats?stream=false"))
        .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct StatsParams {
    /// Only these containers, by name or id. Without any, every running one.
    pub ids: Option<Vec<String>>,
}

/// Each container is asked at once, since the engine takes about a second to
/// answer while it measures.
pub async fn read(engine: &Engine, params: StatsParams) -> DockerResult<Vec<Stats>> {
    let running = containers::list(
        engine,
        ListParams {
            state: Some("running".to_owned()),
            ..ListParams::default()
        },
    )
    .await?;
    let wanted = params.ids.unwrap_or_default();
    let mut asks = JoinSet::new();
    for container in running {
        if !wanted.is_empty()
            && !wanted
                .iter()
                .any(|want| *want == container.id || *want == container.name)
        {
            continue;
        }
        let engine = engine.clone();
        asks.spawn(async move {
            let sample = sample(&engine, &container.id).await;
            (container.id, container.name, sample)
        });
    }
    let mut found = Vec::new();
    while let Some(asked) = asks.join_next().await {
        let Ok((id, name, Ok(sample))) = asked else {
            continue;
        };
        found.push(stats_of(id, name, &sample));
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "cpu_stats":{"cpu_usage":{"total_usage":3000000000},"system_cpu_usage":20000000000,"online_cpus":4},
        "precpu_stats":{"cpu_usage":{"total_usage":2000000000},"system_cpu_usage":10000000000},
        "memory_stats":{"usage":300000000,"limit":8000000000,"stats":{"inactive_file":100000000}},
        "networks":{"eth0":{"rx_bytes":1000,"tx_bytes":200},"eth1":{"rx_bytes":24,"tx_bytes":6}},
        "blkio_stats":{"io_service_bytes_recursive":[{"op":"read","value":4096},{"op":"write","value":8192},{"op":"Read","value":4}]},
        "pids_stats":{"current":7}
    }"#;

    #[test]
    fn a_sample_reads_as_docker_stats_shows_it() {
        let sample: raw::Sample = serde_json::from_str(SAMPLE).unwrap();
        let stats = stats_of("abc".to_owned(), "web".to_owned(), &sample);
        assert_eq!(stats.cpu_percent, 40.0);
        assert_eq!(
            (stats.memory, stats.memory_limit),
            (200_000_000, 8_000_000_000)
        );
        assert_eq!((stats.net_rx, stats.net_tx), (1024, 206));
        assert_eq!(
            (stats.disk_read, stats.disk_write, stats.pids),
            (4100, 8192, 7)
        );
    }

    #[test]
    fn a_first_sample_with_nothing_to_compare_reads_as_idle() {
        let sample: raw::Sample =
            serde_json::from_str(r#"{"cpu_stats":{"cpu_usage":{"total_usage":5}}}"#).unwrap();
        assert_eq!(cpu_percent(&sample), 0.0);
        assert_eq!(memory_in_use(&sample.memory_stats), 0);
    }
}
