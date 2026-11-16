use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, CreatePipe, DisconnectNamedPipe, WaitNamedPipeW,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};

use crate::util::wide;

fn sd_from_sddl(sddl: &str) -> *mut c_void {
    let w = wide(sddl);
    let mut sd: *mut c_void = null_mut();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        )
    };
    sd
}

pub fn create_named(name: &str, sddl: &str) -> HANDLE {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd_from_sddl(sddl),
        bInheritHandle: 0,
    };
    let wn = wide(name);
    let h = unsafe {
        CreateNamedPipeW(
            wn.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            &sa,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        null_mut()
    } else {
        h
    }
}

pub fn wait_client(pipe: HANDLE) -> bool {
    const ERROR_PIPE_CONNECTED: u32 = 535;
    unsafe { ConnectNamedPipe(pipe, null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED }
}

pub fn disconnect(pipe: HANDLE) {
    unsafe {
        FlushFileBuffers(pipe);
        DisconnectNamedPipe(pipe);
    }
}

pub fn client(name: &str) -> HANDLE {
    const READ_WRITE_DATA: u32 = 0x0010_0083;
    const ERROR_PIPE_BUSY: u32 = 231;
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    let wn = wide(name);
    for _ in 0..20 {
        let h = unsafe {
            CreateFileW(
                wn.as_ptr(),
                READ_WRITE_DATA,
                0,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            return h;
        }
        match unsafe { GetLastError() } {
            ERROR_PIPE_BUSY => unsafe {
                WaitNamedPipeW(wn.as_ptr(), 1000);
            },
            ERROR_FILE_NOT_FOUND => std::thread::sleep(std::time::Duration::from_millis(100)),
            _ => return null_mut(),
        }
    }
    null_mut()
}

pub fn send_line(h: HANDLE, line: &str) -> bool {
    let bytes = format!("{line}\n");
    let mut written = 0u32;
    unsafe {
        WriteFile(
            h,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            null_mut(),
        ) != 0
    }
}

pub fn recv_line(h: HANDLE) -> Option<String> {
    let mut buf = [0u8; 4096];
    let mut got = 0u32;
    let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut got, null_mut()) };
    if ok == 0 || got == 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&buf[..got as usize]).into_owned();
    Some(text.trim_end_matches(['\n', '\r']).to_string())
}

/// An inheritable one-way pipe: (read end, write end).
pub fn anon() -> Option<(HANDLE, HANDLE)> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let mut r: HANDLE = null_mut();
    let mut w: HANDLE = null_mut();
    if unsafe { CreatePipe(&mut r, &mut w, &sa, 0) } == 0 {
        return None;
    }
    Some((r, w))
}
