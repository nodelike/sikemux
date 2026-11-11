//! How a Linux host's core is updated. Root installs it, so the person's
//! core asks a root service over [`SOCKET`], which finds the release in the
//! feeds itself and checks it before putting it in place.

use semver::Version;
use serde::{Deserialize, Serialize};

use sikemux_view::prefs::UpdateChannel;
use sikemux_view::updates::UpdateOutcome;

/// Where the installer puts the core, setgid to the group that guards its secrets.
pub const INSTALLED_CORE: &str = "/usr/local/lib/sikemux/sikemux-host";
/// Where the installer unpacks the AppImage for that core, so its window runs from
/// a file only root can change rather than from a mount anyone can make.
pub const INSTALLED_APP: &str = "/usr/local/lib/sikemux/app";
/// The window's program inside [`INSTALLED_APP`].
pub const INSTALLED_APP_PROGRAM: &str = "usr/bin/sikemux";
/// Where the root service that installs a new core listens.
pub const SOCKET: &str = "/run/sikemux-update.sock";
/// The guarded store's entry holding whether this person's host updates. Only
/// the core writes it, and root reads it before installing anything.
pub const SWITCH_SERVICE: &str = "sikemux-updates";
pub const SWITCH_ACCOUNT: &str = "switch";
pub const SWITCH_OFF: &str = "off";
/// How the last update ended, which root writes before it exits and anyone may read.
pub const OUTCOME_FILE: &str = "/usr/local/lib/sikemux/update-outcome.json";
/// The core an update replaced, kept until the new one says it started well.
pub const PREVIOUS_CORE: &str = "/usr/local/lib/sikemux/.sikemux-host.previous";

/// The one line the core sends root: the channel the person follows, and
/// what to do about the update `id`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRequest {
    pub channel: UpdateChannel,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub action: HostAction,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostAction {
    #[default]
    Install,
    /// The new core started well, so the one before it can go.
    Ready,
    /// The new core did not start well: put the one before it back.
    RollBack,
}

/// How the last update ended, when one was ever recorded.
pub fn read_outcome() -> Option<UpdateOutcome> {
    serde_json::from_slice(&std::fs::read(OUTCOME_FILE).ok()?).ok()
}

/// One line the root service writes back for each thing it did, ending with
/// `Installed`, `Current`, `Off` or `Failed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "camelCase")]
pub enum UpdateStep {
    Downloaded { version: String, bytes: u64 },
    Verified,
    Installed { from: String, version: String },
    Current { version: String },
    Off,
    Failed { message: String },
}

impl UpdateStep {
    pub fn is_last(&self) -> bool {
        !matches!(self, UpdateStep::Downloaded { .. } | UpdateStep::Verified)
    }
}

/// What `sikemux update` hears: what root did, and the version the core now
/// hands itself over to, if any.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostUpdate {
    pub steps: Vec<UpdateStep>,
    pub handing_over: Option<String>,
}

/// The file a release publishes for a host on `arch`.
pub fn host_asset(version: &Version, arch: &str) -> String {
    format!("sikemux-host-{version}-linux-{arch}.tar.gz")
}

/// Asks root to do what `request` says, telling `seen` each step on the way,
/// and answers with the last.
#[cfg(unix)]
pub async fn ask_root(request: &HostRequest, mut seen: impl FnMut(&UpdateStep)) -> UpdateStep {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let failed = |message: String| UpdateStep::Failed { message };
    let mut stream = match tokio::net::UnixStream::connect(SOCKET).await {
        Ok(stream) => stream,
        Err(error) => {
            return failed(format!(
                "this host's update service is not answering at {SOCKET} ({error}); rerun the installer to set it up"
            ))
        }
    };
    let request = serde_json::to_string(request).unwrap_or_default();
    if let Err(error) = stream.write_all(format!("{request}\n").as_bytes()).await {
        return failed(format!("could not reach the update service: {error}"));
    }
    let mut lines = BufReader::new(stream).lines();
    loop {
        let step = match lines.next_line().await {
            Ok(Some(line)) => serde_json::from_str(&line).unwrap_or_else(|_| {
                failed(format!("the update service said something unreadable: {line}"))
            }),
            Ok(None) => failed(
                "the update service stopped before it finished; see `journalctl -u 'sikemux-update@*'`"
                    .into(),
            ),
            Err(error) => failed(format!("lost the update service: {error}")),
        };
        if step.is_last() {
            return step;
        }
        seen(&step);
    }
}
