//! Who is on the other end of a connection, and what each request needs from
//! them. The app on this host may do anything. Any other program here, such as
//! the CLI or an agent's shell, may read and stop the core and set up a host
//! with no window, but not act as the person. A paired device does what it is
//! approved for at the moment it asks, and never touches the core itself. One
//! with full control that joined through this host's own account is the person,
//! as the app is: it lets devices in, links hosts, adds projects and agent
//! accounts, shares chats and commits, so a host with no window is run from it.

use crate::protocol::{DeviceAccess, Request};

use super::{CoreError, CoreResult};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Peer {
    /// Sikemux's own window or notch, over the core's socket, as the code
    /// signature of the program on the other end shows.
    App,
    /// Any other program on this host, over the core's socket.
    Local,
    Device {
        id: String,
    },
    /// A shared chat's link to its viewers, inside the core. It only reads
    /// the chat it shares, and is refused every request.
    Share {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Needs {
    Watch,
    Full,
    Person,
    Local,
}

/// What a paired device may do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Standing {
    pub access: DeviceAccess,
    /// It joined through the account this host is signed in to.
    pub owners: bool,
}

pub(crate) const LOCAL_ONLY: &str = "only Sikemux on this host can do that";
pub(crate) const APP_ONLY: &str = "only Sikemux's own window or notch can do that";
pub(crate) const WATCH_ONLY: &str =
    "this device can watch and answer permission requests, not drive sessions";
pub(crate) const UNPAIRED: &str = "this device is no longer paired with this host";
pub(crate) const OWNER_ONLY: &str =
    "only a device on this host's account with full control can do that";

pub(crate) fn needs(request: &Request) -> Needs {
    match request {
        Request::Call { method, .. } => super::calls::needs(method),
        Request::List
        | Request::Attach { .. }
        | Request::Detach { .. }
        | Request::TerminalLines { .. }
        | Request::TerminalText { .. }
        | Request::TerminalFind { .. }
        | Request::TerminalWaitFor { .. }
        | Request::TaskOutput { .. }
        | Request::AcpList
        | Request::AcpAttach { .. }
        | Request::AcpHistory { .. }
        | Request::AcpDetach { .. }
        | Request::AcpWake { .. }
        | Request::BackdropImage
        | Request::Unpair
        | Request::SetNotifications { .. }
        | Request::ClearNotifications
        | Request::SetForeground { .. }
        | Request::AcpPermissionReply { .. }
        | Request::Workspace
        | Request::Attentions
        | Request::Host => Needs::Watch,
        Request::Spawn { .. }
        | Request::Resize { .. }
        | Request::Kill { .. }
        | Request::ResetModes { .. }
        | Request::TerminalInput { .. }
        | Request::TerminalClear { .. }
        | Request::ShellAtPrompt { .. }
        | Request::AcpStart { .. }
        | Request::AcpPrompt { .. }
        | Request::AcpEdit { .. }
        | Request::AcpSteer { .. }
        | Request::AttachFile { .. }
        | Request::AcpCancel { .. }
        | Request::AcpStopTask { .. }
        | Request::AcpStop { .. }
        | Request::AcpSetPermissionMode { .. }
        | Request::AcpSetConfig { .. }
        | Request::StartChat { .. }
        | Request::ResumeChat { .. }
        | Request::AcpSwitchAccount { .. } => Needs::Full,
        Request::Configure { .. }
        | Request::ListManifests
        | Request::ReloadManifests
        | Request::ExplainAgentDetection { .. }
        | Request::StopAll
        | Request::RegisterWindow
        | Request::PluginsChanged
        | Request::Shutdown { .. }
        | Request::RemoteStatus
        | Request::SetRemoteAccess { .. }
        | Request::SetDeviceAccess { .. }
        | Request::RevokeDevice { .. }
        | Request::AnswerPairing { .. }
        | Request::AllowDevices { .. }
        | Request::SignRegistration { .. }
        | Request::SetOwner { .. }
        | Request::PublishOnScreen { .. }
        | Request::WatchView
        | Request::FocusAgent { .. }
        | Request::PublishBackdrop { .. } => Needs::Local,
    }
}

/// What this host has set up, which decides what a program other than the app
/// may still do to set it up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Setup {
    pub signed_in: bool,
    pub paired: bool,
    /// The core tells its app by code signature, so the app is always there
    /// to set the host up and nothing else needs to.
    pub app_signed: bool,
}

