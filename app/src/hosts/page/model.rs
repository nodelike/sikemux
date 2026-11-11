//! What the Hosts page knows of each host, gathered from its core's view and `hostStats`.

use gpui_kit::{Hsla, SharedString};
use sikemux_core::server::hosts::{HostReach, HostRow};
use sikemux_view::device::DeviceView;
use sikemux_view::host_stats::{
    AlertLevel, HostAlert, HostAlertKind, HostEventKind, HostMark, HostStats,
};
use sikemux_view::inbox::AgentShown;
use sikemux_view::updates::UpdateState;
use sikemux_wire::protocol::AgentStatus;

use crate::hosts::{Feed, Place};
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::workbench::Workbench;

/// The key this host goes by on the page, where it has no core key of its own to show.
pub const THIS: &str = "this";

/// How long a killed agent counts on its host's card.
const KILLED_FOR_MS: u64 = 15 * 60 * 1000;

/// A share at or past which a meter warns, while no alert says otherwise.
const WARN_AT: f32 = 85.;

pub struct Listed<'a> {
    pub key: String,
    pub place: Place,
    pub name: SharedString,
    pub row: Option<&'a HostRow>,
    pub feed: Option<&'a Feed>,
    pub state: State,
}

impl<'a> Listed<'a> {
    pub fn view(&self) -> Option<&'a DeviceView> {
        self.feed?.view.as_ref()
    }

    pub fn stats(&self) -> Option<&'a HostStats> {
        self.feed?.stats.as_ref()
    }

    pub fn online(&self) -> bool {
        !matches!(self.state, State::Offline { .. })
    }

    pub fn is_this(&self) -> bool {
        self.place == Place::This
    }

    /// What it is, as its card says under its name.
    pub fn model(&self) -> String {
        let about = self.stats().map(|stats| &stats.about);
        about
            .and_then(|about| about.model.clone())
            .or_else(|| about.map(|about| about.system.clone()))
            .or_else(|| self.row.and_then(|row| row.platform.clone()))
            .unwrap_or_default()
    }

    pub fn mark(&self) -> HostMark {
        self.stats()
            .map_or(HostMark::Unknown, |stats| stats.about.mark)
    }

    /// The agents its core lists open, with those it keeps asleep or ended.
    pub fn agents(&self) -> impl Iterator<Item = &'a AgentShown> {
        self.view()
            .into_iter()
            .flat_map(|view| view.inbox.agents.iter().chain(&view.inbox.kept))
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        if !self.online() {
            counts.away = self.agents().count();
            return counts;
        }
        for agent in self.agents() {
            match agent.status {
                AgentStatus::Working | AgentStatus::Blocked => counts.running += 1,
                AgentStatus::Stopped => counts.stopped += 1,
                _ => counts.idle += 1,
            }
        }
        counts.killed = self.killed().len();
        counts
    }

    /// The agents the system stopped lately to free memory, by id.
    pub fn killed(&self) -> Vec<String> {
        let since = crate::ui::time::now_ms().saturating_sub(KILLED_FOR_MS);
        let mut killed: Vec<String> = Vec::new();
        for event in self.stats().into_iter().flat_map(|stats| &stats.events) {
            if let HostEventKind::Killed {
                agent_id: Some(agent),
                ..
            } = &event.what
            {
                if event.at >= since && !killed.contains(agent) {
                    killed.push(agent.clone());
                }
            }
        }
        killed
    }

    /// The agent its core lists by `id`.
    pub fn agent(&self, id: &str) -> Option<&'a AgentShown> {
        self.agents().find(|agent| agent.agent_id == id)
    }

    /// What it is called with its at sign, else what its row is titled.
    pub fn agent_name(&self, id: &str) -> String {
        match self.agent(id) {
            Some(AgentShown {
                name: Some(name), ..
            }) => format!("@{name}"),
            Some(agent) => agent.title.clone(),
            None => "an agent".to_owned(),
        }
    }

    /// How full or busy it is by `kind`, in percent, once its core has said.
    pub fn percent(&self, kind: HostAlertKind) -> Option<f32> {
        let stats = self.stats()?;
        let (used, total) = match kind {
            HostAlertKind::Cpu => return Some(stats.now.cpu as f32 / 10.),
            HostAlertKind::Memory => (stats.now.memory_used, stats.about.memory_total),
            HostAlertKind::Disk => (stats.now.disk_used, stats.about.disk_total),
            HostAlertKind::Offline | HostAlertKind::Unknown => return None,
        };
        (total > 0).then(|| used as f32 * 100. / total as f32)
    }

    /// How a meter of `kind` is coloured: as its alert says, else warning when nearly full.
    pub fn level(&self, kind: HostAlertKind) -> Option<AlertLevel> {
        let alerts = self.stats().into_iter().flat_map(|stats| &stats.alerts);
        if let Some(alert) = alerts
            .filter(|alert| alert.kind == kind)
            .max_by_key(|alert| alert.level == AlertLevel::Danger)
        {
            return Some(alert.level);
        }
        self.percent(kind)
            .filter(|percent| *percent >= WARN_AT)
            .map(|_| AlertLevel::Warn)
    }
}

