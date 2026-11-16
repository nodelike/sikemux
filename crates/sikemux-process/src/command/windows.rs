use std::collections::BTreeMap;
use std::ffi::{c_void, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read as _};
use std::ops::{Deref, DerefMut};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Output};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::AsyncReadExt as _;
use windows_sys::Win32::Foundation::{
    DuplicateHandle, GetLastError, DUPLICATE_SAME_ACCESS, ERROR_INSUFFICIENT_BUFFER, HANDLE,
    INVALID_HANDLE_VALUE, TRUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_READ_ATTRIBUTES, FILE_WRITE_ATTRIBUTES, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, HPCON, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess,
    GetExitCodeProcess, InitializeProcThreadAttributeList, RegisterWaitForSingleObject,
    TerminateProcess, UnregisterWaitEx, UpdateProcThreadAttribute, WaitForSingleObject,
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, EXTENDED_STARTUPINFO_PRESENT,
    INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    WT_EXECUTEINWAITTHREAD, WT_EXECUTEONLYONCE,
};

/// The exit code of a process stopped by force, as std's `Child::kill` leaves it.
const KILLED: u32 = 1;

pub(super) enum Stdio {
    Inherit,
    Null,
    Piped,
    Handle(OwnedHandle),
}

impl Stdio {
    pub(super) fn null() -> Self {
        Self::Null
    }

    pub(super) fn piped() -> Self {
        Self::Piped
    }

    pub(super) fn inherit() -> Self {
        Self::Inherit
    }
}

macro_rules! stdio_from_handle {
    ($($from:ty),*) => {
        $(impl From<$from> for Stdio {
            fn from(from: $from) -> Self {
                Self::Handle(OwnedHandle::from(from))
            }
        })*
    };
}

stdio_from_handle!(
    std::fs::File,
    std::io::PipeReader,
    std::io::PipeWriter,
    ChildStdin,
    ChildStdout,
    ChildStderr,
    OwnedHandle
);

enum Arg {
    Regular(OsString),
    Raw(OsString),
}

impl Arg {
    fn as_os_str(&self) -> &OsStr {
        match self {
            Arg::Regular(arg) | Arg::Raw(arg) => arg,
        }
    }
}

/// A variable's name, compared without case as Windows compares them.
#[derive(Clone)]
struct Name {
    name: OsString,
    upper: Vec<u16>,
}

impl Name {
    fn new(name: &OsStr) -> Self {
        let mut upper = Vec::new();
        for unit in char::decode_utf16(name.encode_wide()) {
            match unit {
                Ok(char) => {
                    for char in char.to_uppercase() {
                        upper.extend_from_slice(char.encode_utf16(&mut [0; 2]));
                    }
                }
                Err(unpaired) => upper.push(unpaired.unpaired_surrogate()),
            }
        }
        Self {
            name: name.to_owned(),
            upper,
        }
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.upper == other.upper
    }
}

impl Eq for Name {}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.upper.cmp(&other.upper)
    }
}

pub(super) struct Command {
    program: OsString,
    args: Vec<Arg>,
    clear_env: bool,
    env: BTreeMap<Name, Option<OsString>>,
    current_dir: Option<PathBuf>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    flags: u32,
    pseudoconsole: Option<HPCON>,
}

impl Command {
    pub(super) fn new(program: &OsStr) -> Self {
        Self {
            program: program.to_owned(),
            args: Vec::new(),
            clear_env: false,
            env: BTreeMap::new(),
            current_dir: None,
            stdin: None,
            stdout: None,
            stderr: None,
            flags: 0,
            pseudoconsole: None,
        }
    }

    pub(super) fn arg(&mut self, arg: impl AsRef<OsStr>) {
        self.args.push(Arg::Regular(arg.as_ref().to_owned()));
    }

