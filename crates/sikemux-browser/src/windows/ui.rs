//! The thread the app's window runs on, which every page lives on. The app names it once,
//! from that thread, before any tab opens; work from other threads is posted to a hidden
//! window there and runs between the window's own messages.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use sikemux_process::sync::Locked;
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, PostMessageW, RegisterClassW, HWND_MESSAGE, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WNDCLASSW,
};

type Work = Box<dyn FnOnce() + Send>;

const RUN_WORK: u32 = WM_APP + 0x51;

struct Ui {
    thread: u32,
    /// The hidden window work is posted to, as a number since a handle may not cross threads.
    inbox: isize,
}

static UI: OnceLock<Ui> = OnceLock::new();
static WORK: Mutex<VecDeque<Work>> = Mutex::new(VecDeque::new());

/// Makes this thread, which must run the app's window, the one pages are made and driven on.
pub fn run_pages_on_this_thread() {
    UI.get_or_init(|| {
        // SAFETY: plain Win32 calls on this thread; the class and its procedure live as long
        // as the program, and COM may already be set up here by the window itself.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let instance = GetModuleHandleW(None)
                .map(|module| HINSTANCE(module.0))
                .unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(inbox),
                hInstance: instance,
                lpszClassName: w!("SikemuxPageWork"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let window = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("SikemuxPageWork"),
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            );
            Ui {
                thread: GetCurrentThreadId(),
                inbox: window.map(|window| window.0 as isize).unwrap_or(0),
            }
        }
    });
}

pub(crate) fn started() -> bool {
    UI.get().is_some_and(|ui| ui.inbox != 0)
}

pub(crate) fn here() -> bool {
    // SAFETY: reads this thread's id.
    UI.get()
        .is_some_and(|ui| ui.thread == unsafe { GetCurrentThreadId() })
}

/// Runs `work` on the window's thread: at once when called there, else between its messages.
pub(crate) fn on_ui(work: impl FnOnce() + Send + 'static) {
    if here() {
        return work();
    }
    let Some(ui) = UI.get().filter(|ui| ui.inbox != 0) else {
        return;
    };
    WORK.locked().push_back(Box::new(work));
    // SAFETY: the hidden window lives as long as the program.
    let _ = unsafe {
        PostMessageW(
            Some(HWND(ui.inbox as *mut _)),
            RUN_WORK,
            WPARAM(0),
            LPARAM(0),
        )
    };
}

unsafe extern "system" fn inbox(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message != RUN_WORK {
        // SAFETY: the system's default handling of a message to this window.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    while let Some(work) = WORK.locked().pop_front() {
        work();
    }
    LRESULT(0)
}

/// Something only the window's thread may touch, carried to it from another.
pub(crate) struct OnUi<T>(pub(crate) T);

impl<T> OnUi<T> {
    /// Taken whole, so a closure moving it to the window's thread carries the wrapper and
    /// not just what is inside.
    pub(crate) fn into_inner(self) -> T {
        self.0
    }
}

// SAFETY: what is inside is only reached on the window's thread, through `on_ui`.
unsafe impl<T> Send for OnUi<T> {}
