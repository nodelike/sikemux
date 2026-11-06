//! This host, the devices paired with it, and what the core builds for them.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceAccess {
    /// Everything a person at the host can do in a session.
    Full,
    /// Read sessions and answer agents' permission requests.
    Watch,
}

/// The computer a core runs on, as a person would recognise it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    /// The name set in System Settings, such as "Kishore's MacBook Pro".
    pub name: String,
    /// The kind of computer, such as "MacBook Pro" or "Mac mini".
    pub model: String,
    /// The Sikemux release running there, such as "0.4.3-nightly.5".
    pub version: String,
    pub channel: BuildChannel,
    /// The home folder of the person the core runs as, so a client can shorten its paths.
    pub home: PathBuf,
}

/// Which kind of Sikemux build a core belongs to. Each keeps its own key and
/// paired devices, so one host can show up once per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildChannel {
    Dev,
    Nightly,
    Stable,
}

impl BuildChannel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Nightly => "nightly",
            Self::Stable => "stable",
        }
    }

    /// A release's channel by its version: `0.5.0-nightly.3` is a nightly.
    pub fn of_release(version: &str) -> Self {
        if version.contains("-nightly") {
            Self::Nightly
        } else {
            Self::Stable
        }
    }
}

/// A device approved to reach this core from another machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// The device's public key, which is what the core recognises it by.
    pub id: String,
    pub name: String,
    pub platform: String,
    pub access: DeviceAccess,
    /// Milliseconds since the Unix epoch.
    pub paired_at: u64,
    pub last_seen: Option<u64>,
}

/// A phone the person lets in ahead of it asking, by its account key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowedDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub access: DeviceAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    /// The core's public key, which paired devices dial.
    pub core_id: String,
    /// Where the core can be reached directly, while remote access is on.
    pub addresses: Vec<String>,
    pub devices: Vec<DeviceInfo>,
    /// Ids of the devices connected now.
    pub connected: Vec<String>,
    /// Phones that came with a ticket from the account and wait for the
    /// person to answer.
    pub pending: Vec<PendingDevice>,
    /// The account this host is signed in to.
    pub owner: Option<String>,
    /// The live connection to that account, or why the account let this
    /// host go.
    #[serde(default)]
    pub account: Option<AccountLink>,
    /// Set while the accounts server no longer works with this build. Remote
    /// access and the account stay off until it updates.
    #[serde(default)]
    pub update_required: Option<UpdateRequired>,
    /// The phones that asked this host for notifications.
    #[serde(default)]
    pub notifications: Vec<PhoneNotifications>,
    /// The person asked to allow each new device and host here, rather than
    /// let in whatever their account signed in (ADR 0018).
    #[serde(default)]
    pub require_approval: bool,
}

/// What a phone wants to hear about from this host. The phone owns these
/// and sends them on every connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyPrefs {
    /// Permission requests, and terminal agents waiting for input.
    pub needs_you: bool,
    pub finished: bool,
    pub problems: bool,
    pub when: NotifyWhen,
    #[serde(default)]
    pub muted: Vec<NotifyMute>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotifyWhen {
    /// Only while nobody is using this host: locked, or untouched for a
    /// couple of minutes.
    Away,
    Always,
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyMute {
    pub agent_id: String,
    /// Milliseconds since the Unix epoch, or `None` while the agent runs.
    pub until: Option<u64>,
}

/// Whether a phone's notifications from this host reach it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneNotifications {
    pub device_id: String,
    pub state: NotificationState,
    /// Milliseconds since the Unix epoch when `state` began.
    pub since: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationState {
    /// The phone asked for none from this host.
    Off,
    /// The last one was delivered, or none was sent yet.
    On,
    /// Notifications are turned off on the phone itself.
    PhoneOff,
    /// The push services could not deliver the last one.
    NotReaching,
    /// The phone is not on this host's account, so the server will not
    /// pass notifications to it.
    OtherAccount,
    /// This host is not signed in to an account, which notifications go
    /// through.
    SignedOut,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRequired {
    pub current: String,
    pub minimum: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLink {
    pub state: AccountLinkState,
    /// Why the account let this host go, once `state` is `removed`. None
    /// when the server no longer knew this host at all.
    pub reason: Option<crate::accounts::protocol::RevokeReason>,
    /// Milliseconds since the Unix epoch when `state` began.
    pub since: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountLinkState {
    Connecting,
    Live,
    /// The last attempt failed; another follows.
    Offline,
    /// Taken off the account from elsewhere, or the account was deleted.
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDevice {
    /// Names this request in `AnswerPairing`.
    pub id: String,
    pub device_id: String,
    /// What the device calls itself. Nothing vouches for it.
    pub name: String,
    pub platform: String,
    /// The device came with a ticket from the host's account, as every
    /// device does.
    #[serde(default)]
    pub from_account: bool,
    /// When the phone stops waiting for an answer, in milliseconds since the
    /// Unix epoch.
    pub expires_at: u64,
}

/// Which build of the core's program a core runs. `source` fingerprints the code
/// the core is compiled from, so two compilations of the same code are the
/// same build although they finished at different times.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildIdentity {
    pub version: String,
    pub commit: String,
    pub built_at: u64,
    pub source: String,
}

impl BuildIdentity {
    /// Whether both run the same code, whenever and from whichever commit
    /// each was compiled.
    pub fn same_build(&self, other: &Self) -> bool {
        self.version == other.version && self.source == other.source
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkChange {
    Online,
    Offline,
    /// Still online, over a different interface.
    Changed,
    /// The Mac woke from sleep.
    Woke,
}

/// A picture shown behind the panes, shrunk to suit a phone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackdropImage {
    /// Changes whenever the picture does, so a device fetches it only then.
    pub id: String,
    pub data_url: String,
}