    pub(super) fn args<I, S>(&mut self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg);
        }
    }

    pub(super) fn raw_arg(&mut self, arg: impl AsRef<OsStr>) {
        self.args.push(Arg::Raw(arg.as_ref().to_owned()));
    }

    pub(super) fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
        let key = Name::new(key.as_ref());
        self.env.remove(&key);
        self.env.insert(key, Some(value.as_ref().to_owned()));
    }

    pub(super) fn envs<I, K, V>(&mut self, variables: I)
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (key, value) in variables {
            self.env(key, value);
        }
    }

    pub(super) fn env_remove(&mut self, key: impl AsRef<OsStr>) {
        let key = Name::new(key.as_ref());
        self.env.remove(&key);
        self.env.insert(key, None);
    }

    pub(super) fn env_clear(&mut self) {
        self.clear_env = true;
        self.env.clear();
    }

    pub(super) fn current_dir(&mut self, dir: impl AsRef<Path>) {
        self.current_dir = Some(dir.as_ref().to_owned());
    }

    pub(super) fn stdin(&mut self, stdio: Stdio) {
        self.stdin = Some(stdio);
    }

    pub(super) fn stdout(&mut self, stdio: Stdio) {
        self.stdout = Some(stdio);
    }

    pub(super) fn stderr(&mut self, stdio: Stdio) {
        self.stderr = Some(stdio);
    }

    pub(super) fn creation_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    pub(super) fn pseudoconsole(&mut self, console: HPCON) {
        self.pseudoconsole = Some(console);
    }

    pub(super) fn get_program(&self) -> &OsStr {
        &self.program
    }

    pub(super) fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.args.iter().map(Arg::as_os_str)
    }

    pub(super) fn get_envs(&self) -> impl Iterator<Item = (&OsStr, Option<&OsStr>)> {
        self.env
            .iter()
            .map(|(key, value)| (key.name.as_os_str(), value.as_deref()))
    }

    pub(super) fn get_current_dir(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    /// The child's variables, sorted without case as Windows expects them.
    fn environment(&self) -> BTreeMap<Name, OsString> {
        let mut variables: BTreeMap<Name, OsString> = if self.clear_env {
            BTreeMap::new()
        } else {
            std::env::vars_os()
                .map(|(key, value)| (Name::new(&key), value))
                .collect()
        };
        for (key, value) in &self.env {
            variables.remove(key);
            if let Some(value) = value {
                variables.insert(key.clone(), value.clone());
            }
        }
        variables
    }

    fn start(&self, defaults: [Default; 3]) -> io::Result<Started> {
        let variables = self.environment();
        let changed_path = self
            .env
            .iter()
            .find(|(key, _)| key.upper == wide("PATH"))
            .and_then(|(_, value)| value.clone());
        let program = resolve(&self.program, changed_path.as_deref())?;
        let (application, mut line) = if is_batch_file(&program) {
            (
                system_directory()?.join("cmd.exe"),
                batch_command_line(&program, &self.args)?,
            )
        } else {
            (program, command_line(&self.program, &self.args)?)
        };
        line.push(0);
        let block = environment_block(&variables)?;
        let application = nul_terminated(application.as_os_str())?;
        let current_dir = self
            .current_dir
            .as_deref()
            .map(|dir| nul_terminated(dir.as_os_str()))
            .transpose()?;

        let slots = [&self.stdin, &self.stdout, &self.stderr];
        let ids = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
        let mut ours: [Option<OwnedHandle>; 3] = [None, None, None];
        let mut theirs: [Option<OwnedHandle>; 3] = [None, None, None];
        if self.pseudoconsole.is_none() {
            for index in 0..3 {
                let input = index == 0;
                let (child_end, our_end) = match (slots[index], defaults[index]) {
                    (Some(Stdio::Inherit), _) | (None, Default::Inherit) => {
                        (inherited(ids[index])?, None)
                    }
                    (Some(Stdio::Null), _) | (None, Default::Null) => {
                        (Some(null_device(input)?), None)
                    }
                    (Some(Stdio::Piped), _) | (None, Default::Piped) => {
                        let (ours, theirs) = pipe(!input)?;
                        (Some(inheritable(theirs.as_raw_handle())?), Some(ours))
                    }
                    (Some(Stdio::Handle(handle)), _) => {
                        (Some(inheritable(handle.as_raw_handle())?), None)
                    }
                };
                theirs[index] = child_end;
                ours[index] = our_end;
            }
        }

        let handed: Vec<HANDLE> = theirs
            .iter()
            .flatten()
            .map(|end| end.as_raw_handle())
            .collect();
        let mut attributes = Attributes::new(
            usize::from(!handed.is_empty()) + usize::from(self.pseudoconsole.is_some()),
        )?;
        if !handed.is_empty() {
            // SAFETY: `handed` outlives the attribute list's use in CreateProcess below.
            unsafe {
                attributes.set(
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handed.as_ptr().cast(),
                    size_of_val(handed.as_slice()),
                )?
            };
        }
        if let Some(console) = &self.pseudoconsole {
            // SAFETY: ConPTY takes the console's value itself, which stays open past the call.
            unsafe {
                attributes.set(
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    *console as *const c_void,
                    size_of::<HPCON>(),
                )?
            };
        }

        // SAFETY: STARTUPINFOEXW is plain data, for which zero is valid.
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        let standard = |index: usize| match &theirs[index] {
            Some(end) => end.as_raw_handle(),
            None if self.pseudoconsole.is_some() => INVALID_HANDLE_VALUE,
            None => null_mut(),
        };
        startup.StartupInfo.hStdInput = standard(0);
        startup.StartupInfo.hStdOutput = standard(1);
        startup.StartupInfo.hStdError = standard(2);
        let mut flags = self.flags | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;
        if self.pseudoconsole.is_some() {
            flags &= !(CREATE_NO_WINDOW | DETACHED_PROCESS);
        }
        if let Some(list) = attributes.list() {
            startup.lpAttributeList = list;
            flags |= EXTENDED_STARTUPINFO_PRESENT;
        }

        let token = child_token();
        // SAFETY: plain data, for which zero is valid; CreateProcess fills it in.
        let mut started: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let inherit = i32::from(!handed.is_empty());
        let current_dir = current_dir.as_ref().map_or(null(), |dir| dir.as_ptr());
        let environment = block.as_ptr().cast::<c_void>();
        // SAFETY: every pointer is to a NUL-terminated string or a struct that outlives the
        // call; the command line is writable, as CreateProcess requires.
        let created = unsafe {
            if token.is_null() {
                CreateProcessW(
                    application.as_ptr(),
                    line.as_mut_ptr(),
                    null(),
                    null(),
                    inherit,
                    flags,
                    environment,
                    current_dir,
                    &startup.StartupInfo,
                    &mut started,
                )
            } else {
                CreateProcessAsUserW(
                    token,
                    application.as_ptr(),
                    line.as_mut_ptr(),
                    null(),
                    null(),
                    inherit,
                    flags,
                    environment,
                    current_dir,
                    &startup.StartupInfo,
                    &mut started,
                )
            }
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateProcess handed us both handles, and nothing else owns them.
        let (process, _thread) = unsafe {
            (
                OwnedHandle::from_raw_handle(started.hProcess),
                OwnedHandle::from_raw_handle(started.hThread),
            )
        };
        let [stdin, stdout, stderr] = ours;
        Ok((
            Child {
                process,
                pid: started.dwProcessId,
                status: None,
            },
            stdin.map(ChildStdin::from),
            stdout.map(ChildStdout::from),
            stderr.map(ChildStderr::from),
        ))
    }
}

/// The token every child starts on, or null for this process's own.
fn child_token() -> HANDLE {
    null_mut()
}

#[derive(Clone, Copy)]
enum Default {
    Inherit,
    Null,
    Piped,
}

const SPAWN: [Default; 3] = [Default::Inherit; 3];
const OUTPUT: [Default; 3] = [Default::Null, Default::Piped, Default::Piped];

struct Attributes {
    buffer: Vec<usize>,
    count: usize,
}

impl Attributes {
    fn new(count: usize) -> io::Result<Self> {
        if count == 0 {
            return Ok(Self {
                buffer: Vec::new(),
                count,
            });
        }
        let mut size = 0;
        // SAFETY: a null list asks only for the size it needs.
        unsafe { InitializeProcThreadAttributeList(null_mut(), count as u32, 0, &mut size) };
        // SAFETY: as above.
        if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0_usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: the buffer holds `size` bytes, aligned for the list.
        if unsafe {
            InitializeProcThreadAttributeList(
                buffer.as_mut_ptr().cast(),
                count as u32,
                0,
                &mut size,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { buffer, count })
    }

    fn list(&mut self) -> Option<LPPROC_THREAD_ATTRIBUTE_LIST> {
        (self.count > 0).then(|| self.buffer.as_mut_ptr().cast())
    }

    /// # Safety
    ///
    /// `value` must stay valid until the list is last used.
    unsafe fn set(
        &mut self,
        attribute: usize,
        value: *const c_void,
        size: usize,
    ) -> io::Result<()> {
        let list = self
            .list()
            .ok_or_else(|| io::Error::other("no room for the attribute"))?;
        // SAFETY: the list was initialized with room for this attribute; the caller keeps
        // `value` alive.
        if unsafe { UpdateProcThreadAttribute(list, 0, attribute, value, size, null_mut(), null()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        if let Some(list) = self.list() {
            // SAFETY: the list was initialized in `new`.
            unsafe { DeleteProcThreadAttributeList(list) };
        }
    }
}

fn inheritable(handle: RawHandle) -> io::Result<OwnedHandle> {
    let mut copy = null_mut();
    // SAFETY: `handle` is open for the call; the copy is a new handle we own.
    let copied = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &mut copy,
            0,
            TRUE,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if copied == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: DuplicateHandle made `copy` for us alone.
    Ok(unsafe { OwnedHandle::from_raw_handle(copy) })
}

/// This process's own standard handle, if it has one, for the child to share.
fn inherited(id: STD_HANDLE) -> io::Result<Option<OwnedHandle>> {
    // SAFETY: GetStdHandle takes a plain value and returns a handle we do not own.
    let handle = unsafe { GetStdHandle(id) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Ok(None);
    }
    inheritable(handle).map(Some)
}

/// std's `ChildStdout`, `ChildStderr` and `ChildStdin` wait forever on a read or write whose
/// handle is not overlapped, so this process's end is overlapped and the child's is plain.
fn pipe(ours_reads: bool) -> io::Result<(OwnedHandle, File)> {
    const PIPE_BUFFER: u32 = 64 * 1024;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        r"\\.\pipe\sikemux-child-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let (direction, their_access) = if ours_reads {
        (
            PIPE_ACCESS_INBOUND,
            FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES,
        )
    } else {
        (
            PIPE_ACCESS_OUTBOUND,
            FILE_GENERIC_READ | FILE_WRITE_ATTRIBUTES,
        )
    };
    let wide_name = nul_terminated(OsStr::new(&name))?;
    // SAFETY: the name is NUL-terminated and outlives the call.
    let ours = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            direction | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            null(),
        )
    };
    if ours == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateNamedPipeW made this handle for us alone.
    let ours = unsafe { OwnedHandle::from_raw_handle(ours) };
    let theirs = OpenOptions::new()
        .access_mode(their_access)
        .share_mode(0)
        .open(&name)?;
    Ok((ours, theirs))
}

fn null_device(input: bool) -> io::Result<OwnedHandle> {
    let null = OpenOptions::new().read(input).write(!input).open("NUL")?;
    inheritable(null.as_raw_handle())
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn nul_terminated(text: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<u16> = text.encode_wide().collect();
    if units.contains(&0) {
        return Err(nul_error());
    }
    units.push(0);
    Ok(units)
}

fn nul_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "nul byte found in provided data",
    )
}

