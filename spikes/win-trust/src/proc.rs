use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, STARTUPINFOEXW,
};

use crate::util::wide;

pub const BLOCK_NON_MICROSOFT_BINARIES_ALWAYS_ON: u64 = 1 << 44;
pub const IMAGE_LOAD_NO_REMOTE_ALWAYS_ON: u64 = 1 << 52;
pub const IMAGE_LOAD_NO_LOW_LABEL_ALWAYS_ON: u64 = 1 << 56;

pub struct Spawn<'a> {
    pub token: HANDLE,
    pub cmdline: String,
    pub inherit: &'a [HANDLE],
    pub mitigation: Option<u64>,
    pub visible_desktop: bool,
    pub process_sddl: Option<&'a str>,
}

pub fn make_inheritable(handle: HANDLE) {
    unsafe {
        SetHandleInformation(
            handle,
            HANDLE_FLAG_INHERIT as u32,
            HANDLE_FLAG_INHERIT as u32,
        )
    };
}

/// Launch a process with the given token, restricting inherited handles to the
/// listed ones and optionally stamping a process-creation mitigation policy.
pub fn spawn(s: &Spawn) -> Option<PROCESS_INFORMATION> {
    let attr_count = 1 + u32::from(s.mitigation.is_some());
    let mut size: usize = 0;
    unsafe { InitializeProcThreadAttributeList(null_mut(), attr_count, 0, &mut size) };
    let mut attr_buf = vec![0u8; size];
    let attr_list = attr_buf.as_mut_ptr() as *mut c_void;
    if unsafe { InitializeProcThreadAttributeList(attr_list, attr_count, 0, &mut size) } == 0 {
        return None;
    }

    if unsafe {
        UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            s.inherit.as_ptr() as *const c_void,
            std::mem::size_of_val(s.inherit),
            null_mut(),
            null(),
        )
    } == 0
    {
        unsafe { DeleteProcThreadAttributeList(attr_list) };
        return None;
    }

    let mitigation = s.mitigation.unwrap_or(0);
    if s.mitigation.is_some()
        && unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
                &mitigation as *const u64 as *const c_void,
                std::mem::size_of::<u64>(),
                null_mut(),
                null(),
            )
        } == 0
    {
        unsafe { DeleteProcThreadAttributeList(attr_list) };
        return None;
    }

    let mut desktop = wide(r"winsta0\default");
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attr_list;
    if s.visible_desktop {
        startup.StartupInfo.lpDesktop = desktop.as_mut_ptr();
    }

    let mut env: *mut c_void = null_mut();
    unsafe { CreateEnvironmentBlock(&mut env, s.token, 0) };

    let mut proc_sd: *mut c_void = null_mut();
    let proc_attr = s.process_sddl.map(|sddl| {
        let w = wide(sddl);
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w.as_ptr(),
                SDDL_REVISION_1,
                &mut proc_sd,
                null_mut(),
            )
        };
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: proc_sd,
            bInheritHandle: 0,
        }
    });
    let proc_attr_ptr = proc_attr
        .as_ref()
        .map(|a| a as *const SECURITY_ATTRIBUTES)
        .unwrap_or(null());

    let mut cmd = wide(&s.cmdline);
    let mut info = PROCESS_INFORMATION::default();
    let ok = unsafe {
        CreateProcessAsUserW(
            s.token,
            null(),
            cmd.as_mut_ptr(),
            proc_attr_ptr,
            null(),
            1,
            CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            env,
            null(),
            &startup.StartupInfo,
            &mut info,
        )
    };

    if !env.is_null() {
        unsafe { DestroyEnvironmentBlock(env) };
    }
    unsafe { DeleteProcThreadAttributeList(attr_list) };

    if ok == 0 {
        None
    } else {
        Some(info)
    }
}
