use std::fs::File;
use std::io::PipeWriter;
use std::os::windows::io::{FromRawHandle, IntoRawHandle};

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};

/// Points stderr at `pipe`. Hands back where stderr printed before, which a window app
/// started without a console does not have, or None when it could not be moved.
pub(super) fn redirect(pipe: PipeWriter) -> Option<Option<File>> {
    // SAFETY: it only reads the process's standard error handle.
    let printed = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    let pipe = pipe.into_raw_handle();
    // SAFETY: `pipe` is an open handle that stderr owns from here on, for the life of the process.
    if unsafe { SetStdHandle(STD_ERROR_HANDLE, pipe) } == 0 {
        // SAFETY: the handle came from `into_raw_handle` above and nothing else holds it.
        drop(unsafe { File::from_raw_handle(pipe) });
        return None;
    }
    if printed.is_null() || printed == INVALID_HANDLE_VALUE {
        return Some(None);
    }
    // SAFETY: stderr no longer refers to the old handle, so this File is its only owner.
    Some(Some(unsafe { File::from_raw_handle(printed) }))
}