fn environment_block(variables: &BTreeMap<Name, OsString>) -> io::Result<Vec<u16>> {
    let mut block = Vec::new();
    for (key, value) in variables {
        let start = block.len();
        block.extend(key.name.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        if block[start..].contains(&0) {
            return Err(nul_error());
        }
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn system_directory() -> io::Result<PathBuf> {
    // SAFETY: the buffer's length is passed with it.
    directory(|buffer, length| unsafe { GetSystemDirectoryW(buffer, length) })
}

fn windows_directory() -> io::Result<PathBuf> {
    // SAFETY: the buffer's length is passed with it.
    directory(|buffer, length| unsafe { GetWindowsDirectoryW(buffer, length) })
}

fn directory(read: impl Fn(*mut u16, u32) -> u32) -> io::Result<PathBuf> {
    let mut buffer = vec![0_u16; 260];
    loop {
        let length = read(buffer.as_mut_ptr(), buffer.len() as u32) as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < buffer.len() {
            buffer.truncate(length);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        buffer.resize(length, 0);
    }
}

/// Finds the program the way std's `Command` does on Windows: a path is taken as it is, with
/// `.exe` tried first; a bare name is searched for in a changed PATH, this program's folder,
/// the system folders, then this process's PATH, with `.exe` added when it has no extension.
fn resolve(program: &OsStr, changed_path: Option<&OsStr>) -> io::Result<PathBuf> {
    if program.is_empty() {
        return Err(not_found());
    }
    let bytes = program.as_encoded_bytes();
    let is_name = !bytes.iter().any(|byte| matches!(byte, b'/' | b'\\' | b':'));
    if !is_name {
        let path = PathBuf::from(program);
        let has_exe = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
        if has_exe {
            return Ok(path);
        }
        let mut with_exe = program.to_owned();
        with_exe.push(".exe");
        let with_exe = PathBuf::from(with_exe);
        return Ok(if with_exe.exists() { with_exe } else { path });
    }
    nul_terminated(program)?;
    let has_extension = bytes.contains(&b'.');
    let candidate = |mut directory: PathBuf| {
        directory.push(program);
        if !has_extension {
            directory.set_extension("exe");
        }
        directory.exists().then_some(directory)
    };
    let own_folder = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    changed_path
        .into_iter()
        .flat_map(std::env::split_paths)
        .chain(own_folder)
        .chain(system_directory().ok())
        .chain(windows_directory().ok())
        .chain(
            std::env::var_os("PATH")
                .iter()
                .flat_map(std::env::split_paths)
                .collect::<Vec<_>>(),
        )
        .find_map(candidate)
        .ok_or_else(not_found)
}

fn not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "program not found")
}

fn is_batch_file(program: &Path) -> bool {
    program.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
    })
}

