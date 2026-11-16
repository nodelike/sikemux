use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    RevertToSelf, SECURITY_ATTRIBUTES, TOKEN_DUPLICATE, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
use windows_sys::Win32::System::Pipes::ImpersonateNamedPipeClient;
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_STATUS,
    SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, OpenThreadToken, WaitForSingleObject,
};

use crate::pipe::{anon, create_named, disconnect, recv_line, send_line, wait_client};
use crate::proc::{
    spawn, Spawn, BLOCK_NON_MICROSOFT_BINARIES_ALWAYS_ON, IMAGE_LOAD_NO_LOW_LABEL_ALWAYS_ON,
    IMAGE_LOAD_NO_REMOTE_ALWAYS_ON,
};
use crate::sd::{
    deny_admins, grant_user_default_dacl, medium_restricted, set_integrity, to_primary,
    user_sid_string, HIGH_RID, MEDIUM_RID,
};
use crate::util::{
    install_exe, report, report_err, secrets_dir, SendHandle, SERVICE_NAME, SVC_PIPE,
};

const SVC_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;0x12008b;;;AU)";
const CORE_MITIGATION: u64 = IMAGE_LOAD_NO_REMOTE_ALWAYS_ON | IMAGE_LOAD_NO_LOW_LABEL_ALWAYS_ON;
const APP_MITIGATION: u64 = BLOCK_NON_MICROSOFT_BINARIES_ALWAYS_ON
    | IMAGE_LOAD_NO_REMOTE_ALWAYS_ON
    | IMAGE_LOAD_NO_LOW_LABEL_ALWAYS_ON;

static STATUS_HANDLE: AtomicIsize = AtomicIsize::new(0);

#[derive(Default)]
struct Registry {
    core_pid: u32,
    core2_pid: u32,
    app_high_pid: u32,
    app_medium_pid: u32,
    hwnd_high: isize,
    hwnd_medium: isize,
    started: bool,
}
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    core_pid: 0,
    core2_pid: 0,
    app_high_pid: 0,
    app_medium_pid: 0,
    hwnd_high: 0,
    hwnd_medium: 0,
    started: false,
});

pub fn dispatch() {
    let mut name = crate::util::wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(svc_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
}

unsafe extern "system" fn handler(control: u32, _e: u32, _d: *mut c_void, _c: *mut c_void) -> u32 {
    if control == SERVICE_CONTROL_STOP {
        set_state(SERVICE_STOPPED);
        std::process::exit(0);
    }
    0
}

fn set_state(state: u32) {
    let mut status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP
        } else {
            0
        },
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 0,
    };
    let h = STATUS_HANDLE.load(Ordering::SeqCst) as SERVICE_STATUS_HANDLE;
    unsafe { SetServiceStatus(h, &mut status) };
}

unsafe extern "system" fn svc_main(_argc: u32, _argv: *mut *mut u16) {
    let name = crate::util::wide(SERVICE_NAME);
    let h = RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(handler), null());
    STATUS_HANDLE.store(h as isize, Ordering::SeqCst);
    set_state(SERVICE_RUNNING);
    report("svc", "service_started", "running");
    report("svc", "idle_users", &count_secret_dirs().to_string());
    serve();
}

fn count_secret_dirs() -> usize {
    std::fs::read_dir(secrets_dir())
        .map(|d| d.flatten().count())
        .unwrap_or(0)
}

fn serve() {
    loop {
        let pipe = create_named(SVC_PIPE, SVC_PIPE_SDDL);
        if !wait_client(pipe) {
            unsafe { CloseHandle(pipe) };
            continue;
        }
        let req = recv_line(pipe).unwrap_or_default();
        handle(pipe, &req);
        disconnect(pipe);
        unsafe { CloseHandle(pipe) };
    }
}

