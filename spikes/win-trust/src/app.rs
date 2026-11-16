use std::ffi::c_void;
use std::num::NonZeroIsize;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::GlobalLock;
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, PROCESS_VM_READ, PROCESS_VM_WRITE};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, FillRect, UpdateWindow, COLOR_WINDOW, PAINTSTRUCT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, ShowWindow, TranslateMessage, CW_USEDEFAULT, MSG, SW_SHOW, WM_CHAR,
    WM_DESTROY, WM_PAINT, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use wry::{WebContext, WebViewBuilder};

use crate::pipe::send_line;
use crate::sd::current_integrity;
use crate::util::{report, wide};

static CHARS: AtomicU32 = AtomicU32::new(0);
static TAG: OnceLock<String> = OnceLock::new();

fn tag() -> &'static str {
    TAG.get().map(String::as_str).unwrap_or("app")
}

fn key(name: &str) -> String {
    format!("app_{}_{name}", tag())
}

struct Win {
    hwnd: isize,
    hinstance: isize,
}
impl HasWindowHandle for Win {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let mut h = Win32WindowHandle::new(NonZeroIsize::new(self.hwnd).ok_or(HandleError::Unavailable)?);
        h.hinstance = NonZeroIsize::new(self.hinstance);
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(h)) })
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            FillRect(hdc, &ps.rcPaint, (COLOR_WINDOW + 1) as _);
            EndPaint(hwnd, &ps);
            0
        }
        WM_CHAR => {
            if wp as u32 == 0x16 {
                report(tag(), &key("clipboard"), &read_clipboard());
            } else {
                CHARS.fetch_add(1, Ordering::SeqCst);
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn read_clipboard() -> String {
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return "open-failed".into();
        }
        let h = GetClipboardData(13); // CF_UNICODETEXT
        let text = if h.is_null() {
            "empty".to_string()
        } else {
            let p = GlobalLock(h) as *const u16;
            let s = crate::util::from_wide_ptr(p);
            s.chars().take(16).collect()
        };
        CloseClipboard();
        text
    }
}

fn create_window() -> HWND {
    let hinstance = unsafe { GetModuleHandleW(null_mut()) };
    let class = wide(&format!("SikemuxSpikeApp-{}", tag()));
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null_mut(),
        lpszClassName: class.as_ptr(),
    };
    unsafe { RegisterClassW(&wc) };
    let title = wide(&format!("Sikemux Spike {}", tag()));
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            600,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        )
    };
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetForegroundWindow(hwnd);
    }
    hwnd
}

fn try_d3d() -> String {
    type Create = unsafe extern "system" fn(
        *mut c_void,
        u32,
        isize,
        u32,
        *const u32,
        u32,
        u32,
        *mut *mut c_void,
        *mut u32,
        *mut *mut c_void,
    ) -> i32;
    unsafe {
        let lib = LoadLibraryW(wide("d3d11.dll").as_ptr());
        if lib.is_null() {
            return "d3d11-blocked".into();
        }
        let proc = GetProcAddress(lib, c"D3D11CreateDevice".as_ptr() as *const u8);
        let Some(proc) = proc else {
            return "no-entry".into();
        };
        let create: Create = std::mem::transmute(proc);
        let mut device: *mut c_void = null_mut();
        let mut ctx: *mut c_void = null_mut();
        let mut level = 0u32;
        // driver type 5 = WARP (software, Microsoft-signed).
        let hr = create(null_mut(), 5, 0, 0, null_mut(), 0, 7, &mut device, &mut level, &mut ctx);
        if hr == 0 {
            "warp-ok".into()
        } else {
            format!("warp-hr({hr:#x})")
        }
    }
}

pub fn run(args: &[String]) {
    let name = args.first().cloned().unwrap_or_else(|| "app".into());
    let _ = TAG.set(name);

    let mut link: HANDLE = null_mut();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--link" {
            if let Some(v) = it.next() {
                link = v.parse::<usize>().unwrap_or(0) as HANDLE;
            }
        }
    }
    if !link.is_null() {
        send_line(link, &format!("hello-{}", tag()));
    }

    report(tag(), &key("il"), crate::core::il_name(current_integrity().unwrap_or(0)));

    let hwnd = create_window();
    report(tag(), &key("hwnd"), &(hwnd as isize).to_string());
    report(tag(), &key("window"), "ok");
    report(tag(), &key("d3d"), &try_d3d());

    let pid = unsafe { GetCurrentProcessId() };
    let self_open = unsafe { OpenProcess(PROCESS_VM_READ | PROCESS_VM_WRITE, 0, pid) };
    report(tag(), &key("self_open"), if self_open.is_null() { "denied" } else { "allowed" });

    let data_dir = std::env::var("LOCALAPPDATA")
        .map(|l| std::path::Path::new(&l).join(format!("SikemuxSpike/webview-{}", tag())))
        .ok();
    let mut web_context = WebContext::new(data_dir);
    let html = "<html><body style='background:#204'>spike</body>\
        <script>window.ipc.postMessage('loaded')</script></html>";
    let built = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_html(html)
        .with_ipc_handler(|req| {
            if req.body().as_str() == "loaded" {
                report(tag(), &key("webview"), "loaded");
            }
        })
        .build(&Win {
            hwnd: hwnd as isize,
            hinstance: unsafe { GetModuleHandleW(null_mut()) } as isize,
        });
    let _webview = match built {
        Ok(w) => {
            report(tag(), &key("webview_build"), "ok");
            Some(w)
        }
        Err(e) => {
            report(tag(), &key("webview_build"), &format!("err({e})"));
            None
        }
    };

    std::thread::spawn(|| {
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            report(tag(), &key("chars"), &CHARS.load(Ordering::SeqCst).to_string());
        }
    });

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
