//! The core's local socket on Windows: a named pipe only this user can open.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

/// Full control for the pipe's owner and for the system, and for no one else.
const OWNER_AND_SYSTEM: &str = "D:P(A;;GA;;;OW)(A;;GA;;;SY)";

pub(crate) struct PipeListener {
    name: String,
    next: NamedPipeServer,
}

impl PipeListener {
    /// Claims the pipe of the core at `socket`. Only one core can: a second one is told
    /// the pipe is taken.
    pub(crate) fn bind(socket: &Path) -> io::Result<Self> {
        let name = sikemux_client::local::pipe_name(socket);
        let next = create(&name, true)?;
        Ok(Self { name, next })
    }

    pub(crate) async fn accept(&mut self) -> io::Result<(NamedPipeServer, ())> {
        self.next.connect().await?;
        let waiting = create(&self.name, false)?;
        Ok((std::mem::replace(&mut self.next, waiting), ()))
    }
}

fn create(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    let wide: Vec<u16> = OWNER_AND_SYSTEM.encode_utf16().chain([0]).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `wide` ends in a zero, and on success `descriptor` is a buffer the call
    // allocated, freed below once the pipe has copied what it needs.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first).reject_remote_clients(true);
    // SAFETY: `attributes` points at a valid descriptor for the length of the call.
    let pipe = unsafe {
        options.create_with_security_attributes_raw(name, std::ptr::from_mut(&mut attributes).cast())
    };
    // SAFETY: `descriptor` came from the conversion above and is freed once.
    unsafe { LocalFree(descriptor.cast()) };
    pipe
}

/// The process on the other end of `pipe`.
pub(crate) fn client_pid(pipe: &NamedPipeServer) -> Option<u32> {
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

    let mut pid = 0u32;
    // SAFETY: the handle belongs to `pipe`, which outlives the call, and `pid` is a live u32.
    let found = unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid) };
    (found != 0 && pid != 0).then_some(pid)
}