/// The command line std builds, which a program splits back into its arguments with
/// `CommandLineToArgvW`.
fn command_line(program: &OsStr, args: &[Arg]) -> io::Result<Vec<u16>> {
    let mut line = vec![u16::from(b'"')];
    line.extend(program.encode_wide());
    line.push(u16::from(b'"'));
    for arg in args {
        line.push(u16::from(b' '));
        append_arg(&mut line, arg)?;
    }
    Ok(line)
}

fn append_arg(line: &mut Vec<u16>, arg: &Arg) -> io::Result<()> {
    let (text, escape) = match arg {
        Arg::Regular(text) => (text, true),
        Arg::Raw(text) => (text, false),
    };
    let units: Vec<u16> = text.encode_wide().collect();
    if units.contains(&0) {
        return Err(nul_error());
    }
    let quote = escape
        && (units.is_empty()
            || units
                .iter()
                .any(|&unit| unit == u16::from(b' ') || unit == u16::from(b'\t')));
    if quote {
        line.push(u16::from(b'"'));
    }
    let mut backslashes = 0;
    for unit in units {
        if escape {
            if unit == u16::from(b'\\') {
                backslashes += 1;
            } else {
                if unit == u16::from(b'"') {
                    line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes + 1));
                }
                backslashes = 0;
            }
        }
        line.push(unit);
    }
    if quote {
        line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        line.push(u16::from(b'"'));
    }
    Ok(())
}

