//! Which program is on the other end of a connection to the core's socket.
//! Agents run as the same user with a shell and can read any file the app
//! can, so no secret tells Sikemux's own window and notch from them; the
//! kernel's word on the peer's code does.
//!
//! On macOS a core signed by Sikemux's team trusts only peers its team signed
//! as the app or the notch. A build without that signature, such as a dev or
//! community build, cannot ask for it, so it trusts the app and notch at their
//! paths beside its own binary. On Linux that path is all there is, and it is
//! weaker: a process can connect and then `exec` the app, keeping the
//! connection. A dev build also trusts the program that started it, such as a
//! test.

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::net::UnixStream;

use super::access::Peer;

/// The window's executable, beside the core's own in the app or the build.
const APP: &str = "sikemux";
/// The notch inside a release bundle; the core runs from `Contents/MacOS`.
const BUNDLED_NOTCH: &str = "Contents/Helpers/Sikemux Notch.app";
const NOTCH_EXECUTABLE: &str = "Contents/MacOS/sikemux-notch";
/// The programs beside a dev build's own binary that act as its app.
const DEV_PROGRAMS: [&str; 3] = [
    "Sikemux Notch Dev.app",
    "Sikemux Notch Dev.app/Contents/MacOS/sikemux-notch",
    "examples/remote",
];

struct Trust {
    team_signed: bool,
    /// Empty for a core its team signed, which never trusts a path.
    app_paths: Vec<PathBuf>,
}

static TRUST: OnceLock<Trust> = OnceLock::new();

/// Settles what this core trusts from its own code as it started, before an
/// update can replace the files it ran from.
pub(crate) fn settle() {
    trust();
}

fn trust() -> &'static Trust {
    TRUST.get_or_init(|| {
        let team_signed = platform::own_code_signed_by_team();
        let app_paths = match own_directory() {
            Some(own) if !team_signed => app_paths_beside(&own),
            _ => Vec::new(),
        };
        Trust {
            team_signed,
            app_paths,
        }
    })
}

fn app_paths_beside(own: &Path) -> Vec<PathBuf> {
    let mut paths = vec![own.join(APP)];
    if let Some(bundle) = own
        .ends_with("Contents/MacOS")
        .then(|| own.parent()?.parent())
        .flatten()
    {
        let notch = bundle.join(BUNDLED_NOTCH);
        paths.extend([bundle.to_path_buf(), notch.join(NOTCH_EXECUTABLE), notch]);
    }
    if cfg!(debug_assertions) {
        paths.extend(DEV_PROGRAMS.iter().map(|dev| own.join(dev)));
    }
    paths
}

pub(crate) async fn identify(stream: &UnixStream) -> Peer {
    let Some(credentials) = platform::credentials(stream.as_raw_fd()) else {
        return Peer::Local;
    };
    let app = tokio::task::spawn_blocking(move || is_app(&credentials))
        .await
        .unwrap_or(false);
    if app {
        Peer::App
    } else {
        Peer::Local
    }
}

fn is_app(credentials: &platform::Credentials) -> bool {
    let pid = credentials.pid();
    let trust = trust();
    if pid == std::process::id()
        || cfg!(debug_assertions)
            && !trust.team_signed
            && pid == std::os::unix::process::parent_id()
    {
        return true;
    }
    let examined = platform::examine(credentials);
    if examined.signed_app {
        return true;
    }
    examined
        .program
        .is_some_and(|program| trust.app_paths.contains(&program))
}

fn own_directory() -> Option<PathBuf> {
    let own = std::env::current_exe().ok()?.canonicalize().ok()?;
    own.parent().map(Path::to_path_buf)
}

struct Examined {
    /// Signed by Sikemux's team as its app or notch.
    signed_app: bool,
    program: Option<PathBuf>,
}

