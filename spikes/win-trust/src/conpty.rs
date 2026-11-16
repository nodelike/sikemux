use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{ClosePseudoConsole, CreatePseudoConsole, COORD, HPCON};
use windows_sys::Win32::System::Environment::CreateEnvironmentBlock;
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
    STARTUPINFOEXW,
};

use crate::pipe::anon;
use crate::util::wide;

/// Open a ConPTY from this (High) process and run cmd.exe inside it under the
/// given Medium token. Returns the console output, so the caller can confirm the
/// shell really ran at Medium integrity.
pub fn run(medium_token: HANDLE) -> Result<String, String> {
    let (in_read, in_write) = anon().ok_or("stdin pipe")?;
    let (out_read, out_write) = anon().ok_or("stdout pipe")?;

    let mut hpc: HPCON = 0;
    let hr = unsafe { CreatePseudoConsole(COORD { X: 100, Y: 40 }, in_read, out_write, 0, &mut hpc) };
    if hr != 0 {
        return Err(format!("CreatePseudoConsole hr={hr:#x}"));
    }
    unsafe {
        CloseHandle(in_read);
        CloseHandle(out_write);
    }

    let mut size: usize = 0;
    unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size) };
    let mut attr_buf = vec![0u8; size];
    let attr_list = attr_buf.as_mut_ptr() as *mut c_void;
    unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut size) };
    unsafe {
        UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            hpc as *const c_void,
            std::mem::size_of::<HPCON>(),
            null_mut(),
            null(),
        )
    };

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attr_list;

    let mut env: *mut c_void = null_mut();
    unsafe { CreateEnvironmentBlock(&mut env, medium_token, 0) };

    let mut cmd = wide("cmd.exe /d /q /k");
    let mut info = PROCESS_INFORMATION::default();
    let ok = unsafe {
        CreateProcessAsUserW(
            medium_token,
            null(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            env,
            null(),
            &startup.StartupInfo,
            &mut info,
        )
    };
    unsafe { DeleteProcThreadAttributeList(attr_list) };
    if ok == 0 {
        unsafe { ClosePseudoConsole(hpc) };
        return Err(format!("launch cmd ({})", crate::util::errno()));
    }

    for line in ["whoami /groups | findstr Mandatory\r\n", "echo conpty-ok\r\n", "exit\r\n"] {
        let mut written = 0u32;
        unsafe { WriteFile(in_write, line.as_ptr(), line.len() as u32, &mut written, null_mut()) };
    }

    let mut output = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut got = 0u32;
        let ok = unsafe { ReadFile(out_read, buf.as_mut_ptr(), buf.len() as u32, &mut got, null_mut()) };
        if ok == 0 || got == 0 {
            break;
        }
        output.extend_from_slice(&buf[..got as usize]);
        if output.len() > 1 << 20 {
            break;
        }
    }
    unsafe {
        WaitForSingleObject(info.hProcess, 10_000);
        ClosePseudoConsole(hpc);
        CloseHandle(in_write);
        CloseHandle(out_read);
    }

    Ok(String::from_utf8_lossy(&output).into_owned())
}