/// The line std hands `cmd.exe` to run a batch file, quoted so that cmd neither splits an
/// argument nor expands a `%variable%` in it.
fn batch_command_line(script: &Path, args: &[Arg]) -> io::Result<Vec<u16>> {
    let mut line = wide("cmd.exe /e:ON /v:OFF /d /c \"");
    line.push(u16::from(b'"'));
    let script: Vec<u16> = script.as_os_str().encode_wide().collect();
    if script.contains(&u16::from(b'"')) || script.last() == Some(&u16::from(b'\\')) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows file names may not contain `\"` or end with `\\`",
        ));
    }
    line.extend(script);
    line.push(u16::from(b'"'));
    for arg in args {
        line.push(u16::from(b' '));
        match arg {
            Arg::Regular(text) => {
                let bytes = text.as_encoded_bytes();
                if bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "batch file arguments are invalid",
                    ));
                }
                append_batch_arg(&mut line, text)?;
            }
            Arg::Raw(_) => append_arg(&mut line, arg)?,
        }
    }
    line.push(u16::from(b'"'));
    Ok(line)
}

fn append_batch_arg(line: &mut Vec<u16>, arg: &OsStr) -> io::Result<()> {
    let units: Vec<u16> = arg.encode_wide().collect();
    if units.contains(&0) {
        return Err(nul_error());
    }
    const UNQUOTED: &str = r"#$*+-./:?@\_";
    let mut quote = units.is_empty() || units.last() == Some(&u16::from(b'\\'));
    for char in char::decode_utf16(units.iter().copied()).flatten() {
        let ascii_needs_quotes =
            char.is_ascii() && !(char.is_ascii_alphanumeric() || UNQUOTED.contains(char));
        if ascii_needs_quotes || char.is_control() {
            quote = true;
        }
    }
    if quote {
        line.push(u16::from(b'"'));
    }
    let mut backslashes = 0;
    for unit in units {
        if unit == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if unit == u16::from(b'"') {
                line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
                line.push(u16::from(b'"'));
            } else if unit == u16::from(b'%') || unit == u16::from(b'\r') {
                line.extend(wide("%%cd:~,"));
            }
            backslashes = 0;
        }
        line.push(unit);
    }
    if quote {
        line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        line.push(u16::from(b'"'));
    }
    Ok(())
}

