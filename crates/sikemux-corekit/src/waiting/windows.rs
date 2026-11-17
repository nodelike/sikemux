//! The mark is an empty file, and its name names an event in this session's namespace, which
//! lasts while its waiter holds it.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};

pub fn keep_private(_folder: &Path) -> io::Result<()> {
    Ok(())
}

fn event_name(path: &Path) -> Vec<u16> {
    let mark = path.file_name().unwrap_or_default().to_string_lossy();
    format!(r"Local\sikemux-waiting-{mark}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

pub struct Mark(HANDLE);

impl Mark {
    pub fn leave(path: &Path) -> io::Result<Self> {
        let name = event_name(path);
        // SAFETY: `name` ends in a nul; the event resets itself once a wait takes it.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mark = Self(event);
        fs::File::create(path)?;
        Ok(mark)
    }

    pub fn wait(&self, left: Duration) -> bool {
        let millis = u32::try_from(left.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: the event is open until this mark drops.
        unsafe { WaitForSingleObject(self.0, millis) == WAIT_OBJECT_0 }
    }
}

impl Drop for Mark {
    fn drop(&mut self) {
        // SAFETY: the event was opened in `leave` and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

pub struct Waker;

impl Waker {
    pub fn new() -> Option<Self> {
        Some(Self)
    }

    /// False when nobody waits at `path` any more.
    pub fn wake(&self, path: &Path) -> bool {
        let name = event_name(path);
        // SAFETY: `name` ends in a nul.
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if event.is_null() {
            return false;
        }
        // SAFETY: `event` was opened above with the right to set it, and is closed once.
        unsafe {
            SetEvent(event);
            CloseHandle(event);
        }
        true
    }
}
