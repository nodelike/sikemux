use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    AllocateAndInitializeSid, CreateRestrictedToken, CreateWellKnownSid, DuplicateTokenEx,
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, SecurityImpersonation,
    SetTokenInformation, TokenIntegrityLevel, TokenPrimary, TokenUser, WinBuiltinAdministratorsSid,
    DISABLE_MAX_PRIVILEGE, SECURITY_MANDATORY_LABEL_AUTHORITY, SID_AND_ATTRIBUTES,
    TOKEN_ACCESS_MASK, TOKEN_ALL_ACCESS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::SystemServices::SE_GROUP_INTEGRITY;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::util::{errno, from_wide_ptr};

pub const MEDIUM_RID: u32 = 0x2000;
pub const HIGH_RID: u32 = 0x3000;

pub fn self_token(access: TOKEN_ACCESS_MASK) -> HANDLE {
    let mut token: HANDLE = null_mut();
    unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) };
    token
}

pub fn to_primary(token: HANDLE) -> HANDLE {
    let mut out: HANDLE = null_mut();
    unsafe {
        DuplicateTokenEx(
            token,
            TOKEN_ALL_ACCESS,
            null(),
            SecurityImpersonation,
            TokenPrimary,
            &mut out,
        )
    };
    out
}

/// Deny-only the local Administrators group and drop every privilege, so the
/// result runs as the same person with no administrative power.
pub fn deny_admins(token: HANDLE) -> HANDLE {
    let mut admins = [0u8; 68];
    let mut size = admins.len() as u32;
    let ok = unsafe {
        CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            null_mut(),
            admins.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if ok == 0 {
        return null_mut();
    }
    let deny = [SID_AND_ATTRIBUTES {
        Sid: admins.as_mut_ptr() as *mut c_void,
        Attributes: 0,
    }];
    let mut out: HANDLE = null_mut();
    unsafe {
        CreateRestrictedToken(
            token,
            DISABLE_MAX_PRIVILEGE,
            deny.len() as u32,
            deny.as_ptr(),
            0,
            null(),
            0,
            null(),
            &mut out,
        )
    };
    out
}

pub fn set_integrity(token: HANDLE, rid: u32) -> bool {
    let mut psid: *mut c_void = null_mut();
    let ok = unsafe {
        AllocateAndInitializeSid(
            &SECURITY_MANDATORY_LABEL_AUTHORITY,
            1,
            rid,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut psid,
        )
    };
    if ok == 0 {
        return false;
    }
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: psid,
            Attributes: SE_GROUP_INTEGRITY as u32,
        },
    };
    unsafe {
        SetTokenInformation(
            token,
            TokenIntegrityLevel,
            &label as *const _ as *const c_void,
            std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32,
        ) != 0
    }
}

pub fn user_sid_string(token: HANDLE) -> Option<String> {
    let mut needed = 0u32;
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }
    let user = buf.as_ptr() as *const TOKEN_USER;
    let sid = unsafe { (*user).User.Sid };
    let mut out: *mut u16 = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut out) } == 0 {
        return None;
    }
    let s = from_wide_ptr(out);
    unsafe { LocalFree(out as *mut c_void) };
    Some(s)
}

pub fn integrity_of(token: HANDLE) -> Option<u32> {
    let mut needed = 0u32;
    unsafe { GetTokenInformation(token, TokenIntegrityLevel, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    if unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        )
    } == 0
    {
        return None;
    }
    let label = buf.as_ptr() as *const TOKEN_MANDATORY_LABEL;
    let sid = unsafe { (*label).Label.Sid };
    let count = unsafe { *GetSidSubAuthorityCount(sid) };
    if count == 0 {
        return None;
    }
    Some(unsafe { *GetSidSubAuthority(sid, (count - 1) as u32) })
}

pub fn current_integrity() -> Option<u32> {
    let token = self_token(TOKEN_QUERY);
    if token.is_null() {
        return None;
    }
    let out = integrity_of(token);
    unsafe { windows_sys::Win32::Foundation::CloseHandle(token) };
    out
}

/// A Medium-integrity copy of this process's own token, with every privilege
/// dropped. Lowering integrity from your own token needs no privilege, so a
/// process can always hand its children a weaker token than its own.
pub fn medium_restricted(base: HANDLE) -> HANDLE {
    let mut out: HANDLE = null_mut();
    unsafe {
        CreateRestrictedToken(
            base,
            DISABLE_MAX_PRIVILEGE,
            0,
            null(),
            0,
            null(),
            0,
            null(),
            &mut out,
        )
    };
    if !out.is_null() {
        set_integrity(out, MEDIUM_RID);
    }
    out
}

pub fn fail(context: &str) -> String {
    format!("{context} failed ({})", errno())
}
