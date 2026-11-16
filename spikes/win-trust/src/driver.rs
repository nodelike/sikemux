use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Security::{
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
    OpenServiceW, StartServiceW, SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS, SERVICE_CONTROL_STOP,
    SERVICE_DEMAND_START, SERVICE_ERROR_NORMAL, SERVICE_STATUS, SERVICE_WIN32_OWN_PROCESS,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessWithLogonW, PROCESS_INFORMATION, STARTUPINFOW,
};

use crate::pipe::{client, recv_line, send_line};
use crate::proc::{spawn, Spawn};
use crate::sd::{deny_admins, self_token, set_integrity, to_primary, MEDIUM_RID};
use crate::util::{
    data_dir, install_dir, install_exe, log_path, report, secrets_dir, wide, SERVICE_NAME, SVC_PIPE,
};

const USER_B: &str = "spikeb";
const PASS_B: &str = "Sp1ke!Pass_B9";

struct Checks {
    rows: Vec<(String, &'static str, bool)>,
}
impl Checks {
    fn new() -> Self {
        Checks { rows: Vec::new() }
    }
    fn want(&mut self, log: &str, key: &str, expect: &'static str) {
        let line = log.lines().rev().find(|l| l.contains(&format!("{key}=")));
        let pass = line
            .map(|l| l.contains(&format!("{key}={expect}")))
            .unwrap_or(false);
        self.rows.push((key.to_string(), expect, pass));
    }
    fn want_present(&mut self, log: &str, key: &str) {
        let pass = log.lines().any(|l| l.contains(&format!("{key}=")));
        self.rows.push((key.to_string(), "<present>", pass));
    }
}

fn stage() {
    let _ = std::fs::create_dir_all(install_dir());
    let _ = std::fs::create_dir_all(secrets_dir());
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::remove_file(log_path());
    let me = std::env::current_exe().expect("current exe");
    std::fs::copy(&me, install_exe()).expect("copy spike.exe to Program Files");
}

fn open_scm() -> *mut c_void {
    let h = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_ALL_ACCESS) };
    assert!(!h.is_null(), "OpenSCManagerW failed (run elevated)");
    h
}

fn install_service() {
    let scm = open_scm();
    let name = wide(SERVICE_NAME);
    let bin = wide(&format!("\"{}\" service", install_exe().display()));
    let svc = unsafe {
        CreateServiceW(
            scm,
            name.as_ptr(),
            name.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            bin.as_ptr(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        )
    };
    let svc = if svc.is_null() {
        unsafe { OpenServiceW(scm, name.as_ptr(), SERVICE_ALL_ACCESS) }
    } else {
        svc
    };
    assert!(!svc.is_null(), "CreateService/OpenService failed");
    let ok = unsafe { StartServiceW(svc, 0, null()) };
    report(
        "driver",
        "service_start",
        if ok != 0 { "ok" } else { "already-or-fail" },
    );
    unsafe {
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
    }
}

fn remove_service() {
    let scm = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_ALL_ACCESS) };
    if scm.is_null() {
        return;
    }
    let name = wide(SERVICE_NAME);
    let svc = unsafe { OpenServiceW(scm, name.as_ptr(), SERVICE_ALL_ACCESS) };
    if !svc.is_null() {
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        unsafe {
            ControlService(svc, SERVICE_CONTROL_STOP, &mut status);
            DeleteService(svc);
            CloseServiceHandle(svc);
        }
    }
    unsafe { CloseServiceHandle(scm) };
}

