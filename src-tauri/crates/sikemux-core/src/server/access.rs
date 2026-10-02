//! Who is on the other end of a connection, and what each request needs from
//! them. The app on this Mac may do anything. A paired device does what it
//! is approved for at the moment it asks, and never touches the core itself.

use crate::protocol::{DeviceAccess, Request};

use super::{CoreError, CoreResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Peer {
    /// The app or the CLI, over the core's own socket.
    Local,
    Device {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Needs {
    Watch,
    Full,
    Local,
}

pub(crate) const LOCAL_ONLY: &str = "only Sikemux on this Mac can do that";
pub(crate) const WATCH_ONLY: &str =
    "this device can watch and answer permission requests, not drive sessions";
pub(crate) const UNPAIRED: &str = "this device is no longer paired with this Mac";

pub(crate) fn needs(request: &Request) -> Needs {
    match request {
        Request::List
        | Request::Attach { .. }
        | Request::Detach { .. }
        | Request::Subscribe { .. }
        | Request::TaskOutput { .. }
        | Request::AcpList
        | Request::AcpAttach { .. }
        | Request::AcpDetach { .. }
        | Request::AcpWake { .. }
        | Request::BackdropImage
        | Request::Unpair
        | Request::AcpPermissionReply { .. }
        | Request::Workspace
        | Request::Attentions
        | Request::Host => Needs::Watch,
        Request::Spawn { .. }
        | Request::Resize { .. }
        | Request::Kill { .. }
        | Request::ResetModes { .. }
        | Request::AcpStart { .. }
        | Request::AcpPrompt { .. }
        | Request::AcpSteer { .. }
        | Request::AcpCancel { .. }
        | Request::AcpStopTask { .. }
        | Request::AcpStop { .. }
        | Request::AcpSetPermissionMode { .. }
        | Request::AcpSetConfig { .. }
        | Request::StartChat { .. } => Needs::Full,
        Request::Configure { .. }
        | Request::ListManifests
        | Request::ReloadManifests
        | Request::ExplainAgentDetection { .. }
        | Request::StopAll
        | Request::RegisterWindow
        | Request::HarnessAwaitingTrust { .. }
        | Request::HarnessStopRuns { .. }
        | Request::Shutdown { .. }
        | Request::RemoteStatus
        | Request::SetRemoteAccess { .. }
        | Request::SetDeviceAccess { .. }
        | Request::RevokeDevice { .. }
        | Request::OpenPairing
        | Request::ClosePairing
        | Request::AnswerPairing { .. }
        | Request::PublishWorkspace { .. }
        | Request::PublishChats { .. }
        | Request::PublishPalette { .. }
        | Request::PublishBackdrop { .. } => Needs::Local,
    }
}

impl Peer {
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Peer::Local)
    }

    pub(crate) fn device_id(&self) -> Option<String> {
        match self {
            Peer::Local => None,
            Peer::Device { id } => Some(id.clone()),
        }
    }

    pub(crate) fn is_device(&self, id: &str) -> bool {
        matches!(self, Peer::Device { id: own } if own == id)
    }
}

/// `access` is what the device may do now: `None` once it is unpaired.
pub(crate) fn permit(peer: &Peer, access: Option<DeviceAccess>, needs: Needs) -> CoreResult<()> {
    if peer.is_local() {
        return Ok(());
    }
    match (access, needs) {
        (None, _) => Err(CoreError::from(UNPAIRED)),
        (Some(_), Needs::Local) => Err(CoreError::from(LOCAL_ONLY)),
        (Some(DeviceAccess::Watch), Needs::Full) => Err(CoreError::from(WATCH_ONLY)),
        (Some(_), _) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SessionId;

    const SESSION: SessionId = 1;

    fn device() -> Peer {
        Peer::Device { id: "phone".into() }
    }

    fn refusal(access: Option<DeviceAccess>, request: &Request) -> String {
        permit(&device(), access, needs(request))
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn the_app_may_send_anything() {
        for request in [
            Request::Shutdown { stop_all: true },
            Request::RegisterWindow,
            Request::Kill { id: SESSION },
        ] {
            assert!(permit(&Peer::Local, None, needs(&request)).is_ok());
        }
    }

    #[test]
    fn no_device_reaches_the_core_itself() {
        for request in [
            Request::Shutdown { stop_all: true },
            Request::StopAll,
            Request::RegisterWindow,
            Request::Configure { manifest_dir: None },
            Request::SetRemoteAccess { enabled: false },
            Request::RevokeDevice { id: "other".into() },
            Request::OpenPairing,
            Request::AnswerPairing {
                id: "request".into(),
                allow: true,
                access: DeviceAccess::Full,
            },
        ] {
            assert_eq!(refusal(Some(DeviceAccess::Full), &request), LOCAL_ONLY);
        }
    }

    #[test]
    fn a_watching_device_reads_and_answers_permissions_but_does_not_drive() {
        let permission = Request::AcpPermissionReply {
            agent_id: "a".into(),
            request_id: "r".into(),
            option_id: None,
        };
        let watch = Some(DeviceAccess::Watch);
        assert!(permit(&device(), watch, needs(&permission)).is_ok());
        assert!(permit(&device(), watch, needs(&Request::Attach { id: SESSION })).is_ok());
        for request in [
            Request::Kill { id: SESSION },
            Request::Resize {
                id: SESSION,
                cols: 80,
                rows: 24,
            },
            Request::AcpCancel {
                agent_id: "a".into(),
            },
        ] {
            assert_eq!(refusal(watch, &request), WATCH_ONLY);
        }
    }

    #[test]
    fn a_device_with_full_control_drives_sessions() {
        let full = Some(DeviceAccess::Full);
        assert!(permit(&device(), full, needs(&Request::Kill { id: SESSION })).is_ok());
    }

    #[test]
    fn an_unpaired_device_may_not_even_watch() {
        assert_eq!(refusal(None, &Request::List), UNPAIRED);
    }
}
