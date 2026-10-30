use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The X11 window GPUI draws into, which native windows are made children of. None on Wayland.
pub fn window_x11(window: &Window) -> Option<u32> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Xcb(xcb) => Some(xcb.window.get()),
        RawWindowHandle::Xlib(xlib) => u32::try_from(xlib.window).ok(),
        _ => None,
    }
}