fn handle(pipe: HANDLE, req: &str) {
    let mut parts = req.split_whitespace();
    match parts.next() {
        Some("start") => {
            let ok = start(pipe);
            send_line(pipe, if ok { "ok" } else { "fail" });
        }
        Some("agent") => {
            let ok = start_agent(pipe);
            send_line(pipe, if ok { "ok" } else { "fail" });
        }
        Some("hwnd") => {
            let tag = parts.next().unwrap_or("");
            let value: isize = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let mut reg = REGISTRY.lock().unwrap();
            if tag == "high" {
                reg.hwnd_high = value;
            } else {
                reg.hwnd_medium = value;
            }
            send_line(pipe, "ok");
        }
        Some("status") => {
            let reg = REGISTRY.lock().unwrap();
            let line = format!(
                "core={};core2={};app_high={};app_medium={};hwnd_high={};hwnd_medium={}",
                reg.core_pid,
                reg.core2_pid,
                reg.app_high_pid,
                reg.app_medium_pid,
                reg.hwnd_high,
                reg.hwnd_medium
            );
            send_line(pipe, &line);
        }
        Some("secret") => {
            send_line(pipe, "refused");
        }
        _ => {
            send_line(pipe, "?");
        }
    }
}

fn client_token(pipe: HANDLE, step: &str) -> Option<HANDLE> {
    if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
        report_err("svc", step, "impersonate");
        return None;
    }
    let mut thread_token: HANDLE = null_mut();
    let got = unsafe {
        OpenThreadToken(
            windows_sys::Win32::System::Threading::GetCurrentThread(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            0,
            &mut thread_token,
        )
    };
    unsafe { RevertToSelf() };
    if got == 0 {
        report_err("svc", step, "open_thread_token");
        return None;
    }
    let locked = deny_admins(to_primary(thread_token));
    if locked.is_null() {
        report_err("svc", step, "deny_admins");
        return None;
    }
    let granted = user_sid_string(locked).is_some_and(|sid| grant_user_default_dacl(locked, &sid));
    if !granted {
        report_err("svc", step, "default_dacl");
    }
    Some(locked)
}

fn start_agent(pipe: HANDLE) -> bool {
    let Some(locked) = client_token(pipe, "agent") else {
        return false;
    };
    if !set_integrity(locked, MEDIUM_RID) {
        report_err("svc", "agent", "set_integrity");
    }
    let spawned = spawn(&Spawn {
        token: locked,
        cmdline: format!("\"{}\" agent", install_exe().display()),
        inherit: &[],
        mitigation: None,
        visible_desktop: true,
        process_sddl: None,
    });
    let Some(info) = spawned else {
        report_err("svc", "agent", "create_process");
        return false;
    };
    let process = SendHandle(info.hProcess);
    std::thread::spawn(move || {
        let process = process;
        report("svc", "agent_exit", &exit_code(process.0));
    });
    true
}

fn start(pipe: HANDLE) -> bool {
    let Some(locked) = client_token(pipe, "start") else {
        return false;
    };
    if !set_integrity(locked, HIGH_RID) {
        report_err("svc", "start", "set_integrity");
    }
    let Some(sid) = user_sid_string(locked) else {
        report_err("svc", "start", "user_sid");
        return false;
    };
    report("svc", "start_sid", &sid);

    let first = {
        let mut reg = REGISTRY.lock().unwrap();
        let first = !reg.started;
        reg.started = true;
        first
    };
    launch_core(SendHandle(locked), &sid, 1, first);
    true
}

fn make_secret_dir(sid: &str) -> std::path::PathBuf {
    let dir = secrets_dir().join(sid);
    let wdir = crate::util::wide(&dir.to_string_lossy());
    let mut sd: *mut c_void = null_mut();
    let sddl = crate::util::wide("O:SYG:SYD:P(A;OICI;GA;;;SY)");
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        );
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let _ = std::fs::create_dir_all(secrets_dir());
        CreateDirectoryW(wdir.as_ptr(), &sa);
    }
    dir
}

fn secret_handler(c2s_read: SendHandle, s2c_write: SendHandle, sid: String) {
    let dir = make_secret_dir(&sid);
    let file = dir.join("secret.txt");
    loop {
        let Some(req) = recv_line(c2s_read.0) else {
            return;
        };
        let mut parts = req.splitn(2, ' ');
        match parts.next() {
            Some("put") => {
                let _ = std::fs::write(&file, parts.next().unwrap_or(""));
                send_line(s2c_write.0, "ok");
            }
            Some("get") => {
                let value = std::fs::read_to_string(&file).unwrap_or_default();
                send_line(s2c_write.0, &format!("ok {value}"));
            }
            _ => {
                send_line(s2c_write.0, "?");
            }
        }
    }
}