/// How a host stands, worst first.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Alert(HostAlert),
    /// Out of reach, since the moment this window saw it go when it did.
    Offline {
        since: Option<u64>,
    },
    Updating,
    Healthy,
}

impl State {
    pub fn rank(&self) -> u8 {
        match self {
            State::Alert(alert) if alert.level == AlertLevel::Danger => 0,
            State::Offline { .. } => 1,
            State::Alert(_) => 2,
            State::Updating => 3,
            State::Healthy => 4,
        }
    }

    pub fn needs_you(&self) -> bool {
        matches!(self, State::Alert(_) | State::Offline { .. })
    }

    pub fn tint(&self, t: &Tokens) -> Hsla {
        match self {
            State::Alert(alert) => crate::hosts::alert_tint(alert.level, t),
            State::Offline { .. } => t.danger,
            State::Updating => info(t),
            State::Healthy => t.live,
        }
    }

    /// The glyph before its words; a healthy host gets a plain dot instead.
    pub fn icon(&self) -> Option<Icon> {
        match self {
            State::Alert(alert) if alert.level == AlertLevel::Danger => Some(Icon::Warning),
            State::Alert(_) => Some(Icon::Gauge),
            State::Offline { .. } => Some(Icon::Power),
            State::Updating => Some(Icon::Refresh),
            State::Healthy => None,
        }
    }

    /// What it says, such as "Memory 98%" or "Offline 14 min".
    pub fn words(&self) -> String {
        match self {
            State::Alert(alert) => crate::hosts::alert_words(alert),
            State::Offline { since: Some(since) } => {
                format!("Out of reach {}", crate::hosts::lasted(*since))
            }
            State::Offline { since: None } => "Out of reach".to_owned(),
            State::Updating => "Updating".to_owned(),
            State::Healthy => "Healthy".to_owned(),
        }
    }

    /// How long it has lasted, when that is worth a line of its own.
    pub fn lasted(&self) -> Option<String> {
        match self {
            State::Alert(alert) => Some(crate::hosts::lasted(alert.since)),
            _ => None,
        }
    }
}

/// Something under way that needs nobody, such as an update: the theme's cool blue.
pub fn info(t: &Tokens) -> Hsla {
    crate::theme::color(&t.highlight.property)
}

#[derive(Default, Clone, Copy)]
pub struct Counts {
    pub running: usize,
    pub idle: usize,
    pub killed: usize,
    pub stopped: usize,
    /// On a host out of reach: the agents it had when last seen.
    pub away: usize,
}

impl Counts {
    pub fn is_empty(&self) -> bool {
        self.running + self.idle + self.killed + self.stopped + self.away == 0
    }

    pub fn total(&self) -> usize {
        self.running + self.idle + self.stopped + self.away
    }
}

/// The hosts on the account that run Sikemux: this one, then the others in the order they were
/// linked, worst first. An SSH host without Sikemux has no core to say how it is, so it stays
/// in the rail and is left out here.
pub fn hosts(bench: &Workbench) -> Vec<Listed<'_>> {
    let own_key = bench.remote.as_ref().map(|remote| remote.core_id.as_str());
    let this = Listed {
        key: THIS.to_owned(),
        place: Place::This,
        name: this_name(&bench.feeds.this),
        row: None,
        feed: Some(&bench.feeds.this),
        state: state(Some(&bench.feeds.this), true, bench.host_alert(None, true)),
    };
    let others = bench.reads.hosts.iter().filter_map(|row| {
        let key = row.id.clone()?;
        if Some(key.as_str()) == own_key {
            return None;
        }
        let feed = bench.feeds.host(&key);
        let reached = crate::hosts::reach(bench, row) == HostReach::Connected;
        let alert = bench.host_alert(Some(&key), reached);
        let state = state(feed, reached, alert);
        Some(Listed {
            place: Place::Account(key.clone()),
            key,
            name: row.name.clone().into(),
            row: Some(row),
            feed,
            state,
        })
    });
    let mut listed: Vec<Listed> = std::iter::once(this).chain(others).collect();
    listed.sort_by_key(|host| host.state.rank());
    listed
}