#[cfg(target_os = "macos")]
mod platform {
    use std::os::fd::RawFd;

    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement,
    };

    use super::Examined;

    macro_rules! team {
        () => {
            r#"anchor apple generic and certificate leaf[subject.OU] = "D577WD6Z5U""#
        };
    }

    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERTOKEN: libc::c_int = 6;
    const TOKEN_PID: usize = 5;

    /// Signed by Sikemux's team as its app or its notch. `sikemux-editor`, the
    /// command agents run, carries the app's identifier too, but no bundle
    /// naming it the executable.
    const SIGNED_APP: &str = concat!(
        team!(),
        r#" and (identifier "com.nodelike.sikemux" and info[CFBundleExecutable] = "sikemux" or identifier "com.nodelike.sikemux.notch")"#
    );

    pub(super) fn own_code_signed_by_team() -> bool {
        let Ok(code) = SecCode::for_self(Flags::NONE) else {
            return false;
        };
        team!()
            .parse::<SecRequirement>()
            .is_ok_and(|requirement| code.check_validity(Flags::NONE, &requirement).is_ok())
    }

    /// The peer's audit token, which names the process as it was when it
    /// connected, so one that has since run another program no longer matches.
    pub(super) struct Credentials([u32; 8]);

    impl Credentials {
        pub(super) fn pid(&self) -> u32 {
            self.0.get(TOKEN_PID).copied().unwrap_or_default()
        }
    }

    pub(super) fn credentials(fd: RawFd) -> Option<Credentials> {
        let mut token = [0u32; 8];
        let mut length = std::mem::size_of_val(&token) as libc::socklen_t;
        // SAFETY: `token` is a writable buffer of `length` bytes, and the
        // kernel writes at most that many.
        let read = unsafe {
            libc::getsockopt(
                fd,
                SOL_LOCAL,
                LOCAL_PEERTOKEN,
                token.as_mut_ptr().cast(),
                &mut length,
            )
        };
        (read == 0 && length as usize == std::mem::size_of_val(&token))
            .then_some(Credentials(token))
    }

    pub(super) fn examine(credentials: &Credentials) -> Examined {
        let token: Vec<u8> = credentials
            .0
            .iter()
            .flat_map(|word| word.to_ne_bytes())
            .collect();
        let token = CFData::from_buffer(&token);
        let mut attributes = GuestAttributes::new();
        attributes.set_audit_token(token.as_concrete_TypeRef());
        let Ok(code) = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE) else {
            return Examined {
                signed_app: false,
                program: None,
            };
        };
        let signed_app = SIGNED_APP
            .parse::<SecRequirement>()
            .is_ok_and(|requirement| code.check_validity(Flags::NONE, &requirement).is_ok());
        let program = (!signed_app)
            .then(|| code.path(Flags::NONE).ok()?.to_path()?.canonicalize().ok())
            .flatten();
        Examined {
            signed_app,
            program,
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::os::fd::RawFd;

    use super::Examined;

    pub(super) struct Credentials(u32);

    impl Credentials {
        pub(super) fn pid(&self) -> u32 {
            self.0
        }
    }

    pub(super) fn credentials(fd: RawFd) -> Option<Credentials> {
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
        // SAFETY: `credentials` is a writable `ucred` of `length` bytes, and
        // the kernel writes at most that many.
        let read = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                std::ptr::from_mut(&mut credentials).cast(),
                &mut length,
            )
        };
        (read == 0 && credentials.pid > 0).then(|| Credentials(credentials.pid.unsigned_abs()))
    }

    pub(super) fn own_code_signed_by_team() -> bool {
        false
    }

    pub(super) fn examine(credentials: &Credentials) -> Examined {
        Examined {
            signed_app: false,
            program: std::fs::read_link(format!("/proc/{}/exe", credentials.0)).ok(),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use std::os::fd::RawFd;

    use super::Examined;

    pub(super) struct Credentials;

    impl Credentials {
        pub(super) fn pid(&self) -> u32 {
            0
        }
    }

    pub(super) fn credentials(_fd: RawFd) -> Option<Credentials> {
        None
    }

    pub(super) fn own_code_signed_by_team() -> bool {
        false
    }

    pub(super) fn examine(_credentials: &Credentials) -> Examined {
        Examined {
            signed_app: false,
            program: None,
        }
    }
}
