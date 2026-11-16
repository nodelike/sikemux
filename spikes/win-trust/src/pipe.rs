use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, CreatePipe, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
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
    unsafe {
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
    let wn = wide(name);
    unsafe {
        CreateFileW(
            wn.as_ptr(),
            0xC000_0000,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    }
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