pub(super) struct AsyncCommand {
    command: Command,
    kill_on_drop: bool,
}

impl AsyncCommand {
    pub(super) fn kill_on_drop(&mut self, kill: bool) {
        self.kill_on_drop = kill;
    }
}

impl Deref for AsyncCommand {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.command
    }
}

impl DerefMut for AsyncCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

pub(super) fn command(inner: &mut Command) -> &mut Command {
    inner
}

pub(super) fn command_ref(inner: &Command) -> &Command {
    inner
}

pub(super) struct Child {
    process: OwnedHandle,
    pid: u32,
    status: Option<ExitStatus>,
}

impl AsRawHandle for Child {
    fn as_raw_handle(&self) -> RawHandle {
        self.process.as_raw_handle()
    }
}

impl Child {
    pub(super) fn id(&self) -> u32 {
        self.pid
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        // SAFETY: the process handle is open for as long as `self`.
        if unsafe { WaitForSingleObject(self.process.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.exit_status()
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        // SAFETY: as above.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => self.exit_status().map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub(super) fn kill(&mut self) -> io::Result<()> {
        // SAFETY: as above.
        if unsafe { TerminateProcess(self.process.as_raw_handle(), KILLED) } != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match self.try_wait() {
            Ok(Some(_)) => Ok(()),
            _ => Err(error),
        }
    }

    fn exit_status(&mut self) -> io::Result<ExitStatus> {
        let mut code = 0;
        // SAFETY: as above; `code` is valid for the write.
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let status = ExitStatus::from_raw(code);
        self.status = Some(status);
        Ok(status)
    }
}

type Started = (
    Child,
    Option<ChildStdin>,
    Option<ChildStdout>,
    Option<ChildStderr>,
);

pub(super) fn spawn(command: &mut Command) -> io::Result<Started> {
    command.start(SPAWN)
}

pub(super) fn output(command: &mut Command) -> io::Result<Output> {
    let (child, _stdin, stdout, stderr) = command.start(OUTPUT)?;
    wait_with_output(child, stdout, stderr)
}

pub(super) fn status(command: &mut Command) -> io::Result<ExitStatus> {
    let (mut child, stdin, _stdout, _stderr) = command.start(SPAWN)?;
    drop(stdin);
    child.wait()
}

pub(super) fn wait_with_output(
    mut child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
) -> io::Result<Output> {
    let read_all = |mut pipe: Box<dyn io::Read + Send>| {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).map(|_| bytes)
    };
    let (stdout, stderr) = match (stdout, stderr) {
        (Some(stdout), Some(stderr)) => {
            let errors = std::thread::spawn(move || read_all(Box::new(stderr)));
            let stdout = read_all(Box::new(stdout))?;
            let stderr = errors
                .join()
                .map_err(|_| io::Error::other("reading stderr panicked"))??;
            (stdout, stderr)
        }
        (Some(stdout), None) => (read_all(Box::new(stdout))?, Vec::new()),
        (None, Some(stderr)) => (Vec::new(), read_all(Box::new(stderr))?),
        (None, None) => (Vec::new(), Vec::new()),
    };
    let status = child.wait()?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

pub(super) fn into_async(command: Command) -> AsyncCommand {
    AsyncCommand {
        command,
        kill_on_drop: false,
    }
}

pub(super) struct AsyncChild {
    child: Child,
    kill_on_drop: bool,
}

impl AsyncChild {
    pub(super) fn id(&self) -> Option<u32> {
        self.child.status.is_none().then_some(self.child.pid)
    }

    pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.child.status {
            return Ok(status);
        }
        exited(&self.child.process).await?;
        self.child.exit_status()
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub(super) fn start_kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }
}

impl Drop for AsyncChild {
    fn drop(&mut self) {
        if self.kill_on_drop && matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
    }
}

/// Resolves once `process` has exited, woken by Windows' wait thread rather than by asking.
async fn exited(process: &OwnedHandle) -> io::Result<()> {
    if let Some((_registration, exit)) = register_exit(process)? {
        let _ = exit.await;
    }
    Ok(())
}

/// Asks Windows to signal once `process` exits, or `None` when it already has.
fn register_exit(
    process: &OwnedHandle,
) -> io::Result<Option<(Registration, tokio::sync::oneshot::Receiver<()>)>> {
    // SAFETY: the handle is open for the call.
    if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
        return Ok(None);
    }
    let (exited, exit) = tokio::sync::oneshot::channel::<()>();
    let sender = Box::into_raw(Box::new(Some(exited)));
    let mut wait = null_mut();
    // SAFETY: `sender` stays alive until the registration is removed, which waits for the
    // callback to finish; the process handle outlives the registration.
    let registered = unsafe {
        RegisterWaitForSingleObject(
            &mut wait,
            process.as_raw_handle(),
            Some(woken),
            sender.cast(),
            INFINITE,
            WT_EXECUTEINWAITTHREAD | WT_EXECUTEONLYONCE,
        )
    };
    if registered == 0 {
        // SAFETY: the registration failed, so nothing else holds `sender`.
        drop(unsafe { Box::from_raw(sender) });
        return Err(io::Error::last_os_error());
    }
    Ok(Some((Registration { wait, sender }, exit)))
}

