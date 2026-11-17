use std::fs::File;
use std::io::PipeWriter;
use std::os::fd::{AsRawFd, FromRawFd};

/// Points stderr at `pipe`. Hands back where stderr printed before, or None when it could
/// not be moved and still prints there.
pub(super) fn redirect(pipe: PipeWriter) -> Option<Option<File>> {
    // SAFETY: dup takes and returns plain descriptors; stderr stays open until dup2 replaces it.
    let printed = unsafe { libc::dup(libc::STDERR_FILENO) };
    if printed < 0 {
        return None;
    }
    // SAFETY: both descriptors are open; dup2 points stderr at the pipe, and dropping `pipe`
    // afterwards closes only its own copy.
    if unsafe { libc::dup2(pipe.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
        // SAFETY: `printed` was opened above and nothing else holds it.
        unsafe { libc::close(printed) };
        return None;
    }
    // SAFETY: `printed` was opened above and is owned by this File from here on.
    Some(Some(unsafe { File::from_raw_fd(printed) }))
}
