//! Whether anybody is using this host, so phones are told only while nobody
//! is.

pub(crate) trait Presence: Send + Sync {
    /// `None` when this host cannot tell, which counts as away.
    fn away(&self) -> Option<bool>;
}

pub(crate) struct SystemPresence;

#[cfg(target_os = "macos")]
impl Presence for SystemPresence {
    fn away(&self) -> Option<bool> {
        let locked = mac::screen_locked()?;
        Some(locked || mac::idle_seconds() >= mac::AWAY_AFTER_SECONDS)
    }
}

#[cfg(not(target_os = "macos"))]
impl Presence for SystemPresence {
    fn away(&self) -> Option<bool> {
        None
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_char, c_void};

    /// Untouched this long, the host counts as away.
    pub(super) const AWAY_AFTER_SECONDS: f64 = 120.0;

    const COMBINED_SESSION_STATE: i32 = 0;
    const ANY_INPUT_EVENT: u32 = u32::MAX;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
        fn CGSessionCopyCurrentDictionary() -> *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFGetTypeID(value: *const c_void) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: *const c_void) -> u8;
        fn CFRelease(value: *const c_void);
    }

    /// Seconds since the last key press, click or mouse move in this login
    /// session.
    pub(super) fn idle_seconds() -> f64 {
        // SAFETY: a pure query of the window server with constant arguments.
        unsafe { CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION_STATE, ANY_INPUT_EVENT) }
    }

    /// `None` outside a login session with a window server, where nothing
    /// about the person's presence can be read.
    pub(super) fn screen_locked() -> Option<bool> {
        // SAFETY: returns an owned dictionary or null; it is released below.
        let session = unsafe { CGSessionCopyCurrentDictionary() };
        if session.is_null() {
            return None;
        }
        // SAFETY: the text is a NUL-terminated literal; the string is owned
        // and released below.
        let key = unsafe {
            CFStringCreateWithCString(std::ptr::null(), c"CGSSessionScreenIsLocked".as_ptr(), UTF8)
        };
        let locked = if key.is_null() {
            false
        } else {
            // SAFETY: both are live CF objects; the value is borrowed from the
            // dictionary and read only while the dictionary is alive.
            unsafe {
                let value = CFDictionaryGetValue(session, key);
                let locked = !value.is_null()
                    && CFGetTypeID(value) == CFBooleanGetTypeID()
                    && CFBooleanGetValue(value) != 0;
                CFRelease(key);
                locked
            }
        };
        // SAFETY: the dictionary came from a Copy function, so it is ours.
        unsafe { CFRelease(session) };
        Some(locked)
    }
}
