use crate::hosts::LinkedHost;
use serde::{Deserialize, Serialize};
use sikemux_wire::protocol::DeviceAccess;
use sikemux_wire::protocol::DeviceInfo;
use sikemux_wire::protocol::PendingDevice;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSetting {
    pub require_approval: bool,
}

/// Who waits to join this host and who is paired, and the hosts linked with
/// it or asking to be, for the owner's devices.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePairing {
    pub pending: Vec<PendingDevice>,
    pub devices: Vec<DeviceInfo>,
    pub links: Vec<LinkedHost>,
    pub pending_links: Vec<PendingDevice>,
}

/// Why a device or host was let in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdmittedBy {
    /// Its ticket from the account, with nobody asked.
    Ticket,
    /// The person allowed it.
    Person,
    /// The person chose it when signing this host in.
    SignIn,
    /// This host linked with it itself.
    ThisHost,
    /// It was let in before hosts kept records, or nothing says why.
    Unsaid,
}

/// A device or host let in, kept after it leaves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Admission {
    pub key: String,
    pub name: String,
    pub platform: String,
    /// A host linked with, rather than a device paired.
    pub host: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// In milliseconds since the epoch, as is `left_at`.
    pub at: u64,
    pub how: AdmittedBy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_at: Option<u64>,
    /// The linked host that let it in and said so; none when this host did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub told_by: Option<String>,
}

#[derive(Deserialize)]
pub struct PairingAnswer {
    pub id: String,
    pub allow: bool,
    pub access: DeviceAccess,
}

#[derive(Deserialize)]
pub struct DeviceChange {
    pub id: String,
    /// None unpairs the device.
    #[serde(default)]
    pub access: Option<DeviceAccess>,
}
