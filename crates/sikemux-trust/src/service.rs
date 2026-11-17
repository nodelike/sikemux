use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    SetHandleInformation, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST, HANDLE,
    HANDLE_FLAG_INHERIT, NO_ERROR, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::RemoteDesktop::{WTSQueryUserToken, WTSSESSION_NOTIFICATION};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService,
    CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
    SC_ACTION, SC_ACTION_RESTART, SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE,
    SERVICE_ACCEPT_SESSIONCHANGE, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
    SERVICE_CONFIG_FAILURE_ACTIONS, SERVICE_CONTROL_SESSIONCHANGE, SERVICE_CONTROL_STOP,
    SERVICE_ERROR_NORMAL, SERVICE_FAILURE_ACTIONSW, SERVICE_NO_CHANGE, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE,
    SERVICE_STOP, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW,
    SERVICE_WIN32_OWN_PROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, TerminateProcess, WaitForSingleObject, INFINITE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::WTS_SESSION_LOGOFF;

use crate::descriptor::wide;
use crate::pipe::{Connection, Listener};
use crate::spawn::{person_environment, spawn, Start};
use crate::token::{Integrity, Token};

/// Release and dev builds each install their own service, so a dev build never answers for an
/// installed Sikemux.
pub const NAME: &str = if cfg!(debug_assertions) {
    "SikemuxDev"
} else {
    "Sikemux"
};
const DISPLAY_NAME: &str = if cfg!(debug_assertions) {
    "Sikemux (dev)"
} else {
    "Sikemux"
};

/// A core that ends sooner than this after starting counts as failing to start.
const SHORT_RUN: Duration = Duration::from_secs(60);
/// After this many short runs in a row the core is left down.
const SHORT_RUNS: u32 = 5;
const RETRY_WAIT: Duration = Duration::from_secs(1);
/// How long `stop` waits for the service to end its cores and stop.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// How the service runs a core: this program with `subcommand`, taking only `options`, each
/// with one value, from the person who asks. Updates are checked against `release_key`.
pub struct Core {
    pub subcommand: &'static str,
    pub options: &'static [&'static str],
    pub release_key: fn() -> Result<minisign_verify::PublicKey, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Ask {
    Start {
        socket: PathBuf,
        log: PathBuf,
        args: Vec<String>,
    },
    Apply {
        setup: PathBuf,
        signature: String,
        version: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Answer {
    Started { pid: u32 },
    Running { pid: u32 },
    Applying,
    Refused { reason: String },
}

/// Asks the service to start this person's core, or confirms it already runs. The pid either way.
pub fn start_core(socket: &Path, log: &Path, args: &[OsString]) -> io::Result<u32> {
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_owned)
                .ok_or_else(|| io::Error::other(format!("{arg:?} is not Unicode")))
        })
        .collect::<io::Result<_>>()?;
    let ask = Ask::Start {
        socket: socket.to_owned(),
        log: log.to_owned(),
        args,
    };
    match asked(&ask)? {
        Answer::Started { pid } | Answer::Running { pid } => Ok(pid),
        Answer::Applying => Err(io::Error::other("the service answered with an update")),
        Answer::Refused { reason } => Err(io::Error::other(reason)),
    }
}

/// Asks the service to install the release `version` from the installer at `setup`, signed
/// with `signature` as the feed gave it. The installer ends every Sikemux program, and the
/// service opens the app again once it is done. Only a High core may ask.
pub fn apply_update(setup: &Path, signature: &str, version: &str) -> io::Result<()> {
    let ask = Ask::Apply {
        setup: setup.to_owned(),
        signature: signature.to_owned(),
        version: version.to_owned(),
    };
    match asked(&ask)? {
        Answer::Applying => Ok(()),
        Answer::Refused { reason } => Err(io::Error::other(reason)),
        Answer::Started { .. } | Answer::Running { .. } => {
            Err(io::Error::other("the service answered with a core"))
        }
    }
}

fn asked(ask: &Ask) -> io::Result<Answer> {
    let mut connection = Connection::open()?;
    connection.send(&serde_json::to_string(ask)?)?;
    Ok(serde_json::from_str(&connection.receive()?)?)
}