fn launch_core(token: SendHandle, sid: &str, gen: u32, with_apps: bool) {
    let (c2s_r, c2s_w) = anon().unwrap();
    let (s2c_r, s2c_w) = anon().unwrap();
    let proc_sd = format!("O:SYG:SYD:P(A;;GA;;;SY)(A;;0x101001;;;{sid})");

    let (apphi_r, apphi_w) = if with_apps {
        anon().unwrap()
    } else {
        (null_mut(), null_mut())
    };
    let (appmed_r, appmed_w) = if with_apps {
        anon().unwrap()
    } else {
        (null_mut(), null_mut())
    };

    let mut cmdline = format!(
        "\"{}\" core --gen {gen} --sid {sid} --secret-w {} --secret-r {}",
        install_exe().display(),
        c2s_w as usize,
        s2c_r as usize,
    );
    let mut inherit = vec![c2s_w, s2c_r];
    if with_apps {
        cmdline.push_str(&format!(
            " --app-hi {} --app-med {}",
            apphi_r as usize, appmed_r as usize
        ));
        inherit.push(apphi_r);
        inherit.push(appmed_r);
    }

    let core = spawn(&Spawn {
        token: token.0,
        cmdline,
        inherit: &inherit,
        mitigation: Some(CORE_MITIGATION),
        visible_desktop: true,
        process_sddl: Some(&proc_sd),
    });
    let Some(core) = core else {
        report_err("svc", "core_launch", "create_process");
        return;
    };
    let pid = unsafe { GetProcessId(core.hProcess) };
    {
        let mut reg = REGISTRY.lock().unwrap();
        if gen == 1 {
            reg.core_pid = pid;
        } else {
            reg.core2_pid = pid;
        }
    }
    report("svc", &format!("core_pid_gen{gen}"), &pid.to_string());

    let sid_owned = sid.to_string();
    let c2s_r = SendHandle(c2s_r);
    let s2c_w = SendHandle(s2c_w);
    std::thread::spawn(move || secret_handler(c2s_r, s2c_w, sid_owned));

    unsafe {
        CloseHandle(c2s_w);
        CloseHandle(s2c_r);
    }

    if with_apps {
        launch_app(token, "high", token.0, apphi_w, &proc_sd);
        let med = medium_restricted(token.0);
        launch_app(token, "medium", med, appmed_w, &proc_sd);
        unsafe {
            CloseHandle(apphi_r);
            CloseHandle(appmed_r);
        }
    }

    let watch = SendHandle(core.hProcess);
    let token2 = token;
    let sid2 = sid.to_string();
    std::thread::spawn(move || {
        let watch = watch;
        report("svc", &format!("core_exit_gen{gen}"), &exit_code(watch.0));
        if gen < 2 {
            report("svc", "core_restart", "relaunching");
            launch_core(token2, &sid2, gen + 1, false);
        }
    });
}

fn launch_app(core_token: SendHandle, tag: &str, token: HANDLE, link_w: HANDLE, proc_sd: &str) {
    let cmdline = format!(
        "\"{}\" app {tag} --link {}",
        install_exe().display(),
        link_w as usize
    );
    let info = spawn(&Spawn {
        token,
        cmdline,
        inherit: &[link_w],
        mitigation: Some(APP_MITIGATION),
        visible_desktop: true,
        process_sddl: Some(proc_sd),
    });
    if let Some(info) = info {
        let pid = unsafe { GetProcessId(info.hProcess) };
        let mut reg = REGISTRY.lock().unwrap();
        if tag == "high" {
            reg.app_high_pid = pid;
        } else {
            reg.app_medium_pid = pid;
        }
    } else {
        report_err("svc", &format!("app_{tag}_launch"), "create_process");
    }
    let _ = core_token;
    unsafe { CloseHandle(link_w) };
}

fn exit_code(process: HANDLE) -> String {
    let mut code = 0u32;
    unsafe {
        WaitForSingleObject(process, u32::MAX);
        GetExitCodeProcess(process, &mut code);
    }
    format!("{code:#x}")
}
