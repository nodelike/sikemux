use std::collections::HashMap;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Services::{OpenSCManagerW, OpenServiceW, SERVICE_STOP};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenThread, TerminateProcess, PROCESS_CREATE_THREAD, PROCESS_DUP_HANDLE,
    PROCESS_QUERY_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VK_CONTROL,
    VK_V,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_CHAR};

use crate::pipe::{client, create_named, recv_line, send_line};
use crate::util::{install_dir, report, SVC_PIPE};

const PROBE: u32 =
    PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_CREATE_THREAD | PROCESS_DUP_HANDLE | 0x40000 | PROCESS_QUERY_INFORMATION;

fn status() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let h = client(SVC_PIPE);
    if h.is_null() {
        return map;
    }
    send_line(h, "status");
    let line = recv_line(h).unwrap_or_default();
    unsafe { CloseHandle(h) };
    for field in line.split(';') {
        if let Some((k, v)) = field.split_once('=') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

fn probe_process(tag: &str, pid: u32) {
    if pid == 0 {
        report("agent", &format!("open_{tag}"), "no-pid");
        return;
    }
    let h = unsafe { OpenProcess(PROBE, 0, pid) };
    report("agent", &format!("open_{tag}"), if h.is_null() { "denied" } else { "allowed" });
    if !h.is_null() {
        unsafe { CloseHandle(h) };
    }
    let t = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    report("agent", &format!("terminate_{tag}"), if t.is_null() { "denied" } else { "allowed" });
    if !t.is_null() {
        unsafe { CloseHandle(t) };
    }
}

fn probe_threads(core_pid: u32) {
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let (mut allowed, mut denied) = (0, 0);
    let mut ok = unsafe { Thread32First(snap, &mut entry) };
    while ok != 0 {
        if entry.th32OwnerProcessID == core_pid {
            let h = unsafe { OpenThread(0x0002 | 0x0010, 0, entry.th32ThreadID) }; // SUSPEND_RESUME|SET_CONTEXT
            if h.is_null() {
                denied += 1;
            } else {
                allowed += 1;
                unsafe { CloseHandle(h) };
            }
        }
        ok = unsafe { Thread32Next(snap, &mut entry) };
    }
    unsafe { CloseHandle(snap) };
    report("agent", "thread_open", &format!("allowed={allowed},denied={denied}"));
}

fn set_clipboard(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return;
        }
        EmptyClipboard();
        let bytes = wide.len() * 2;
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        let dst = GlobalLock(mem) as *mut u16;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
        GlobalUnlock(mem);
        SetClipboardData(13, mem as HANDLE);
        CloseClipboard();
    }
}

fn key_event(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_input_burst() {
    let mut inputs = Vec::new();
    for ch in "AGENTTYPE".encode_utf16() {
        inputs.push(key_event(0, ch, KEYEVENTF_UNICODE));
        inputs.push(key_event(0, ch, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    inputs.push(key_event(VK_CONTROL, 0, 0));
    inputs.push(key_event(VK_V, 0, 0));
    inputs.push(key_event(VK_V, 0, KEYEVENTF_KEYUP));
    inputs.push(key_event(VK_CONTROL, 0, KEYEVENTF_KEYUP));
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    report("agent", "sendinput", &format!("sent={sent}"));
}

fn poke_window(tag: &str, hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let hwnd = hwnd as HANDLE;
    for _ in 0..5 {
        unsafe { PostMessageW(hwnd, WM_CHAR, 0x41, 0) };
    }
    let mut result = 0usize;
    unsafe { SendMessageTimeoutW(hwnd, WM_CHAR, 0x42, 0, SMTO_ABORTIFHUNG, 1000, &mut result) };
    report("agent", &format!("poke_{tag}"), "sent");
}

pub fn run() {
    let st = status();
    let pid = |k: &str| st.get(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let hwnd = |k: &str| st.get(k).and_then(|v| v.parse::<isize>().ok()).unwrap_or(0);
    let core_pid = pid("core");

    // Secrets file: the agent is the same person but not SYSTEM, so it must be refused.
    let secret_read = std::fs::read_dir(crate::util::secrets_dir())
        .map(|dirs| {
            dirs.flatten()
                .any(|d| std::fs::read_to_string(d.path().join("secret.txt")).is_ok())
        })
        .unwrap_or(false);
    report("agent", "read_secret_file", if secret_read { "allowed" } else { "denied" });

    probe_process("core", core_pid);
    probe_process("app_high", pid("app_high"));
    probe_process("app_medium", pid("app_medium"));
    probe_threads(core_pid);

    set_clipboard("AGENT_PASTE");
    poke_window("app_high", hwnd("hwnd_high"));
    poke_window("app_medium", hwnd("hwnd_medium"));
    send_input_burst();

    // Ask the service for a secret directly, by name: must be refused.
    let h = client(SVC_PIPE);
    if !h.is_null() {
        send_line(h, "secret");
        report("agent", "direct_secret", &recv_line(h).unwrap_or_default());
        unsafe { CloseHandle(h) };
    }

    // Try to squat a new instance of the service pipe: no create-instance right.
    let squat = create_named(SVC_PIPE, "D:P(A;;GA;;;WD)");
    report("agent", "squat_service_pipe", if squat.is_null() { "denied" } else { "allowed" });

    // Try to stop the service.
    let scm = unsafe { OpenSCManagerW(null(), null(), 0x0001) }; // SC_MANAGER_CONNECT
    let svc = if scm.is_null() {
        null_mut()
    } else {
        let name = crate::util::wide(crate::util::SERVICE_NAME);
        unsafe { OpenServiceW(scm, name.as_ptr(), SERVICE_STOP) }
    };
    report("agent", "service_stop", if svc.is_null() { "denied" } else { "allowed" });

    // Program Files is out of reach; the person's own LOCALAPPDATA is not.
    let pf = std::fs::write(install_dir().join("agent.txt"), "x");
    report("agent", "write_progfiles", if pf.is_ok() { "allowed" } else { "denied" });
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let f = std::path::Path::new(&local).join("SikemuxSpike/core-file.txt");
        let w = std::fs::write(f, "agent-wrote");
        report("agent", "write_user_file", if w.is_ok() { "allowed" } else { "denied" });
    }

    // Start the installed core ourselves: with no brokered channel it is refused.
    let orphan = std::process::Command::new(crate::util::install_exe()).arg("core").spawn();
    if let Ok(mut child) = orphan {
        let _ = child.wait();
        report("agent", "launched_core", "done");
    }

    // Finally, kill the real core. The service must bring it back.
    if core_pid != 0 {
        let h = unsafe { OpenProcess(PROCESS_TERMINATE, 0, core_pid) };
        if !h.is_null() {
            unsafe {
                TerminateProcess(h, 1);
                CloseHandle(h);
            }
            report("agent", "kill_core", "done");
        } else {
            report("agent", "kill_core", "denied");
        }
    }
}
