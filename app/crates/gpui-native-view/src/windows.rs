use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The window GPUI draws into, by its handle, which native windows are made children of.
pub fn window_hwnd(window: &Window) -> Option<isize> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
        _ => None,
    }
}