/// Whether this machine has the service installed.
pub fn installed() -> bool {
    Manager::open(SC_MANAGER_CONNECT)
        .and_then(|manager| manager.service(SERVICE_QUERY_STATUS))
        .is_ok()
}

/// Installs the service to run `command_line` as SYSTEM at every boot, restarted if it fails,
/// and starts it. An installed service is pointed at `command_line` instead. Needs an
/// administrator.
pub fn install(command_line: &str) -> io::Result<()> {
    let manager = Manager::open(SC_MANAGER_CREATE_SERVICE | SC_MANAGER_CONNECT)?;
    let binary = wide(command_line);
    let service = match manager.service(SERVICE_ALL_ACCESS) {
        Ok(service) => {
            // SAFETY: the service is open and `binary` ends in a nul; every null leaves that
            // setting as it is.
            let changed = unsafe {
                ChangeServiceConfigW(
                    service.0,
                    SERVICE_NO_CHANGE,
                    SERVICE_NO_CHANGE,
                    SERVICE_NO_CHANGE,
                    binary.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if changed == 0 {
                return Err(io::Error::last_os_error());
            }
            service
        }
        Err(error) if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) => {
            create(&manager, &binary)?
        }
        Err(error) => return Err(error),
    };
    let mut restart = [SC_ACTION {
        Type: SC_ACTION_RESTART,
        Delay: 1000,
    }; 3];
    let actions = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 24 * 60 * 60,
        lpRebootMsg: std::ptr::null_mut(),
        lpCommand: std::ptr::null_mut(),
        cActions: restart.len() as u32,
        lpsaActions: restart.as_mut_ptr(),
    };
    // SAFETY: `actions` and the array it points at live through the call.
    let set = unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            std::ptr::from_ref(&actions).cast(),
        )
    };
    if set == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the service is open; it takes no arguments.
    if unsafe { StartServiceW(service.0, 0, std::ptr::null()) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_SERVICE_ALREADY_RUNNING as i32) {
            return Err(error);
        }
    }
    Ok(())
}