/// Whether only Sikemux's own window or notch may send `request` from this
/// host. A host with no window is set up from a terminal, which cannot be told
/// from an agent's shell: `sikemux login` while no account owns it,
/// `logout`, and `pair` until its first device is in. A host signed in once
/// stays its account's until the app, root or the account moves it.
pub(crate) fn for_person(request: &Request, setup: Setup) -> bool {
    match request {
        Request::Call { method, .. } => super::calls::for_person(method),
        Request::SetOwner { .. }
        | Request::SignRegistration { .. }
        | Request::AnswerPairing { .. }
            if setup.app_signed =>
        {
            true
        }
        Request::SetOwner { owner: None } => false,
        Request::SetOwner { owner: Some(_) } | Request::SignRegistration { .. } => setup.signed_in,
        Request::AnswerPairing { .. } => setup.paired,
        Request::List
        | Request::Attach { .. }
        | Request::Detach { .. }
        | Request::TerminalLines { .. }
        | Request::TerminalText { .. }
        | Request::TerminalFind { .. }
        | Request::TerminalWaitFor { .. }
        | Request::TaskOutput { .. }
        | Request::ShellAtPrompt { .. }
        | Request::AcpList
        | Request::AcpAttach { .. }
        | Request::AcpHistory { .. }
        | Request::AcpDetach { .. }
        | Request::BackdropImage
        | Request::Workspace
        | Request::Attentions
        | Request::Host
        | Request::WatchView
        | Request::RemoteStatus
        | Request::ListManifests
        | Request::ExplainAgentDetection { .. }
        | Request::StopAll
        | Request::Shutdown { .. }
        | Request::Unpair
        | Request::SetNotifications { .. }
        | Request::ClearNotifications
        | Request::SetForeground { .. } => false,
        Request::Spawn { .. }
        | Request::Resize { .. }
        | Request::Kill { .. }
        | Request::ResetModes { .. }
        | Request::TerminalInput { .. }
        | Request::TerminalClear { .. }
        | Request::AcpStart { .. }
        | Request::AcpPrompt { .. }
        | Request::AcpEdit { .. }
        | Request::AcpSteer { .. }
        | Request::AttachFile { .. }
        | Request::AcpCancel { .. }
        | Request::AcpStopTask { .. }
        | Request::AcpStop { .. }
        | Request::AcpSetPermissionMode { .. }
        | Request::AcpSetConfig { .. }
        | Request::AcpWake { .. }
        | Request::AcpPermissionReply { .. }
        | Request::AcpSwitchAccount { .. }
        | Request::StartChat { .. }
        | Request::ResumeChat { .. }
        | Request::Configure { .. }
        | Request::ReloadManifests
        | Request::RegisterWindow
        | Request::PluginsChanged
        | Request::FocusAgent { .. }
        | Request::SetRemoteAccess { .. }
        | Request::SetDeviceAccess { .. }
        | Request::RevokeDevice { .. }
        | Request::AllowDevices { .. }
        | Request::PublishOnScreen { .. }
        | Request::PublishBackdrop { .. } => true,
    }
}

impl Peer {
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Peer::App | Peer::Local)
    }

    pub(crate) fn device_id(&self) -> Option<String> {
        match self {
            Peer::App | Peer::Local | Peer::Share { .. } => None,
            Peer::Device { id } => Some(id.clone()),
        }
    }

    pub(crate) fn is_device(&self, id: &str) -> bool {
        matches!(self, Peer::Device { id: own } if own == id)
    }
}

/// `standing` is what the device may do now: `None` once it is unpaired.
pub(crate) fn permit(peer: &Peer, standing: Option<Standing>, needs: Needs) -> CoreResult<()> {
    if peer.is_local() {
        return Ok(());
    }
    let Some(Standing { access, owners }) = standing else {
        return Err(CoreError::from(UNPAIRED));
    };
    match needs {
        Needs::Local => Err(CoreError::from(LOCAL_ONLY)),
        Needs::Person if !owners || access != DeviceAccess::Full => {
            Err(CoreError::from(OWNER_ONLY))
        }
        Needs::Full if access == DeviceAccess::Watch => Err(CoreError::from(WATCH_ONLY)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AllowedDevice, SessionId};

    const SESSION: SessionId = 1;

    fn device() -> Peer {
        Peer::Device { id: "phone".into() }
    }

    fn standing(access: DeviceAccess) -> Option<Standing> {
        Some(Standing {
            access,
            owners: false,
        })
    }

    fn refusal(standing: Option<Standing>, request: &Request) -> String {
        permit(&device(), standing, needs(request))
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
            assert!(permit(&Peer::App, None, needs(&request)).is_ok());
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
            Request::AnswerPairing {
                id: "request".into(),
                allow: true,
                access: DeviceAccess::Full,
            },
            Request::AllowDevices {
                devices: vec![AllowedDevice {
                    id: "f".repeat(64),
                    name: "Attacker".into(),
                    platform: "ios".into(),
                    access: DeviceAccess::Full,
                }],
            },
            Request::SignRegistration {
                nonce: "a".repeat(64),
                user_id: "user_attacker".into(),
            },
            Request::SetOwner {
                owner: Some("user_attacker".into()),
            },
            Request::PublishOnScreen {
                agent_ids: Vec::new(),
                focused: true,
            },
        ] {
            assert_eq!(refusal(standing(DeviceAccess::Full), &request), LOCAL_ONLY);
        }
    }

    #[test]
    fn a_watching_device_reads_and_answers_permissions_but_does_not_drive() {
        let permission = Request::AcpPermissionReply {
            agent_id: "a".into(),
            request_id: "r".into(),
            option_id: None,
        };
        let watch = standing(DeviceAccess::Watch);
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
            Request::AttachFile {
                agent_id: "a".into(),
                name: "shot.jpg".into(),
                mime: "image/jpeg".into(),
                data: String::new(),
            },
            Request::ResumeChat {
                recent: "claude:s1".into(),
                permission_mode: None,
                model: None,
                effort: None,
            },
        ] {
            assert_eq!(refusal(watch, &request), WATCH_ONLY);
        }
    }

    #[test]
    fn a_device_with_full_control_drives_sessions() {
        let full = standing(DeviceAccess::Full);
        assert!(permit(&device(), full, needs(&Request::Kill { id: SESSION })).is_ok());
    }

    #[test]
    fn an_unpaired_device_may_not_even_watch() {
        assert_eq!(refusal(None, &Request::List), UNPAIRED);
    }
}
