//! The file and process calls that differ between Unix and Windows.

use std::io;
use std::path::{Path, PathBuf};

/// Links `destination` to `source`. Windows may refuse a link to someone without the
/// right to make them, so a file is then hard linked; a folder has no such fallback.
pub fn symlink(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination)
    }
    #[cfg(windows)]
    {
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(source, destination)
        } else {
            std::os::windows::fs::symlink_file(source, destination)
                .or_else(|_| std::fs::hard_link(source, destination))
        }
    }
}

/// The real path of `path`, written the way Git writes paths. Windows' own form starts with
/// `\\?\`, which Git rejects and which never equals the path Git prints for the same folder.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let real = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = real.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(real)
}

/// Whether a program can be run from `path`. Windows tells by the name's extension.
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        path.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    ["exe", "cmd", "bat", "com"]
                        .iter()
                        .any(|known| known.eq_ignore_ascii_case(extension))
                })
    }
}

/// Lets `path` be run as a program. Windows decides by the name, so there is nothing to set.
pub fn make_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// The process that started this one.
pub fn parent_id() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(windows)]
    {
        windows_parent_id().unwrap_or(0)
    }
}

#[cfg(windows)]
fn windows_parent_id() -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let own = std::process::id();
    // SAFETY: the snapshot handle is closed before returning, and the entry's size is set
    // before the first call as the API requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            if entry.th32ProcessID == own {
                found = Some(entry.th32ParentProcessID);
                break;
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

/// Moves `from` to `to`, failing instead of replacing something already at `to`.
#[cfg(windows)]
pub fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<u16>>()
    };
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both buffers end in a zero and outlive the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Sets the permission bits of `path`. Windows keeps a user's files private through their
/// folder's access list, so there is nothing to set.
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(windows)]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// `mode` for opening files on every platform. Windows has no permission bits, so there it
/// leaves the options as they are.
pub trait OpenOptionsMode {
    fn mode(&mut self, mode: u32) -> &mut Self;
}

impl OpenOptionsMode for std::fs::OpenOptions {
    fn mode(&mut self, mode: u32) -> &mut Self {
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(self, mode);
        #[cfg(windows)]
        let _ = mode;
        self
    }
}

/// `mode` for making folders on every platform, with the same limit.
pub trait DirBuilderMode {
    fn mode(&mut self, mode: u32) -> &mut Self;
}

impl DirBuilderMode for std::fs::DirBuilder {
    fn mode(&mut self, mode: u32) -> &mut Self {
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(self, mode);
        #[cfg(windows)]
        let _ = mode;
        self
    }
}

/// `read_exact_at` on Windows, as `std::os::unix::fs::FileExt` has it.
#[cfg(windows)]
pub trait ReadExactAt {
    fn read_exact_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<()>;
}

#[cfg(windows)]
impl ReadExactAt for std::fs::File {
    fn read_exact_at(&self, mut buffer: &mut [u8], mut offset: u64) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        while !buffer.is_empty() {
            match self.seek_read(buffer, offset) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(read) => {
                    buffer = &mut buffer[read..];
                    offset += read as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// Whether a process with this id is still running.
pub fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists; it sends nothing.
        unsafe {
            libc::kill(pid, 0) == 0
                || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: the handle is closed before returning, and the exit code is written to a
        // live local.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0;
            let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
            CloseHandle(handle);
            alive
        }
    }
}

/// Asks a process to end: SIGTERM on Unix, and on Windows, which has no such request,
/// ends it outright.
pub fn terminate(pid: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        let pid = libc::pid_t::try_from(pid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "impossible process id"))?;
        // SAFETY: kill only sends a signal; it reads and writes no memory of ours.
        if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };

        // SAFETY: the handle is closed before returning.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ended = TerminateProcess(handle, 1) != 0;
            let error = io::Error::last_os_error();
            CloseHandle(handle);
            if ended {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}