fn create(manager: &Manager, binary: &[u16]) -> io::Result<Handle> {
    let name = wide(NAME);
    let display = wide(DISPLAY_NAME);
    // SAFETY: every string ends in a nul; the null arguments mean no group, dependencies or
    // account, which runs the service as LocalSystem.
    let handle = unsafe {
        CreateServiceW(
            manager.0 .0,
            name.as_ptr(),
            display.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            binary.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    Handle::checked(handle)
}

/// Stops the service, which ends every core it started, and waits until it has. A machine
/// without the service has nothing to stop. Needs an administrator.
pub fn stop() -> io::Result<()> {
    let service =
        match Manager::open(SC_MANAGER_CONNECT)?.service(SERVICE_STOP | SERVICE_QUERY_STATUS) {
            Ok(service) => service,
            Err(error) if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
    let mut status = SERVICE_STATUS::default();
    // SAFETY: the service is open, and the call writes one status. It fails when the service
    // is already stopped, which is what is wanted.
    unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) };
    let until = Instant::now() + STOP_WAIT;
    loop {
        // SAFETY: the service is open, and the call writes one status.
        if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if status.dwCurrentState == SERVICE_STOPPED {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(io::Error::other("the service did not stop in time"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stops the service, which ends every core it started, and removes it. Needs an administrator.
pub fn remove() -> io::Result<()> {
    stop()?;
    let service = Manager::open(SC_MANAGER_CONNECT)?.service(SERVICE_ALL_ACCESS)?;
    // SAFETY: the service is open; Windows removes it once its last handle closes.
    if unsafe { DeleteService(service.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Runs as the service until Windows stops it. Called from the program the service installs.
pub fn run(core: Core) -> io::Result<()> {
    SERVICE
        .set(Service {
            core,
            program: std::env::current_exe()?,
            cores: Mutex::new(HashMap::new()),
            status: OnceLock::new(),
        })
        .map_err(|_| io::Error::other("the service runs once per process"))?;
    let mut name = wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    // SAFETY: the table ends in an empty entry and lives until the call returns, after the
    // service stops.
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

static SERVICE: OnceLock<Service> = OnceLock::new();
static STOP: OnceLock<Sender<()>> = OnceLock::new();

struct Service {
    core: Core,
    program: PathBuf,
    /// One core per person, by SID.
    cores: Mutex<HashMap<String, Arc<Running>>>,
    status: OnceLock<StatusHandle>,
}

struct StatusHandle(SERVICE_STATUS_HANDLE);
// SAFETY: a status handle is a plain value Windows accepts from any thread.
unsafe impl Send for StatusHandle {}
// SAFETY: as above.
unsafe impl Sync for StatusHandle {}

struct Running {
    session: u32,
    process: Mutex<OwnedHandle>,
    pid: Mutex<u32>,
    ending: AtomicBool,
}

struct Launch {
    token: Token,
    user: String,
    command_line: String,
    log: PathBuf,
}

unsafe extern "system" fn service_main(_count: u32, _arguments: *mut windows_sys::core::PWSTR) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    let name = wide(NAME);
    // SAFETY: the name ends in a nul and the handler is a plain function.
    let status =
        unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), std::ptr::null()) };
    if status.is_null() {
        return;
    }
    let _ = service.status.set(StatusHandle(status));
    let (stop, stopped) = mpsc::channel();
    let _ = STOP.set(stop);
    report(SERVICE_RUNNING);
    std::thread::spawn(|| crate::update::reopen_app(&service.program));
    std::thread::spawn(|| listen(service));
    let _ = stopped.recv();
    report(SERVICE_STOP_PENDING);
    service.end_cores(|_| true);
    report(SERVICE_STOPPED);
}

unsafe extern "system" fn control(
    control: u32,
    event: u32,
    data: *mut std::ffi::c_void,
    _context: *mut std::ffi::c_void,
) -> u32 {
    let Some(service) = SERVICE.get() else {
        return NO_ERROR;
    };
    match control {
        SERVICE_CONTROL_STOP => {
            if let Some(stop) = STOP.get() {
                let _ = stop.send(());
            }
        }
        SERVICE_CONTROL_SESSIONCHANGE if event == WTS_SESSION_LOGOFF => {
            // SAFETY: for a session change, Windows passes a WTSSESSION_NOTIFICATION.
            let session = unsafe { (*data.cast::<WTSSESSION_NOTIFICATION>()).dwSessionId };
            service.end_cores(|running| running.session == session);
        }
        _ => {}
    }
    NO_ERROR
}

fn report(state: SERVICE_STATUS_CURRENT_STATE) {
    let Some(status) = SERVICE.get().and_then(|service| service.status.get()) else {
        return;
    };
    let accepted = match state {
        SERVICE_RUNNING => SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SESSIONCHANGE,
        _ => 0,
    };
    let current = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: NO_ERROR,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 10_000,
    };
    // SAFETY: the handle came from RegisterServiceCtrlHandlerExW.
    unsafe { SetServiceStatus(status.0, &current) };
}

fn listen(service: &'static Service) {
    let mut listener = match Listener::new() {
        Ok(listener) => listener,
        Err(error) => return eprintln!("the service cannot make its pipe: {error}"),
    };
    loop {
        match listener.accept() {
            Ok(connection) => {
                std::thread::spawn(move || service.answer(connection));
            }
            Err(error) => {
                eprintln!("the service's pipe failed: {error}");
                return;
            }
        }
    }
}

impl Service {
    fn answer(&'static self, mut connection: Connection) {
        let answer = match connection.receive() {
            Ok(line) => self.handle(&connection, &line),
            Err(_) => return,
        };
        let answer = answer.unwrap_or_else(|reason| Answer::Refused { reason });
        if let Ok(line) = serde_json::to_string(&answer) {
            let _ = connection.send(&line);
        }
    }

    fn handle(&'static self, connection: &Connection, line: &str) -> Result<Answer, String> {
        let ask: Ask = serde_json::from_str(line).map_err(|error| error.to_string())?;
        let person = connection
            .client_token()
            .map_err(|error| error.to_string())?;
        match ask {
            Ask::Start { socket, log, args } => self.start_core(person, socket, log, &args),
            Ask::Apply {
                setup,
                signature,
                version,
            } => {
                if person.integrity().map_err(|error| error.to_string())? != Integrity::High {
                    return Err("only a core asks for an update".into());
                }
                let key = (self.core.release_key)()?;
                crate::update::apply(&person, &key, &setup, &signature, &version)?;
                Ok(Answer::Applying)
            }
        }
    }

    fn start_core(
        &'static self,
        person: Token,
        socket: PathBuf,
        log: PathBuf,
        args: &[String],
    ) -> Result<Answer, String> {
        let user = person.user().map_err(|error| error.to_string())?;
        let mut cores = held(&self.cores);
        if let Some(running) = cores.get(&user) {
            return Ok(Answer::Running {
                pid: *held(&running.pid),
            });
        }
        let launch = Launch {
            token: person
                .for_person(Integrity::High)
                .map_err(|error| error.to_string())?,
            command_line: self.command_line(&socket, &log, args)?,
            user: user.clone(),
            log,
        };
        let (process, pid) = self.start(&launch).map_err(|error| error.to_string())?;
        let running = Arc::new(Running {
            session: launch.token.session().map_err(|error| error.to_string())?,
            process: Mutex::new(process),
            pid: Mutex::new(pid),
            ending: AtomicBool::new(false),
        });
        cores.insert(user, Arc::clone(&running));
        drop(cores);
        std::thread::spawn(move || self.keep_up(&launch, &running));
        Ok(Answer::Started { pid })
    }

    /// `"<program>" <subcommand> --socket "<socket>" <option> "<value>"…`, refusing anything
    /// the core was not built to take from the person.
    fn command_line(&self, socket: &Path, log: &Path, args: &[String]) -> Result<String, String> {
        let mut line = format!(
            "{} {} --socket {}",
            quote(self.program.as_os_str())?,
            self.core.subcommand,
            quote(socket.as_os_str())?
        );
        if !socket.is_absolute() || !log.is_absolute() {
            return Err("the socket and log need whole paths".into());
        }
        let (pairs, odd) = args.as_chunks::<2>();
        if !odd.is_empty() {
            return Err("every option takes one value".into());
        }
        for [option, value] in pairs {
            if !self.core.options.contains(&option.as_str()) {
                return Err(format!("the core does not take {option} from a person"));
            }
            line.push_str(&format!(" {option} {}", quote(OsStr::new(value))?));
        }
        Ok(line)
    }

    fn start(&self, launch: &Launch) -> io::Result<(OwnedHandle, u32)> {
        let output = launch.token.as_user(|| {
            OpenOptions::new()
                .append(true)
                .create(true)
                .open(&launch.log)
        })??;
        let output_handle = output.as_raw_handle();
        // SAFETY: the file is open; only the core this spawn starts inherits it, by the list.
        if unsafe { SetHandleInformation(output_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        let environment = self.core_environment(&launch.token)?;
        let system = system_folder()?;
        let process_access = format!("O:SYG:SYD:P(A;;GA;;;SY)(A;;0x101001;;;{})", launch.user);
        let spawned = spawn(&Start {
            token: &launch.token,
            command_line: &launch.command_line,
            inherit: &[output_handle as HANDLE],
            output: Some(output_handle as HANDLE),
            environment: Some(&environment),
            directory: Some(&system.to_string_lossy()),
            process_access: Some(&process_access),
            on_desktop: true,
        })?;
        Ok((spawned.process, spawned.pid))
    }

    /// The core's own environment, which nothing the person's programs can write decides.
    /// Windows' own names come from this service; the person's folders from their profile,
    /// which say where files go but load no code. PATH holds only Windows and Sikemux.
    fn core_environment(&self, token: &Token) -> io::Result<Vec<(OsString, OsString)>> {
        let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(name, _)| is_one_of(name, MACHINE_NAMES))
            .collect();
        environment.extend(
            person_environment(token)?
                .into_iter()
                .filter(|(name, _)| is_one_of(name, PERSON_NAMES)),
        );
        let system = system_folder()?;
        let windows = system.parent().unwrap_or(&system).to_owned();
        let installed = self.program.parent().unwrap_or(&self.program).to_owned();
        let path = std::env::join_paths([system, windows, installed]).map_err(io::Error::other)?;
        environment.push(("PATH".into(), path));
        Ok(environment)
    }

    fn keep_up(&self, launch: &Launch, running: &Running) {
        let mut short_runs = 0;
        loop {
            let started = Instant::now();
            let exit = running.wait();
            if running.ending.load(Ordering::SeqCst) || exit == 0 {
                break;
            }
            short_runs = match started.elapsed() < SHORT_RUN {
                true => short_runs + 1,
                false => 1,
            };
            if short_runs >= SHORT_RUNS {
                eprintln!(
                    "{}'s core ended with {exit:#x}, {short_runs} times in a row; it is left down",
                    launch.user
                );
                break;
            }
            std::thread::sleep(RETRY_WAIT * short_runs);
            if running.ending.load(Ordering::SeqCst) || !signed_in(running.session) {
                break;
            }
            match self.start(launch) {
                Ok((process, pid)) => {
                    *held(&running.process) = process;
                    *held(&running.pid) = pid;
                }
                Err(error) => {
                    eprintln!("{}'s core could not start again: {error}", launch.user);
                    break;
                }
            }
        }
        let mut cores = held(&self.cores);
        if cores
            .get(&launch.user)
            .is_some_and(|kept| std::ptr::eq(kept.as_ref(), running))
        {
            cores.remove(&launch.user);
        }
    }

    fn end_cores(&self, which: impl Fn(&Running) -> bool) {
        let cores = held(&self.cores);
        for running in cores.values().filter(|running| which(running)) {
            running.ending.store(true, Ordering::SeqCst);
            let process = held(&running.process);
            // SAFETY: the process handle is open; ending an exited process fails harmlessly.
            unsafe { TerminateProcess(process.as_raw_handle(), 1) };
        }
    }
}

impl Running {
    fn wait(&self) -> u32 {
        let process = held(&self.process).try_clone();
        let Ok(process) = process else {
            return 1;
        };
        // SAFETY: the handle is open for as long as `process` lives.
        let waited = unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) };
        let mut exit = 1u32;
        if waited == WAIT_OBJECT_0 {
            // SAFETY: the process has ended; the call writes one code.
            unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut exit) };
        }
        exit
    }
}

/// Whether someone is still signed in to `session`, so a core that ended at sign-out is not
/// started again.
fn signed_in(session: u32) -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the call writes one handle, closed below, or fails.
    if unsafe { WTSQueryUserToken(session, &mut token) } == 0 {
        return false;
    }
    drop(Token::owning(token));
    true
}

const MACHINE_NAMES: &[&str] = &[
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATHEXT",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
    "COMPUTERNAME",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "PUBLIC",
    "ALLUSERSPROFILE",
];
const PERSON_NAMES: &[&str] = &[
    "USERNAME",
    "USERDOMAIN",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "TEMP",
    "TMP",
    "DO_NOT_TRACK",
];

fn is_one_of(name: &OsStr, names: &[&str]) -> bool {
    name.to_str()
        .is_some_and(|name| names.iter().any(|kept| kept.eq_ignore_ascii_case(name)))
}

pub(crate) fn system_folder() -> io::Result<PathBuf> {
    let root = std::env::var_os("SystemRoot")
        .ok_or_else(|| io::Error::other("this service has no SystemRoot"))?;
    Ok(PathBuf::from(root).join("System32"))
}

/// Quotes one argument for a Windows command line. Quotes inside an argument are refused, so
/// only backslashes before the closing quote need doubling.
pub(crate) fn quote(argument: &OsStr) -> Result<String, String> {
    let text = argument
        .to_str()
        .ok_or_else(|| format!("{argument:?} is not Unicode"))?;
    if text.contains(['"', '\0', '\n', '\r']) {
        return Err(format!(
            "{text:?} holds a character the core is never passed"
        ));
    }
    let trailing = text.len() - text.trim_end_matches('\\').len();
    Ok(format!("\"{text}{}\"", "\\".repeat(trailing)))
}

struct Handle(SC_HANDLE);

impl Handle {
    fn checked(handle: SC_HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is open and closed once.
        unsafe { CloseServiceHandle(self.0) };
    }
}

struct Manager(Handle);

impl Manager {
    fn open(access: u32) -> io::Result<Self> {
        // SAFETY: null names mean this machine's active service database.
        let handle = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
        Handle::checked(handle).map(Self)
    }

    fn service(&self, access: u32) -> io::Result<Handle> {
        let name = wide(NAME);
        // SAFETY: the manager is open and the name ends in a nul.
        Handle::checked(unsafe { OpenServiceW(self.0 .0, name.as_ptr(), access) })
    }
}

/// Each guarded value is whole between statements, so one left by a panicking thread is
/// still sound to use.
fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
