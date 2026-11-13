//! Who is on the other end of a connection, and what each request needs from
//! them. The app on this host may do anything. Any other program here, such as
//! the CLI or an agent's shell, may read and stop the core and set up a host
//! with no window, but not act as the person. A paired device does what it is
//! approved for at the moment it asks, and never touches the core itself. One
//! with full control that joined through this host's own account is the person,
//! as the app is: it lets devices in, links hosts, adds projects and agent
//! accounts, shares chats and commits, so a host with no window is run from it.

use sikemux_wire::protocol::{ask, DeviceAccess, Request};

use super::{CoreError, CoreResult};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Peer {
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
pub enum Needs {
    Watch,
    Full,
    /// Full control, and joined through this host's own account.
    Owner,
    /// Not for any paired device: only programs on this host.
    NoDevice,
}

/// What a paired device may do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standing {
    pub access: DeviceAccess,
    /// It joined through the account this host is signed in to.
    pub owners: bool,
}

pub const LOCAL_ONLY: &str = "only Sikemux on this host can do that";
pub const APP_ONLY: &str = "only Sikemux's own window or notch can do that";
pub const WATCH_ONLY: &str =
    "this device can watch and answer permission requests, not drive sessions";
pub const UNPAIRED: &str = "this device is no longer paired with this host";
pub const OWNER_ONLY: &str = "only a device on this host's account with full control can do that";

/// What this host has set up, which decides what a program other than the app
/// may still do to set it up.
#[derive(Clone, Copy, Debug)]
pub struct Setup {
    pub signed_in: bool,
    pub paired: bool,
    /// The core tells its app by code signature, so the app is always there
    /// to set the host up and nothing else needs to.
    pub app_signed: bool,
}

/// Who may send a request: what a paired device needs for it, and which programs on this
/// host may send it.
pub struct Rule {
    pub needs: Needs,
    program: Program,
}

/// Which programs on this host may send a request.
#[derive(Clone, Copy)]
pub enum Program {
    Anyone,
    /// Only Sikemux's own window or notch.
    App,
    /// Only the app once this host is set up as far as the check asks.
    AppWhen(fn(Setup) -> bool),
}

/// The rule for `request`. A host with no window is set up from a terminal,
/// which cannot be told from an agent's shell: `sikemux login` while no account
/// owns it, `logout`, and `pair` until its first device is in. A host signed in
/// once stays its account's until the app, root or the account moves it.
pub fn rule(request: &Request, row: fn(&str) -> (Needs, Program)) -> Rule {
    let (needs, program) = match request {
        Request::TerminalInput(_) => return Rule::TERMINAL_INPUT,
        Request::Call(ask::Call { method, .. }) => row(method),
        Request::SetOwner(ask::SetOwner { owner: None }) => {
            (Needs::NoDevice, Program::AppWhen(|setup| setup.app_signed))
        }
        Request::SetOwner(ask::SetOwner { owner: Some(_) }) | Request::SignRegistration(_) => (
            Needs::NoDevice,
            Program::AppWhen(|setup| setup.app_signed || setup.signed_in),
        ),
        Request::AnswerPairing(_) => (
            Needs::NoDevice,
            Program::AppWhen(|setup| setup.app_signed || setup.paired),
        ),
        Request::List
        | Request::Attach(_)
        | Request::Detach(_)
        | Request::TerminalLines(_)
        | Request::TerminalText(_)
        | Request::TerminalFind(_)
        | Request::AcpList
        | Request::AcpAttach(_)
        | Request::AcpHistory(_)
        | Request::AcpDetach(_)
        | Request::BackdropImage
        | Request::Unpair
        | Request::SetNotifications(_)
        | Request::ClearNotifications
        | Request::SetForeground(_)
        | Request::Host => (Needs::Watch, Program::Anyone),
        Request::AcpWake(_) | Request::AcpPermissionReply(_) => (Needs::Watch, Program::App),
        Request::Spawn(_)
        | Request::Resize(_)
        | Request::Kill(_)
        | Request::ResetModes(_)
        | Request::TerminalClear(_)
        | Request::AcpStart(_)
        | Request::AcpPrompt(_)
        | Request::AcpEdit(_)
        | Request::AcpSteer(_)
        | Request::AttachFile(_)
        | Request::AcpCancel(_)
        | Request::AcpStopTask(_)
        | Request::AcpStop(_)
        | Request::AcpSetPermissionMode(_)
        | Request::AcpSetConfig(_)
        | Request::StartChat(_)
        | Request::ResumeChat(_)
        | Request::AcpSwitchAccount(_) => (Needs::Full, Program::App),
        Request::Shutdown(_) | Request::RemoteStatus | Request::WatchView => {
            (Needs::NoDevice, Program::Anyone)
        }
        Request::RegisterWindow
        | Request::SetRemoteAccess(_)
        | Request::SetDeviceAccess(_)
        | Request::RevokeDevice(_)
        | Request::AllowDevices(_)
        | Request::PublishOnScreen(_)
        | Request::FocusAgent(_) => (Needs::NoDevice, Program::App),
    };
    Rule { needs, program }
}

