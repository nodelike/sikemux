//! What the Hosts page shows of one host: what it is, how busy it is now and lately, what its
//! agents use, and what about it needs the person.

use serde::{Deserialize, Serialize};

/// How far back the history a `hostStats` call answers reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatsSpan {
    /// Only what the host is doing now, as a card of it shows.
    #[default]
    Now,
    Minutes15,
    Hour,
    Day,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatsParams {
    #[serde(default)]
    pub span: StatsSpan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStats {
    pub about: HostAbout,
    pub now: HostSample,
    /// Samples over the span asked for, oldest first; none for [`StatsSpan::Now`].
    #[serde(default)]
    pub history: Vec<HostSample>,
    /// What each agent's processes use now, busiest first; none for [`StatsSpan::Now`].
    #[serde(default)]
    pub agents: Vec<AgentUse>,
    /// What runs outside every agent, taken together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest: Option<RestUse>,
    /// The busiest processes on the host, busiest first.
    #[serde(default)]
    pub top: Vec<ProcessUse>,
    /// What takes the disk, largest first, once it has been measured.
    #[serde(default)]
    pub disk_parts: Vec<DiskPart>,
    /// What about the host needs the person now.
    #[serde(default)]
    pub alerts: Vec<HostAlert>,
    /// What happened on the host lately, newest first.
    #[serde(default)]
    pub events: Vec<HostEvent>,
}

/// What a host is, which changes only when it is set up again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAbout {
    /// The computer's own name, such as "kishore-mbp".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What its maker calls it, such as "Mac mini (M2, 2023)" or "Hetzner AX52".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Its operating system and version, such as "macOS 26.1" or "Ubuntu 24.04".
    pub system: String,
    pub mark: HostMark,
    pub cores: u32,
    pub memory_total: u64,
    /// The size of the disk the home folder is on.
    pub disk_total: u64,
    /// Its graphics cards, such as "1× H100 80 GB", where it has any worth naming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    pub core_version: String,
}

/// Whose logo a host is shown with: its graphics card's maker, its cloud's, its own, or its
/// system's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostMark {
    Apple,
    Hetzner,
    DigitalOcean,
    Nvidia,
    Ubuntu,
    Debian,
    Linux,
    #[serde(other)]
    Unknown,
}

/// How busy a host was at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSample {
    /// Unix milliseconds.
    pub at: u64,
    /// The share of all its cores in use, in thousandths.
    pub cpu: u16,
    pub memory_used: u64,
    pub swap_used: u64,
    pub disk_used: u64,
}

/// What one agent's processes use together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUse {
    pub agent_id: String,
    /// The share of all the host's cores, in thousandths.
    pub cpu: u16,
    pub memory: u64,
    /// The command line of what it runs now beyond the agent itself, such as `cargo test`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// When `command` started, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_since: Option<u64>,
    /// The person the agent works for, on a host several people share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
}

/// What runs outside every agent, taken together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestUse {
    pub cpu: u16,
    pub memory: u64,
    /// The busiest of them by name, busiest first.
    pub names: Vec<String>,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessUse {
    /// The process's, or for an app its heaviest process's.
    pub pid: u32,
    pub name: String,
    pub cpu: u16,
    pub memory: u64,
    /// The Mac app it is, with its helpers taken together, by the key `appIcon` draws it for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// The agent whose process it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// One kind of thing taking the disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskPart {
    pub kind: DiskPartKind,
    pub bytes: u64,
    /// Whether the core can clear it, with `clearDisk`.
    pub clearable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiskPartKind {
    /// Build output, such as Rust's `target/` folders, in the projects Sikemux knows.
    BuildOutput,
    /// Agents' worktrees.
    Worktrees,
    /// Package managers' and tools' caches.
    Caches,
    /// Containers' images and volumes.
    Containers,
    /// Everything else on the disk.
    Other,
    #[serde(other)]
    Unknown,
}

/// Something about the host that needs the person, while it lasts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAlert {
    pub kind: HostAlertKind,
    pub level: AlertLevel,
    /// When it started, in Unix milliseconds.
    pub since: u64,
    /// How full or busy, in percent.
    pub percent: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostAlertKind {
    /// Short of memory, as the system says.
    Memory,
    /// The disk is nearly full.
    Disk,
    /// Every core busy for a while.
    Cpu,
    /// A linked host stopped answering.
    Offline,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertLevel {
    Warn,
    Danger,
}

/// Something that happened on the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostEvent {
    /// Unix milliseconds.
    pub at: u64,
    pub what: HostEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostEventKind {
    AlertStarted {
        alert: HostAlertKind,
        percent: u16,
    },
    AlertEnded {
        alert: HostAlertKind,
    },
    /// The system stopped an agent's process to free memory.
    Killed {
        agent_id: Option<String>,
        name: String,
    },
    CommandStarted {
        agent_id: String,
        command: String,
    },
    /// The build output of a worktree, named by its folder, was removed.
    BuildOutputCleared {
        worktree: String,
        /// How much the volume's free space grew, which for a copy-on-write clone is only
        /// what it no longer shared.
        bytes: u64,
        reason: ClearedFor,
    },
    #[serde(other)]
    Unknown,
}

/// Why a worktree's build output was removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClearedFor {
    /// Its agent closed.
    Closed,
    /// Its agent landed everything and ended its turn.
    Landed,
    /// The disk was nearly full and nothing had built in it for longest.
    DiskFull,
}
