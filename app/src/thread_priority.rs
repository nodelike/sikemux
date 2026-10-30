//! How urgently macOS runs the app's own threads that feed the window. A thread left at the
//! default can be put on the slower cores of a busy machine, and every frame waiting on it waits
//! longer, so those threads ask for the window's own class or just under it.

/// For a thread a frame waits on, such as the one that talks to the core.
pub fn interactive() {
    #[cfg(target_os = "macos")]
    set(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE);
}

/// For work the person is waiting on that no frame waits for, such as a browser tool call.
pub fn initiated() {
    #[cfg(target_os = "macos")]
    set(libc::qos_class_t::QOS_CLASS_USER_INITIATED);
}

#[cfg(target_os = "macos")]
fn set(class: libc::qos_class_t) {
    // SAFETY: it only changes the calling thread's own scheduling class.
    unsafe {
        libc::pthread_set_qos_class_self_np(class, 0);
    }
}