/// The page's key for the host a tab or a link names: this host by "this" or its own core key.
pub fn page_key(bench: &Workbench, key: &str) -> String {
    let own = bench.remote.as_ref().map(|remote| remote.core_id.as_str());
    if key == THIS || Some(key) == own {
        THIS.to_owned()
    } else {
        key.to_owned()
    }
}

fn this_name(feed: &Feed) -> SharedString {
    (feed.stats.as_ref())
        .and_then(|stats| stats.about.name.clone())
        .unwrap_or_else(|| "This host".to_owned())
        .into()
}

/// How a host stands: out of reach, updating, else the worst alert about it, else healthy.
fn state(feed: Option<&Feed>, reached: bool, alert: Option<&HostAlert>) -> State {
    if !reached {
        let since = (alert.filter(|alert| alert.kind == HostAlertKind::Offline))
            .map(|alert| alert.since)
            .or(feed.and_then(|feed| feed.away_since));
        return State::Offline { since };
    }
    let updating = (feed.and_then(|feed| feed.view.as_ref()))
        .and_then(crate::hosts::host_updates)
        .is_some_and(|updates| {
            matches!(
                updates.state,
                UpdateState::Downloading
                    | UpdateState::Installing
                    | UpdateState::Waiting
                    | UpdateState::Ready
            )
        });
    if updating {
        return State::Updating;
    }
    let stats = feed.and_then(|feed| feed.stats.as_ref());
    let worst = alert
        .or_else(|| crate::hosts::worst_alert(stats.into_iter().flat_map(|stats| &stats.alerts)));
    match worst {
        Some(alert) => State::Alert(alert.clone()),
        None => State::Healthy,
    }
}

/// A size in bytes as gigabytes, to one place under a hundred: "15.7", "364".
pub fn gb(bytes: u64) -> String {
    let gb = bytes as f64 / 1024f64.powi(3);
    if gb < 100. {
        format!("{gb:.1}")
    } else {
        format!("{gb:.0}")
    }
}

/// A total size as its card says it: "16 GB", "960 GB", "2 TB".
pub fn size(bytes: u64) -> String {
    let gb = bytes as f64 / 1024f64.powi(3);
    if gb >= 1000. {
        let tb = gb / 1024.;
        match tb.fract() < 0.05 {
            true => format!("{tb:.0} TB"),
            false => format!("{tb:.1} TB"),
        }
    } else {
        format!("{gb:.0} GB")
    }
}

/// Used out of a total, in the total's unit: ("15.7", "16 GB"), ("0.82", "2 TB").
pub fn used_of(used: u64, total: u64) -> (String, String) {
    let total_gb = total as f64 / 1024f64.powi(3);
    let used_gb = used as f64 / 1024f64.powi(3);
    if total_gb >= 1000. {
        (format!("{:.2}", used_gb / 1024.), size(total))
    } else {
        (gb(used), size(total))
    }
}

/// The share of all cores in thousandths as cores in use: "7.0".
pub fn cores_used(cpu: u16, cores: u32) -> String {
    format!("{:.1}", cpu as f64 * cores as f64 / 1000.)
}

/// A share in thousandths as whole percent.
pub fn percent(thousandths: u16) -> String {
    format!("{}%", (thousandths as f32 / 10.).round())
}

/// A host's logo and the colour it is drawn in.
pub fn logo(mark: HostMark, t: &Tokens) -> (Icon, Hsla) {
    let hex = |value: u32| gpui_kit::rgb(value).into();
    match mark {
        HostMark::Apple => (Icon::Apple, t.text_primary),
        HostMark::Hetzner => (Icon::Hetzner, hex(0xD50C2D)),
        HostMark::DigitalOcean => (Icon::DigitalOcean, hex(0x0080FF)),
        HostMark::Nvidia => (Icon::Nvidia, hex(0x76B900)),
        HostMark::Ubuntu => (Icon::Ubuntu, hex(0xE95420)),
        HostMark::Debian => (Icon::Debian, hex(0xA81D33)),
        HostMark::Linux => (Icon::Linux, hex(0xFCC624)),
        HostMark::Unknown => (Icon::Server, t.text_secondary),
    }
}
