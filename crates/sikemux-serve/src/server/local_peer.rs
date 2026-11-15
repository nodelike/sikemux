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

use sikemux_client::local::Connection;

use super::access::Peer;
use crate::host_update;

/// The window's executable, beside the core's own in a build.
const APP: &str = sikemux_process::names::APP_PROGRAM;
const BUNDLED_APP: &str = "Contents/MacOS/sikemux";
pub const BUNDLED_NOTCH: &str = "Contents/Helpers/Sikemux Notch.app";
pub const NOTCH_EXECUTABLE: &str = "Contents/MacOS/sikemux-notch";
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
        paths.extend(DEV_PROGRAMS.iter().map(|dev| directory.join(dev)));
    }
    paths
}

/// The process on the other end of a connection from this host.
pub fn pid(stream: &Connection) -> Option<u32> {
    platform::credentials(stream).map(|credentials| credentials.pid())
}

pub async fn identify(stream: &Connection) -> Peer {
    let Some(credentials) = platform::credentials(stream) else {
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
fn is_app(credentials: &platform::Credentials) -> bool {
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
fn outranks_core(credentials: &platform::Credentials) -> bool {
    // SAFETY: geteuid takes nothing and cannot fail.
    let core_is_root = unsafe { libc::geteuid() } == 0;
    credentials.uid() == 0 && !core_is_root
}

/// Only this user can open the core's pipe, so no peer outranks the core.
#[cfg(windows)]
fn outranks_core(_credentials: &platform::Credentials) -> bool {
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
    std::env::current_exe().ok()?.canonicalize().ok()
}

struct Examined {
    /// Signed by Sikemux's team as its app or notch.
    signed_app: bool,
    program: Option<PathBuf>,
}

#[cfg(target_os = "macos")]
mod platform {
    use std::os::fd::AsRawFd;

    use sikemux_client::local::Connection;

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
    const TOKEN_EUID: usize = 1;
    const TOKEN_PID: usize = 5;

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

    /// The peer's audit token, which names the process as it was when it
    /// connected, so one that has since run another program no longer matches.
    pub struct Credentials([u32; 8]);

    impl Credentials {
        pub fn pid(&self) -> u32 {
            self.0.get(TOKEN_PID).copied().unwrap_or_default()
        }

        pub fn uid(&self) -> u32 {
            self.0.get(TOKEN_EUID).copied().unwrap_or(u32::MAX)
        }
    }

    pub fn credentials(stream: &Connection) -> Option<Credentials> {
        let fd = stream.as_raw_fd();
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

    pub fn examine(credentials: &Credentials) -> Examined {
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
    use std::os::fd::AsRawFd;

    use sikemux_client::local::Connection;

    use super::Examined;

    pub struct Credentials {
        pid: u32,
        uid: u32,
    }

    impl Credentials {
        pub fn pid(&self) -> u32 {
            self.pid
        }

        pub fn uid(&self) -> u32 {
            self.uid
        }
    }

    pub fn credentials(stream: &Connection) -> Option<Credentials> {
        let fd = stream.as_raw_fd();
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
        (read == 0 && credentials.pid > 0).then(|| Credentials {
            pid: credentials.pid.unsigned_abs(),
            uid: credentials.uid,
        })
    }

    pub fn own_code_signed_by_team() -> bool {
        false
    }

    pub fn examine(credentials: &Credentials) -> Examined {
        Examined {
            signed_app: false,
            program: std::fs::read_link(format!("/proc/{}/exe", credentials.pid)).ok(),
        }
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
mod platform {
    use sikemux_client::local::Connection;

    use super::Examined;

    pub struct Credentials;

    impl Credentials {
        pub fn pid(&self) -> u32 {
            0
        }

        pub fn uid(&self) -> u32 {
            u32::MAX
        }
    }

    pub fn credentials(_stream: &Connection) -> Option<Credentials> {
        None
    }

    pub fn own_code_signed_by_team() -> bool {
        false
    }

    pub fn examine(_credentials: &Credentials) -> Examined {
        Examined {
            signed_app: false,
            program: None,
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;

    use sikemux_client::local::Connection;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::Examined;

    /// The pipe names its client by pid alone, so this keeps when the connection was taken
    /// up: a process started after that has only been given the pid of one that left.
    pub struct Credentials {
        pid: u32,
        connected: u64,
    }

    impl Credentials {
        pub fn pid(&self) -> u32 {
            self.pid
        }
    }

    fn ticks(time: FILETIME) -> u64 {
        u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)
    }

    pub fn credentials(stream: &Connection) -> Option<Credentials> {
        let mut pid = 0u32;
        // SAFETY: the pipe's handle is open while `stream` lives, and the call writes one u32.
        let read = unsafe { GetNamedPipeClientProcessId(stream.as_raw_handle(), &mut pid) };
        let mut now = FILETIME::default();
        // SAFETY: the call writes one FILETIME.
        unsafe { GetSystemTimeAsFileTime(&mut now) };
        (read != 0 && pid != 0).then(|| Credentials {
            pid,
            connected: ticks(now),
        })
    }

    pub fn own_code_signed_by_team() -> bool {
        false
    }

    pub fn examine(credentials: &Credentials) -> Examined {
        Examined {
            signed_app: false,
            program: program(credentials),
        }
    }

    fn program(credentials: &Credentials) -> Option<PathBuf> {
        // SAFETY: OpenProcess takes plain values; the handle is closed below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, credentials.pid) };
        if process.is_null() {
            return None;
        }
        let mut times = [FILETIME::default(); 4];
        let [created, exited, kernel, user] = &mut times;
        // SAFETY: `process` is open, and the call writes four FILETIMEs.
        let timed = unsafe { GetProcessTimes(process, created, exited, kernel, user) } != 0;
        let mut path = vec![0u16; 32 * 1024];
        let mut length = path.len() as u32;
        // SAFETY: `path` is writable for `length` characters, and the call writes at most
        // that many and the length it wrote.
        let named = unsafe {
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut length)
        } != 0;
        // SAFETY: `process` was opened above and is closed once.
        unsafe { CloseHandle(process) };
        if !timed || ticks(times[0]) > credentials.connected || !named {
            return None;
        }
        path.truncate(length as usize);
        PathBuf::from(OsString::from_wide(&path))
            .canonicalize()
            .ok()
    }
}
