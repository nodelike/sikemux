use std::ffi::{c_void, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use crate::descriptor::{wide, Descriptor};
use crate::token::Token;

/// A program to start on a token other than this process's own.
pub struct Start<'a> {
    pub token: &'a Token,
    /// The whole command line, program first, quoted as the program expects.
    pub command_line: &'a str,
    /// The only handles the program inherits; each must be inheritable.
    pub inherit: &'a [HANDLE],
    /// Where the program's output and errors go; one of `inherit`.
    pub output: Option<HANDLE>,
    /// The program's whole environment. None gives it the token's user's own, as at sign-in.
    pub environment: Option<&'a [(OsString, OsString)]>,
    pub directory: Option<&'a str>,
    /// Who may open the new process, in SDDL.
    pub process_access: Option<&'a str>,
    /// On the signed-in person's desktop, where its windows show, rather than the service's.
    pub on_desktop: bool,
}

pub struct Spawned {
    pub process: OwnedHandle,
    pub pid: u32,
}

pub fn spawn(start: &Start) -> io::Result<Spawned> {
    let attributes = Attributes::listing(start.inherit)?;
    let mut desktop = wide(r"winsta0\default");
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.list;
    if start.on_desktop {
        startup.StartupInfo.lpDesktop = desktop.as_mut_ptr();
    }
    if let Some(output) = start.output {
        startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdOutput = output;
        startup.StartupInfo.hStdError = output;
    }
    let block = match start.environment {
        Some(pairs) => block_of(pairs),
        None => Environment::of(start.token)?.block(),
    };
    let directory = start.directory.map(wide);
    let descriptor = start.process_access.map(Descriptor::parse).transpose()?;
    let process_attributes = descriptor.as_ref().map(Descriptor::attributes);
    let mut command_line = wide(start.command_line);
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every pointer refers to a value above that lives through the call; the command
    // line is writable, as CreateProcess requires, and `info` takes two handles.
    let started = unsafe {
        CreateProcessAsUserW(
            start.token.handle(),
            std::ptr::null(),
            command_line.as_mut_ptr(),
            process_attributes
                .as_ref()
                .map_or(std::ptr::null(), std::ptr::from_ref),
            std::ptr::null(),
            i32::from(!start.inherit.is_empty()),
            CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            block.as_ptr().cast(),
            directory
                .as_ref()
                .map_or(std::ptr::null(), |path| path.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    };
    if started == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the thread handle is this call's to close, and nothing else holds it.
    unsafe { CloseHandle(info.hThread) };
    Ok(Spawned {
        // SAFETY: the process handle is ours alone from here.
        process: unsafe { OwnedHandle::from_raw_handle(info.hProcess) },
        pid: info.dwProcessId,
    })
}

/// The environment the token's user gets at sign-in, read from their profile rather than from
/// this process, as `NAME`, value pairs.
pub fn person_environment(token: &Token) -> io::Result<Vec<(OsString, OsString)>> {
    let block = Environment::of(token)?.block();
    let pairs = block
        .split(|&unit| unit == 0)
        .take_while(|entry| !entry.is_empty())
        .filter_map(|entry| {
            // A name may itself start with `=`, as the per-drive folders `=C:` do.
            let at = entry
                .iter()
                .skip(1)
                .position(|&unit| unit == u16::from(b'='))?
                + 1;
            Some((
                OsString::from_wide(&entry[..at]),
                OsString::from_wide(&entry[at + 1..]),
            ))
        })
        .collect();
    Ok(pairs)
}

fn block_of(pairs: &[(OsString, OsString)]) -> Vec<u16> {
    let mut block = Vec::new();
    for (name, value) in pairs {
        block.extend(name.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    if pairs.is_empty() {
        block.push(0);
    }
    block
}

struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _memory: Vec<u64>,
}

impl Attributes {
    /// Limits inheritance to `handles`, so the program gets nothing else this process holds.
    fn listing(handles: &[HANDLE]) -> io::Result<Self> {
        if handles.is_empty() {
            return Ok(Self {
                list: std::ptr::null_mut(),
                _memory: Vec::new(),
            });
        }
        let mut size = 0usize;
        // SAFETY: with no list, the call only writes the size it needs.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
        let mut memory = vec![0u64; size.div_ceil(8)];
        let list: LPPROC_THREAD_ATTRIBUTE_LIST = memory.as_mut_ptr().cast();
        // SAFETY: `memory` is writable for `size` bytes.
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let attributes = Self {
            list,
            _memory: memory,
        };
        // SAFETY: `handles` outlives the list, which the spawn reads before `handles` drops.
        let set = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(attributes)
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        if !self.list.is_null() {
            // SAFETY: the list was initialized above and is deleted once.
            unsafe { DeleteProcThreadAttributeList(self.list) };
        }
    }
}

struct Environment(*mut c_void);

impl Environment {
    fn of(token: &Token) -> io::Result<Self> {
        let mut block: *mut c_void = std::ptr::null_mut();
        // SAFETY: `token` is open, and the call writes one block, destroyed when this drops.
        if unsafe { CreateEnvironmentBlock(&mut block, token.handle(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(block))
    }

    /// A copy of the block, entries ending in a nul and the block in another.
    fn block(&self) -> Vec<u16> {
        let start = self.0.cast::<u16>();
        let mut length = 0;
        // SAFETY: the block is a run of nul-terminated entries ended by an empty one, so the
        // walk stops at the first pair of nuls, inside the block.
        while unsafe { *start.add(length) != 0 || *start.add(length + 1) != 0 } {
            length += 1;
        }
        // SAFETY: the `length + 2` units up to and including both nuls lie inside the block.
        unsafe { std::slice::from_raw_parts(start, length + 2) }.to_vec()
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        // SAFETY: the block came from CreateEnvironmentBlock and is destroyed once.
        unsafe { DestroyEnvironmentBlock(self.0) };
    }
}
