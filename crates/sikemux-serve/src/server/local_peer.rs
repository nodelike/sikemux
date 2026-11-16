//! Which program is on the other end of a connection to the core's socket.
//! Agents run as the same user with a shell and can read any file the app
//! can, so no secret tells Sikemux's own window and notch from them; the
//! kernel's word on the peer's code does.
//!
//! On macOS a core signed by Sikemux's team trusts only peers its team signed
//! as the app or the notch. A build without that signature, such as a dev or
//! community build, cannot ask for it, so it trusts the app and notch at their
//! paths in the bundle or build it runs from. On Linux that path is all there is, and it is
//! weaker: a process can connect and then `exec` the app, keeping the
//! connection. The installed core trusts only the AppImage unpacked beside it, never one
//! running from its own mount, which any program could make. A dev build also trusts the
//! program that started it, such as a test.
//!
//! On Windows the path is all there is too. A process cannot become another program there, but
//! any process of the same user may write into another's memory, so a program that reaches into
//! the running app passes as the app; nothing on Windows tells the two apart.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sikemux_client::local::{Connection, Peer as Credentials};

use super::access::Peer;
use crate::host_update;

/// The window's executable, beside the core's own in a build.
const APP: &str = sikemux_process::names::APP_PROGRAM;
const BUNDLED_APP: &str = "Contents/MacOS/sikemux";
pub const BUNDLED_NOTCH: &str = "Contents/Helpers/Sikemux Notch.app";
pub const NOTCH_EXECUTABLE: &str = "Contents/MacOS/sikemux-notch";
/// The notch beside a dev build's own binary, which acts as its app.
const DEV_NOTCH: [&str; 2] = [
    "Sikemux Notch Dev.app",
    "Sikemux Notch Dev.app/Contents/MacOS/sikemux-notch",
];

struct Trust {
    team_signed: bool,
    /// Empty for a core its team signed, which never trusts a path.
    app_paths: Vec<PathBuf>,
}

static TRUST: OnceLock<Trust> = OnceLock::new();

/// Settles what this core trusts from its own code as it started, before an
/// update can replace the files it ran from.
pub fn settle() {
    trust();
}

fn trust() -> &'static Trust {
    TRUST.get_or_init(|| {
        let team_signed = platform::own_code_signed_by_team();
        let app_paths = match own_executable() {
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
    let Some(directory) = own.parent() else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    match sikemux_process::paths::app_bundle_of_core(own) {
        Some(bundle) => {
            let notch = bundle.join(BUNDLED_NOTCH);
            paths.extend([
                bundle.join(BUNDLED_APP),
                bundle.to_path_buf(),
                notch.join(NOTCH_EXECUTABLE),
                notch,
            ]);
        }
        None => paths.push(directory.join(APP)),
    }
    if own == Path::new(host_update::INSTALLED_CORE) {
        paths.push(Path::new(host_update::INSTALLED_APP).join(host_update::INSTALLED_APP_PROGRAM));
    }
    if cfg!(debug_assertions) {
        paths.extend(DEV_NOTCH.iter().map(|dev| directory.join(dev)));
        // Joined by part: a canonical Windows path takes no `/` as a separator.
        paths.push(
            directory
                .join("examples")
                .join(format!("remote{}", std::env::consts::EXE_SUFFIX)),
        );
    }
    paths
}

/// The process on the other end of a connection from this host.
pub fn pid(stream: &Connection) -> Option<u32> {
    sikemux_client::local::peer(stream).map(|credentials| credentials.pid())
}

pub async fn identify(stream: &Connection) -> Peer {
    let Some(credentials) = sikemux_client::local::peer(stream) else {
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

/// Whether this core tells its app by the code signature, which nothing on
/// this host can forge, rather than by where the program lives.
pub fn app_is_signed() -> bool {
    trust().team_signed
}

/// Root may do anything on this host already, so it acts as the app, unless
/// the core runs as root and its agents do too.
fn is_app(credentials: &Credentials) -> bool {
    let pid = credentials.pid();
    let trust = trust();
    if outranks_core(credentials)
        || pid == std::process::id()
        || cfg!(debug_assertions) && !trust.team_signed && Some(pid) == parent_id()
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

#[cfg(unix)]
fn outranks_core(credentials: &Credentials) -> bool {
    // SAFETY: geteuid takes nothing and cannot fail.
    let core_is_root = unsafe { libc::geteuid() } == 0;
    credentials.uid() == 0 && !core_is_root
}

/// Only this user can open the core's pipe, so no peer outranks the core.
#[cfg(windows)]
fn outranks_core(_credentials: &Credentials) -> bool {
    false
}

#[cfg(unix)]
fn parent_id() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}

#[cfg(windows)]
fn parent_id() -> Option<u32> {
    sikemux_process::procs::parent(std::process::id()).map(|(parent, _)| parent)
}

fn own_executable() -> Option<PathBuf> {
    sikemux_process::files::real_path(std::env::current_exe().ok()?).ok()
}

struct Examined {
    /// Signed by Sikemux's team as its app or notch.
    signed_app: bool,
    program: Option<PathBuf>,
}

#[cfg(target_os = "macos")]
mod platform {
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement,
    };

    use super::{Credentials, Examined};

    macro_rules! team {
        () => {
            r#"anchor apple generic and certificate leaf[subject.OU] = "D577WD6Z5U""#
        };
    }

    /// Signed by Sikemux's team as its app or its notch, never as the core
    /// agents run as their `sikemux` command.
    const SIGNED_APP: &str = concat!(
        team!(),
        r#" and (identifier "com.nodelike.sikemux" or identifier "com.nodelike.sikemux.notch")"#
    );

    pub fn own_code_signed_by_team() -> bool {
        let Ok(code) = SecCode::for_self(Flags::NONE) else {
            return false;
        };
        team!()
            .parse::<SecRequirement>()
            .is_ok_and(|requirement| code.check_validity(Flags::NONE, &requirement).is_ok())
    }

    pub fn examine(credentials: &Credentials) -> Examined {
        let token: Vec<u8> = credentials
            .audit_token()
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
            .then(|| {
                sikemux_process::files::real_path(code.path(Flags::NONE).ok()?.to_path()?).ok()
            })
            .flatten();
        Examined {
            signed_app,
            program,
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{Credentials, Examined};

    pub fn own_code_signed_by_team() -> bool {
        false
    }

    pub fn examine(credentials: &Credentials) -> Examined {
        Examined {
            signed_app: false,
            program: credentials
                .program()
                .and_then(|program| sikemux_process::files::real_path(program).ok()),
        }
    }
}