fn wait_for_service() -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let h = client(SVC_PIPE);
        if !h.is_null() {
            unsafe { CloseHandle(h) };
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

fn ask_start() {
    let h = client(SVC_PIPE);
    if h.is_null() {
        report("driver", "start_request", "no-pipe");
        return;
    }
    send_line(h, "start");
    report("driver", "start_request", &recv_line(h).unwrap_or_default());
    unsafe { CloseHandle(h) };
}

fn launch_agent() {
    let base =
        self_token(TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT);
    let primary = to_primary(base);
    let locked = deny_admins(primary);
    if locked.is_null() {
        report("driver", "agent_token", "failed");
        return;
    }
    set_integrity(locked, MEDIUM_RID);
    let spawned = spawn(&Spawn {
        token: locked,
        cmdline: format!("\"{}\" agent", install_exe().display()),
        inherit: &[],
        mitigation: None,
        visible_desktop: true,
        process_sddl: None,
    });
    report(
        "driver",
        "agent_launch",
        if spawned.is_some() {
            "started"
        } else {
            "failed"
        },
    );
}

fn second_user() {
    let add = std::process::Command::new("net")
        .args(["user", USER_B, PASS_B, "/add"])
        .output();
    let added = add.map(|o| o.status.success()).unwrap_or(false);
    report(
        "driver",
        "second_user_add",
        if added { "ok" } else { "exists-or-fail" },
    );

    let user = wide(USER_B);
    let domain = wide(".");
    let pass = wide(PASS_B);
    let mut cmd = wide(&format!("\"{}\" second", install_exe().display()));
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let ok = unsafe {
        CreateProcessWithLogonW(
            user.as_ptr(),
            domain.as_ptr(),
            pass.as_ptr(),
            1, // LOGON_WITH_PROFILE
            null(),
            cmd.as_mut_ptr(),
            0,
            null(),
            null(),
            &startup,
            &mut info,
        )
    };
    report(
        "driver",
        "second_launch",
        if ok != 0 { "started" } else { "failed" },
    );
}

/// Run as a second signed-in person (via CreateProcessWithLogonW). Asks the
/// service to stand up a core for this new SID, proving per-person isolation.
pub fn second() {
    let h = client(SVC_PIPE);
    if h.is_null() {
        report("second", "second_start", "no-pipe");
        return;
    }
    send_line(h, "start");
    report("second", "second_start", &recv_line(h).unwrap_or_default());
    unsafe { CloseHandle(h) };
}

fn read_log() -> String {
    std::fs::read_to_string(log_path()).unwrap_or_default()
}

fn wait_for_outcome() -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let log = read_log();
        let done = log.contains("kill_core=") && log.contains("core_gen=2");
        if done || Instant::now() >= deadline {
            return log;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn summarize(log: &str) -> bool {
    let mut c = Checks::new();
    // Proof 1: a non-admin program of the same person cannot read the secrets.
    c.want(log, "read_secret_file", "denied");
    // Proof 2: it cannot open the service-started core/high app for tampering.
    c.want(log, "open_core", "denied");
    c.want(log, "open_app_high", "denied");
    c.want(log, "thread_open", "allowed=0");
    // Proof 3: refused secrets when it starts the core itself, and by asking directly.
    c.want(log, "agent_core_secret", "refused");
    c.want(log, "direct_secret", "refused");
    c.want(log, "squat_service_pipe", "denied");
    c.want(log, "service_stop", "denied");
    c.want(log, "write_progfiles", "denied");
    // Proof 4: the real core works, apps draw, a terminal runs Medium, restart holds.
    c.want(log, "core_il", "high");
    c.want(log, "secret", "ok");
    c.want(log, "terminal", "ok-medium");
    c.want_present(log, "app_high_window");
    c.want_present(log, "app_medium_window");
    c.want(log, "app_link", "ok");
    c.want(log, "child_il", "medium");
    c.want(log, "core_gen", "2"); // restart brought it back
                                  // Signed-in counts and two-person isolation.
    c.want(log, "idle_users", "0");
    c.want(log, "second_start", "ok");

    let mut all = true;
    println!("\n===== WINDOWS TRUST SPIKE SUMMARY =====");
    for (key, expect, pass) in &c.rows {
        all &= pass;
        println!(
            "[{}] {key} -> want {expect}",
            if *pass { "PASS" } else { "FAIL" }
        );
    }
    println!("======================================");
    println!(
        "secret dirs (signed-in people): {}",
        crate::driver::secret_dir_count()
    );
    println!("RESULT: {}", if all { "PASS" } else { "FAIL" });
    all
}

pub fn secret_dir_count() -> usize {
    std::fs::read_dir(secrets_dir())
        .map(|d| d.flatten().count())
        .unwrap_or(0)
}

fn cleanup() {
    remove_service();
    let _ = std::process::Command::new("net")
        .args(["user", USER_B, "/delete"])
        .output();
    let _ = std::fs::remove_dir_all(install_dir());
}

pub fn run() {
    report("driver", "stage", "begin");
    stage();
    install_service();
    if !wait_for_service() {
        report("driver", "service", "never-came-up");
        eprintln!("RESULT: FAIL (service did not start)");
        cleanup();
        std::process::exit(1);
    }
    ask_start();
    std::thread::sleep(Duration::from_secs(2));
    second_user();
    std::thread::sleep(Duration::from_secs(2));
    launch_agent();

    let log = wait_for_outcome();
    let ok = summarize(&log);
    cleanup();
    std::process::exit(if ok { 0 } else { 1 });
}
