use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{RevertToSelf, TOKEN_ALL_ACCESS};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_WRITE_DATA, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    SYNCHRONIZE,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, ImpersonateNamedPipeClient,
    WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use crate::descriptor::{wide, Descriptor};
use crate::token::Token;

/// The service's pipe. Release and dev builds each run their own service.
pub fn name() -> String {
    format!(r"\\.\pipe\{}", crate::service::NAME)
}

/// Only SYSTEM may serve the pipe. Anyone signed in may read and write it, but `0x12008b` leaves
/// out the append-data right, which on a pipe means "create another instance".
const PIPE_ACCESS: &str = "D:P(A;;GA;;;SY)(A;;0x12008b;;;AU)";
/// What a client opens the pipe with: data, never `GENERIC_WRITE`, which includes the
/// create-instance right the pipe refuses.
const CLIENT_ACCESS: u32 = FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const CONNECT_WAIT: Duration = Duration::from_secs(5);
const LONGEST_MESSAGE: usize = 64 * 1024;

pub struct Connection(OwnedHandle);

/// A pipe instance waiting for its next client. The first is made with
/// `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a program that took the name first stops the service
/// rather than answering in its place.
pub struct Listener {
    first: bool,
    descriptor: Descriptor,
}

impl Listener {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            first: true,
            descriptor: Descriptor::parse(PIPE_ACCESS)?,
        })
    }

    pub fn accept(&mut self) -> io::Result<Connection> {
        let attributes = self.descriptor.attributes();
        let first = match std::mem::replace(&mut self.first, false) {
            true => FILE_FLAG_FIRST_PIPE_INSTANCE,
            false => 0,
        };
        // SAFETY: the name ends in a nul and `attributes` points at a descriptor that lives
        // through the call.
        let pipe = unsafe {
            CreateNamedPipeW(
                wide(&name()).as_ptr(),
                PIPE_ACCESS_DUPLEX | first,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                LONGEST_MESSAGE as u32,
                LONGEST_MESSAGE as u32,
                0,
                &attributes,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the pipe is open and owned here alone.
        let connection = Connection(unsafe { OwnedHandle::from_raw_handle(pipe) });
        // SAFETY: the pipe is open; a client that connected before this call counts.
        let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0;
        // SAFETY: reads this thread's last error, set by the call above.
        if !connected && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            return Err(io::Error::last_os_error());
        }
        Ok(connection)
    }
}

impl Connection {
    /// Opens the service's pipe as this process's user.
    pub fn open() -> io::Result<Self> {
        let name = wide(&name());
        let deadline = Instant::now() + CONNECT_WAIT;
        loop {
            // SAFETY: the name ends in a nul and every other argument is a plain value.
            let pipe = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    CLIENT_ACCESS,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if pipe != INVALID_HANDLE_VALUE {
                // SAFETY: the pipe is open and owned here alone.
                return Ok(Self(unsafe { OwnedHandle::from_raw_handle(pipe) }));
            }
            let error = io::Error::last_os_error();
            let busy = error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32);
            let starting = error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32);
            if !(busy || starting) || Instant::now() >= deadline {
                return Err(error);
            }
            if busy {
                // SAFETY: the name ends in a nul.
                unsafe { WaitNamedPipeW(name.as_ptr(), 1000) };
            } else {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    /// The client's token, as the client's thread holds it.
    pub fn client_token(&self) -> io::Result<Token> {
        // SAFETY: the pipe is open and has read from its client, as impersonation requires.
        if unsafe { ImpersonateNamedPipeClient(self.handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: this thread is impersonating; the call writes one handle.
        let opened =
            unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_ALL_ACCESS, 0, &mut token) };
        let error = io::Error::last_os_error();
        // SAFETY: ends the impersonation begun above, whatever the open did.
        unsafe { RevertToSelf() };
        if opened == 0 {
            return Err(error);
        }
        Token::owning(token).primary()
    }

    /// One line of JSON each way.
    pub fn send(&mut self, line: &str) -> io::Result<()> {
        self.write_all(line.as_bytes())?;
        self.write_all(b"\n")?;
        // SAFETY: the pipe is open; this waits until the client has read what was written.
        unsafe { FlushFileBuffers(self.handle()) };
        Ok(())
    }

    pub fn receive(&mut self) -> io::Result<String> {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while line.len() < LONGEST_MESSAGE {
            if self.read(&mut byte)? == 0 || byte[0] == b'\n' {
                return String::from_utf8(line).map_err(io::Error::other);
            }
            line.push(byte[0]);
        }
        Err(io::Error::other("the message is too long"))
    }

    fn handle(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}

impl Read for Connection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut read = 0u32;
        // SAFETY: `buffer` is writable for the length given.
        let done = unsafe {
            ReadFile(
                self.handle(),
                buffer.as_mut_ptr(),
                buffer.len().min(u32::MAX as usize) as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        match done {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(read as usize),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut written = 0u32;
        // SAFETY: `buffer` is readable for the length given.
        let done = unsafe {
            WriteFile(
                self.handle(),
                buffer.as_ptr(),
                buffer.len().min(u32::MAX as usize) as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        match done {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(written as usize),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: the pipe is open; on a client's end the call fails harmlessly.
        unsafe { DisconnectNamedPipe(self.handle()) };
    }
}
