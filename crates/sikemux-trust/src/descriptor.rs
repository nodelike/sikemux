use std::io;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetSecurityDescriptorDacl, ACL, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
};

/// A security descriptor written in SDDL, the text form `icacls` and the SDK use.
pub struct Descriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is plain memory this value owns alone; nothing else refers to it.
unsafe impl Send for Descriptor {}

impl Descriptor {
    pub fn parse(sddl: &str) -> io::Result<Self> {
        let text = wide(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `text` ends in a nul, and the call writes one pointer, freed when this drops.
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

    /// For a call that takes `SECURITY_ATTRIBUTES`; valid while `self` lives.
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }

    /// The access list inside, valid while `self` lives.
    pub(crate) fn dacl(&self) -> io::Result<*mut ACL> {
        let (mut present, mut defaulted) = (0i32, 0i32);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: `self.0` is a descriptor made by the conversion above; the call writes two
        // flags and a pointer into it.
        let read =
            unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) };
        if read == 0 || present == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(dacl)
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor came from LocalAlloc in the conversion and is freed once.
        unsafe { LocalFree(self.0) };
    }
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
