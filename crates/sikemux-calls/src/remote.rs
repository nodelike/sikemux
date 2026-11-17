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