impl Rule {
    /// What typing into a terminal takes, by a request or a frame of input.
    pub const TERMINAL_INPUT: Rule = Rule {
        needs: Needs::Full,
        program: Program::App,
    };

    /// Whether only the app may send the request from this host, set up as `setup` says.
    pub fn only_the_app(&self, setup: Setup) -> bool {
        match self.program {
            Program::Anyone => false,
            Program::App => true,
            Program::AppWhen(set_up) => set_up(setup),
        }
    }
}

impl Peer {
    pub fn is_local(&self) -> bool {
        matches!(self, Peer::App | Peer::Local)
    }

    pub fn device_id(&self) -> Option<String> {
        match self {
            Peer::App | Peer::Local | Peer::Share { .. } => None,
            Peer::Device { id } => Some(id.clone()),
        }
    }

    pub fn is_device(&self, id: &str) -> bool {
        matches!(self, Peer::Device { id: own } if own == id)
    }
}

/// `standing` is what the device may do now: `None` once it is unpaired.
pub fn permit(peer: &Peer, standing: Option<Standing>, needs: Needs) -> CoreResult<()> {
    if peer.is_local() {
        return Ok(());
    }
    let Some(Standing { access, owners }) = standing else {
        return Err(CoreError::from(UNPAIRED));
    };
    match needs {
        Needs::NoDevice => Err(CoreError::from(LOCAL_ONLY)),
        Needs::Owner if !owners || access != DeviceAccess::Full => Err(CoreError::from(OWNER_ONLY)),
        Needs::Full if access == DeviceAccess::Watch => Err(CoreError::from(WATCH_ONLY)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    fn table(method: &str) -> (Needs, Program) {
        unreachable!("no request here is the call {method}")
    }

    use super::*;
    use sikemux_wire::protocol::{AllowedDevice, SessionId};

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
        permit(&device(), standing, rule(request, table).needs)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn the_app_may_send_anything() {
        for request in [
            Request::Shutdown(ask::Shutdown { stop_all: true }),
            Request::RegisterWindow,
            Request::Kill(ask::Kill { id: SESSION }),
        ] {
            assert!(permit(&Peer::App, None, rule(&request, table).needs).is_ok());
        }
    }

    #[test]
    fn no_device_reaches_the_core_itself() {
        for request in [
            Request::Shutdown(ask::Shutdown { stop_all: true }),
            Request::RegisterWindow,
            Request::SetRemoteAccess(ask::SetRemoteAccess { enabled: false }),
            Request::RevokeDevice(ask::RevokeDevice { id: "other".into() }),
            Request::AnswerPairing(ask::AnswerPairing {
                id: "request".into(),
                allow: true,
                access: DeviceAccess::Full,
            }),
            Request::AllowDevices(ask::AllowDevices {
                devices: vec![AllowedDevice {
                    id: "f".repeat(64),
                    name: "Attacker".into(),
                    platform: "ios".into(),
                    access: DeviceAccess::Full,
                }],
            }),
            Request::SignRegistration(ask::SignRegistration {
                nonce: "a".repeat(64),
                user_id: "user_attacker".into(),
            }),
            Request::SetOwner(ask::SetOwner {
                owner: Some("user_attacker".into()),
            }),
            Request::PublishOnScreen(ask::PublishOnScreen {
                agent_ids: Vec::new(),
                focused: true,
            }),
        ] {
            assert_eq!(refusal(standing(DeviceAccess::Full), &request), LOCAL_ONLY);
        }
    }

    #[test]
    fn a_watching_device_reads_and_answers_permissions_but_does_not_drive() {
        let permission = Request::AcpPermissionReply(ask::AcpPermissionReply {
            agent_id: "a".into(),
            request_id: "r".into(),
            option_id: None,
        });
        let watch = standing(DeviceAccess::Watch);
        assert!(permit(&device(), watch, rule(&permission, table).needs).is_ok());
        assert!(permit(
            &device(),
            watch,
            rule(&Request::Attach(ask::Attach { id: SESSION }), table).needs
        )
        .is_ok());
        for request in [
            Request::Kill(ask::Kill { id: SESSION }),
            Request::Resize(ask::Resize {
                id: SESSION,
                cols: 80,
                rows: 24,
            }),
            Request::AcpCancel(ask::AcpCancel {
                agent_id: "a".into(),
            }),
            Request::AttachFile(ask::AttachFile {
                agent_id: "a".into(),
                name: "shot.jpg".into(),
                mime: "image/jpeg".into(),
                data: String::new(),
            }),
            Request::ResumeChat(ask::ResumeChat {
                recent: "claude:s1".into(),
                permission_mode: None,
                model: None,
                effort: None,
            }),
        ] {
            assert_eq!(refusal(watch, &request), WATCH_ONLY);
        }
    }

    #[test]
    fn a_device_with_full_control_drives_sessions() {
        let full = standing(DeviceAccess::Full);
        assert!(permit(
            &device(),
            full,
            rule(&Request::Kill(ask::Kill { id: SESSION }), table).needs
        )
        .is_ok());
    }

    #[test]
    fn an_unpaired_device_may_not_even_watch() {
        assert_eq!(refusal(None, &Request::List), UNPAIRED);
    }
}
