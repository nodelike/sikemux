//! A named pipe in place of the socket. Pipes share one namespace across every user of the
//! machine, so the core's pipe lets only its own user in, refuses other machines, and a client
//! connects only to a pipe its own user owns: another user who took the name first gets nowhere.

use std::ffi::OsString;
use std::future::Future;
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions};
use tokio::runtime::Runtime;
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_PIPE_BUSY, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::Split;

pub type Stream = NamedPipeClient;
pub type Connection = NamedPipeServer;

const PIPE_PREFIX: &str = r"\\.\pipe\";
const BUSY_RETRY: Duration = Duration::from_millis(20);
/// How long a client waits while every instance of the pipe is taken.
const BUSY_LIMIT: Duration = Duration::from_secs(2);

/// The pipe a path names: the path itself when it is already a pipe's, else the path with its
/// backslashes turned to slashes under `\\.\pipe\`, since a pipe's name may hold no backslash.
fn pipe_name(path: &Path) -> OsString {
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with(PIPE_PREFIX) {
        return path.as_os_str().to_owned();
    }
    let mut name = OsString::from(PIPE_PREFIX);
    name.push(text.replace('\\', "/"));
    name
}

pub async fn connect(path: &Path) -> io::Result<Stream> {
    let name = pipe_name(path);
    let deadline = Instant::now() + BUSY_LIMIT;
    let client = loop {
        match ClientOptions::new().open(&name) {
            Ok(client) => break client,
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline => {}
            Err(error) => return Err(error),
        }
        tokio::time::sleep(BUSY_RETRY).await;
    };
    if !owned_by_this_user(client.as_raw_handle())? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "the pipe {} belongs to another user",
                name.to_string_lossy()
            ),
        ));
    }
    Ok(client)
}

impl Split for NamedPipeClient {
    type Read = ReadHalf<NamedPipeClient>;
    type Write = WriteHalf<NamedPipeClient>;

    fn split(self) -> (Self::Read, Self::Write) {
        tokio::io::split(self)
    }
}

impl Split for NamedPipeServer {
    type Read = ReadHalf<NamedPipeServer>;
    type Write = WriteHalf<NamedPipeServer>;

    fn split(self) -> (Self::Read, Self::Write) {
        tokio::io::split(self)
    }
}

/// A pipe has no blocking reads with a timeout, so this one runs the pipe on a runtime of its
/// own. Open it off any async runtime's own threads.
pub struct SyncStream {
    runtime: Runtime,
    pipe: NamedPipeClient,
    timeout: Duration,
}

async fn within<T>(timeout: Duration, work: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    tokio::time::timeout(timeout, work)
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

pub fn connect_sync(path: &Path, timeout: Duration) -> io::Result<SyncStream> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    let pipe = runtime.block_on(within(timeout, connect(path)))?;
    Ok(SyncStream {
        runtime,
        pipe,
        timeout,
    })
}

impl Read for SyncStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.runtime
            .block_on(within(self.timeout, self.pipe.read(buffer)))
    }
}

impl Write for SyncStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.runtime
            .block_on(within(self.timeout, self.pipe.write(bytes)))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The core's end of its pipe. Each client takes the waiting instance, and the listener opens
/// the next one as it does.
pub struct Listener {
    name: OsString,
    next: NamedPipeServer,
}

impl Listener {
    /// Fails when the pipe already exists, as another core's or another user's.
    pub fn bind(path: &Path) -> io::Result<Self> {
        let name = pipe_name(path);
        let next = create_instance(&name, true)?;
        Ok(Self { name, next })
    }

    pub async fn accept(&mut self) -> io::Result<Connection> {
        self.next.connect().await?;
        let waiting = create_instance(&self.name, false)?;
        Ok(std::mem::replace(&mut self.next, waiting))
    }
}

fn create_instance(name: &OsString, first: bool) -> io::Result<NamedPipeServer> {
    let descriptor = SecurityDescriptor::only_this_user()?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first).reject_remote_clients(true);
    // SAFETY: `attributes` is a valid SECURITY_ATTRIBUTES whose descriptor lives until
    // `descriptor` drops, after the pipe is created.
    unsafe {
        options.create_with_security_attributes_raw(
            name,
            std::ptr::from_mut(&mut attributes).cast(),
        )
    }
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    /// Owned by this user, who alone may read and write it.
    fn only_this_user() -> io::Result<Self> {
        let sid = UserSid::of_this_process()?.to_string()?;
        let text: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `text` ends in a nul, and the call writes one descriptor pointer, freed by
        // LocalFree when this drops.
        let made = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if made == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor came from LocalAlloc in the conversion above.
        unsafe { LocalFree(self.0) };
    }
}

/// The token of this process's user, which holds the user's SID.
struct UserSid(Vec<u64>);

impl UserSid {
    fn of_this_process() -> io::Result<Self> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo handle, and the call writes one handle.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut length = 0u32;
        // SAFETY: with no buffer, the call only writes the length it needs.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length) };
        let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
        // SAFETY: `buffer` is writable for `length` bytes and aligned for TOKEN_USER.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        };
        let error = io::Error::last_os_error();
        // SAFETY: `token` was opened above and is closed once.
        unsafe { CloseHandle(token) };
        if read == 0 {
            return Err(error);
        }
        Ok(Self(buffer))
    }

    fn sid(&self) -> PSID {
        // SAFETY: the buffer holds the TOKEN_USER GetTokenInformation wrote, whose SID points
        // into the same buffer.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn to_string(&self) -> io::Result<String> {
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: the SID is valid while `self` lives, and the call writes one string pointer,
        // freed below.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call wrote a nul-terminated string.
        let length = (0..).take_while(|&at| unsafe { *text.add(at) } != 0).count();
        // SAFETY: `text` holds `length` characters before its nul.
        let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
        // SAFETY: the string came from LocalAlloc in ConvertSidToStringSidW.
        unsafe { LocalFree(text.cast()) };
        Ok(sid)
    }
}

/// Only an administrator can make an object owned by someone else, so a pipe this user owns
/// is this user's or the machine's administrators'.
fn owned_by_this_user(pipe: std::os::windows::io::RawHandle) -> io::Result<bool> {
    let own = UserSid::of_this_process()?;
    let mut owner: PSID = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `pipe` is an open handle, and the call writes an owner pointer into the
    // descriptor it allocates, freed below.
    let failed = unsafe {
        GetSecurityInfo(
            pipe,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if failed != 0 {
        return Err(io::Error::from_raw_os_error(failed as i32));
    }
    // SAFETY: both SIDs are valid: the owner while the descriptor lives, ours while `own` does.
    let same = unsafe { EqualSid(owner, own.sid()) } != 0;
    // SAFETY: the descriptor came from LocalAlloc in GetSecurityInfo.
    unsafe { LocalFree(descriptor) };
    Ok(same)
}
