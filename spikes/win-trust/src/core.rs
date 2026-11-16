use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Security::{TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessMitigationPolicy, OpenProcess, ProcessSignaturePolicy,
    PROCESS_VM_READ,
};

use crate::pipe::{client, recv_line, send_line};
use crate::proc::{spawn, Spawn};
use crate::sd::{current_integrity, medium_restricted, self_token};
use crate::util::{install_exe, report, SendHandle, SVC_PIPE};

pub fn il_name(rid: u32) -> &'static str {
    match rid {
        0x1000 => "low",
        0x2000 => "medium",
        0x3000 => "high",
        0x4000 => "system",
        _ => "other",
    }
}

fn handle_arg(map: &HashMap<String, String>, key: &str) -> Option<*mut c_void> {
    let v: usize = map.get(key)?.parse().ok()?;
    Some(v as *mut c_void)
}

fn parse(args: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(stripped) = a.strip_prefix("--") {
            if let Some(v) = it.next() {
                map.insert(format!("--{stripped}"), v.clone());
            }
        }
    }
    map
}

pub fn run(args: &[String]) {
    let map = parse(args);
    let Some(secret_w) = handle_arg(&map, "--secret-w") else {
        return orphan();
    };
    let secret_r = handle_arg(&map, "--secret-r").unwrap_or(null_mut());

    report("core", "core_gen", map.get("--gen").map(String::as_str).unwrap_or("?"));
    report("core", "core_il", il_name(current_integrity().unwrap_or(0)));

    send_line(secret_w, "put hunter2");
    let _ = recv_line(secret_r);
    send_line(secret_w, "get");
    let got = recv_line(secret_r).unwrap_or_default();
    report("core", "secret", if got == "ok hunter2" { "ok" } else { "mismatch" });

    let pid = unsafe { GetCurrentProcessId() };
    let self_by_id = unsafe { OpenProcess(PROCESS_VM_READ, 0, pid) };
    report(
        "core",
        "self_open_byid",
        if self_by_id.is_null() { "denied" } else { "allowed" },
    );

    for _ in 0..3 {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        });
    }

    let tok = self_token(TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT);
    let med = medium_restricted(tok);
    if !med.is_null() {
        let child = Spawn {
            token: med,
            cmdline: format!("\"{}\" child", install_exe().display()),
            inherit: &[],
            mitigation: None,
            visible_desktop: false,
            process_sddl: None,
        };
        report("core", "child_spawn", if spawn(&child).is_some() { "started" } else { "failed" });
    }

    let med2 = medium_restricted(tok);
    match crate::conpty::run(med2) {
        Ok(out) => {
            let ok = out.contains("Medium Mandatory Level") && out.contains("conpty-ok");
            report("core", "terminal", if ok { "ok-medium" } else { "partial" });
        }
        Err(e) => report("core", "terminal", &format!("err({e})")),
    }

    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let dir = std::path::Path::new(&local).join("SikemuxSpike");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("core-file.txt"), "core");
    }

    let mut buf = [0u8; 4];
    let got = unsafe {
        GetProcessMitigationPolicy(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            ProcessSignaturePolicy,
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
        )
    };
    let signed_only = got != 0 && (buf[0] & 1) == 1;
    report("core", "sig_mitigation", if signed_only { "ms-signed-only" } else { "off" });

    if let Some(app_hi) = handle_arg(&map, "--app-hi") {
        let app_med = handle_arg(&map, "--app-med").unwrap_or(null_mut());
        let hi = SendHandle(app_hi);
        let md = SendHandle(app_med);
        std::thread::spawn(move || {
            let (hi, md) = (hi, md);
            let a = recv_line(hi.0).unwrap_or_default();
            let b = recv_line(md.0).unwrap_or_default();
            let ok = a.starts_with("hello") && b.starts_with("hello");
            report("core", "app_link", if ok { "ok" } else { "partial" });
        });
    }

    report("core", "core_ready", "yes");
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Started by the agent, with no brokered channel: the only way left to reach a
/// secret is to ask the service by name, which must refuse.
fn orphan() {
    report("core", "agent_started_core", "running");
    let h = client(SVC_PIPE);
    if !h.is_null() {
        send_line(h, "secret");
        let reply = recv_line(h).unwrap_or_default();
        report("core", "agent_core_secret", if reply.is_empty() { "no-reply" } else { reply.as_str() });
        unsafe { CloseHandle(h) };
    } else {
        report("core", "agent_core_secret", "no-pipe");
    }
    std::process::exit(3);
}