type Waker = Option<tokio::sync::oneshot::Sender<()>>;

unsafe extern "system" fn woken(context: *mut c_void, _timed_out: bool) {
    // SAFETY: `context` is the `Waker` box, alive until the registration is removed.
    let sender = unsafe { &mut *context.cast::<Waker>() };
    if let Some(sender) = sender.take() {
        let _ = sender.send(());
    }
}

struct Registration {
    wait: HANDLE,
    sender: *mut Waker,
}

// SAFETY: a wait registration may be removed from any thread, and the box it frees is only
// touched again by the callback, which removing the registration waits for.
unsafe impl Send for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: INVALID_HANDLE_VALUE makes the call wait for a running callback, so the box
        // is no longer used once it returns.
        unsafe {
            UnregisterWaitEx(self.wait, INVALID_HANDLE_VALUE);
            drop(Box::from_raw(self.sender));
        }
    }
}

type StartedAsync = (
    AsyncChild,
    Option<tokio::process::ChildStdin>,
    Option<tokio::process::ChildStdout>,
    Option<tokio::process::ChildStderr>,
);

fn into_async_child(
    (child, stdin, stdout, stderr): Started,
    kill_on_drop: bool,
) -> io::Result<StartedAsync> {
    Ok((
        AsyncChild {
            child,
            kill_on_drop,
        },
        stdin
            .map(tokio::process::ChildStdin::from_std)
            .transpose()?,
        stdout
            .map(tokio::process::ChildStdout::from_std)
            .transpose()?,
        stderr
            .map(tokio::process::ChildStderr::from_std)
            .transpose()?,
    ))
}

pub(super) fn spawn_async(command: &mut AsyncCommand) -> io::Result<StartedAsync> {
    into_async_child(command.command.start(SPAWN)?, command.kill_on_drop)
}

pub(super) async fn output_async(command: &mut AsyncCommand) -> io::Result<Output> {
    let (child, _stdin, stdout, stderr) =
        into_async_child(command.command.start(OUTPUT)?, command.kill_on_drop)?;
    wait_with_output_async(child, stdout, stderr).await
}

pub(super) async fn status_async(command: &mut AsyncCommand) -> io::Result<ExitStatus> {
    let (mut child, stdin, _stdout, _stderr) = spawn_async(command)?;
    drop(stdin);
    child.wait().await
}

pub(super) async fn wait_with_output_async(
    mut child: AsyncChild,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
) -> io::Result<Output> {
    async fn read_all(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut bytes).await?;
        }
        Ok(bytes)
    }
    let (status, stdout, stderr) =
        tokio::try_join!(child.wait(), read_all(stdout), read_all(stderr))?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}
