use std::io;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
};

use crate::descriptor::wide;

/// Asks the person a yes or no question in a plain Windows dialog of this process's own. At
/// High, the person's other programs can neither type into it nor press its buttons. No is the
/// default, and the call waits until they answer.
pub fn confirm(text: &str) -> io::Result<bool> {
    let (title, text) = (wide("Sikemux"), wide(text));
    // SAFETY: both strings end in a nul and live through the call.
    let pressed = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    match pressed {
        0 => Err(io::Error::last_os_error()),
        pressed => Ok(pressed == IDYES),
    }
}
