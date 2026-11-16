use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    CreateRestrictedToken, CreateWellKnownSid, DuplicateTokenEx, GetSidSubAuthority,
    GetSidSubAuthorityCount, GetTokenInformation, ImpersonateLoggedOnUser, RevertToSelf,
    SecurityImpersonation, SetTokenInformation, TokenDefaultDacl, TokenIntegrityLevel,
    TokenPrimary, TokenSessionId, TokenUser, WinBuiltinAdministratorsSid, WinHighLabelSid,
    WinMediumLabelSid, DISABLE_MAX_PRIVILEGE, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES,
    TOKEN_ALL_ACCESS, TOKEN_DEFAULT_DACL, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TOKEN_USER, WELL_KNOWN_SID_TYPE,
};
use windows_sys::Win32::System::SystemServices::{
    SECURITY_MANDATORY_HIGH_RID, SECURITY_MANDATORY_MEDIUM_RID, SE_GROUP_INTEGRITY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::descriptor::Descriptor;

/// Windows integrity levels: a process may not write into, or send input to, one above its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Integrity {
    Medium,
    High,
}

impl Integrity {
    fn label(self) -> WELL_KNOWN_SID_TYPE {
        match self {
            Integrity::Medium => WinMediumLabelSid,
            Integrity::High => WinHighLabelSid,
        }
    }
}

/// An access token: whose a process is and what it may do.
pub struct Token(OwnedHandle);

impl Token {
    /// This process's own.
    pub fn own() -> io::Result<Self> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: GetCurrentProcess is a pseudo handle, and the call writes one handle.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self::owning(token))
    }

    /// `handle` is a token handle that nothing else will close.
    pub(crate) fn owning(handle: HANDLE) -> Self {
        // SAFETY: the caller hands over the only owner of an open token handle.
        Self(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    pub fn handle(&self) -> HANDLE {
        self.0.as_raw_handle()
    }

    /// A copy a new process can run on, from an impersonation token or any other.
    pub fn primary(&self) -> io::Result<Self> {
        let mut copy: HANDLE = std::ptr::null_mut();
        // SAFETY: `self` is an open token, and the call writes one handle.
        let made = unsafe {
            DuplicateTokenEx(
                self.handle(),
                TOKEN_ALL_ACCESS,
                std::ptr::null(),
                SecurityImpersonation,
                TokenPrimary,
                &mut copy,
            )
        };
        if made == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self::owning(copy))
    }

    /// The same person with no privileges, and Administrators kept only to deny access, so an
    /// elevated administrator's token carries no administrator power. The new process's own
    /// objects grant SYSTEM and the person alone: an elevated token's default grants
    /// Administrators instead, which a deny-only group cannot use, and the process would die
    /// before it ran a line.
    pub fn for_person(&self, integrity: Integrity) -> io::Result<Self> {
        let mut admins = Sid::well_known(WinBuiltinAdministratorsSid)?;
        let deny = [SID_AND_ATTRIBUTES {
            Sid: admins.as_mut_ptr(),
            Attributes: 0,
        }];
        let lowered = self.restricted(&deny)?;
        lowered.set_integrity(integrity)?;
        let user = lowered.user()?;
        let objects = Descriptor::parse(&format!("D:(A;;GA;;;SY)(A;;GA;;;{user})"))?;
        let default = TOKEN_DEFAULT_DACL {
            DefaultDacl: objects.dacl()?,
        };
        lowered.set(TokenDefaultDacl, &default)?;
        Ok(lowered)
    }

    /// What everything the core starts runs on: the same token with no privileges, at Medium,
    /// so no program it runs can write into it. Windows lets any process hand its children a
    /// restricted copy of its own token.
    pub fn for_child(&self) -> io::Result<Self> {
        let lowered = self.restricted(&[])?;
        lowered.set_integrity(Integrity::Medium)?;
        Ok(lowered)
    }

    fn restricted(&self, deny: &[SID_AND_ATTRIBUTES]) -> io::Result<Self> {
        let mut out: HANDLE = std::ptr::null_mut();
        // SAFETY: `self` is an open token and `deny` lives through the call, which writes one
        // handle.
        let made = unsafe {
            CreateRestrictedToken(
                self.handle(),
                DISABLE_MAX_PRIVILEGE,
                deny.len() as u32,
                deny.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut out,
            )
        };
        if made == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self::owning(out))
    }

    fn set_integrity(&self, integrity: Integrity) -> io::Result<()> {
        let mut label = Sid::well_known(integrity.label())?;
        let mandatory = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: label.as_mut_ptr(),
                Attributes: SE_GROUP_INTEGRITY as u32,
            },
        };
        self.set(TokenIntegrityLevel, &mandatory)
    }

    fn set<T>(&self, class: TOKEN_INFORMATION_CLASS, value: &T) -> io::Result<()> {
        // SAFETY: `value` is the structure `class` names and lives through the call.
        let set = unsafe {
            SetTokenInformation(
                self.handle(),
                class,
                std::ptr::from_ref(value).cast(),
                std::mem::size_of::<T>() as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The person's SID as text, such as `S-1-5-21-…-1001`.
    pub fn user(&self) -> io::Result<String> {
        let read = self.read(TokenUser)?;
        // SAFETY: `read` holds a TOKEN_USER, aligned by `read`.
        let sid = unsafe { (*read.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: `sid` points into `read`, which lives through the call.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call wrote a nul-terminated string, freed below.
        let length = (0..)
            .take_while(|&at| unsafe { *text.add(at) } != 0)
            .count();
        // SAFETY: `text` holds `length` characters before its nul.
        let user = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
        // SAFETY: the string came from LocalAlloc and is freed once.
        unsafe { LocalFree(text.cast()) };
        Ok(user)
    }

    /// The sign-in session the token's processes run in.
    pub fn session(&self) -> io::Result<u32> {
        let read = self.read(TokenSessionId)?;
        Ok(read[0] as u32)
    }

    /// Runs `work` with this thread acting as the token's user, so a path the user chose is
    /// opened with the user's rights rather than this process's.
    pub fn as_user<T>(&self, work: impl FnOnce() -> T) -> io::Result<T> {
        // SAFETY: `self` is an open token.
        if unsafe { ImpersonateLoggedOnUser(self.handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let done = work();
        // SAFETY: ends the impersonation begun above.
        unsafe { RevertToSelf() };
        Ok(done)
    }

    pub fn integrity(&self) -> io::Result<Integrity> {
        let read = self.read(TokenIntegrityLevel)?;
        // SAFETY: `read` holds a TOKEN_MANDATORY_LABEL whose SID points into it.
        let rid = unsafe {
            let sid = (*read.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
            let count = *GetSidSubAuthorityCount(sid);
            *GetSidSubAuthority(sid, u32::from(count.saturating_sub(1)))
        };
        match rid as i32 {
            rid if rid >= SECURITY_MANDATORY_HIGH_RID => Ok(Integrity::High),
            rid if rid >= SECURITY_MANDATORY_MEDIUM_RID => Ok(Integrity::Medium),
            _ => Err(io::Error::other(format!(
                "integrity 0x{rid:x} is below Medium"
            ))),
        }
    }

    fn read(&self, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
        let mut length = 0u32;
        // SAFETY: with no buffer, the call writes only the length it needs.
        unsafe { GetTokenInformation(self.handle(), class, std::ptr::null_mut(), 0, &mut length) };
        let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
        // SAFETY: `buffer` is writable for `length` bytes and 8-byte aligned.
        let read = unsafe {
            GetTokenInformation(
                self.handle(),
                class,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }

    /// The integrity of this process.
    pub fn own_integrity() -> io::Result<Integrity> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: GetCurrentProcess is a pseudo handle, and the call writes one handle.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Self::owning(token).integrity()
    }
}

struct Sid([u8; SECURITY_MAX_SID_SIZE as usize]);

impl Sid {
    fn well_known(kind: WELL_KNOWN_SID_TYPE) -> io::Result<Self> {
        let mut sid = Self([0; SECURITY_MAX_SID_SIZE as usize]);
        let mut size = sid.0.len() as u32;
        // SAFETY: `sid` is writable for `size` bytes, the most any SID takes.
        let made =
            unsafe { CreateWellKnownSid(kind, std::ptr::null_mut(), sid.as_mut_ptr(), &mut size) };
        if made == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sid)
    }

    fn as_mut_ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }
}
