//! How short of memory the system is, told by the kernel as it changes.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressure {
    Normal,
    Warn,
    Critical,
}

/// Calls `changed` with the pressure now and each time it changes, from a
/// thread of its own. Does nothing where the system cannot tell.
pub fn watch(changed: impl Fn(Pressure) + Send + Sync + 'static) {
    platform::watch(Box::new(changed));
}

type Changed = Box<dyn Fn(Pressure) + Send + Sync>;

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_int, c_void};

    use super::{Changed, Pressure};

    const NORMAL: usize = 1;
    const WARN: usize = 2;
    const CRITICAL: usize = 4;

    #[repr(C)]
    struct SourceType {
        _private: [u8; 0],
    }

    extern "C" {
        static _dispatch_source_type_memorypressure: SourceType;
        fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
        fn dispatch_source_create(
            kind: *const SourceType,
            handle: usize,
            mask: usize,
            queue: *mut c_void,
        ) -> *mut c_void;
        fn dispatch_set_context(object: *mut c_void, context: *mut c_void);
        fn dispatch_source_set_event_handler_f(
            source: *mut c_void,
            handler: extern "C" fn(*mut c_void),
        );
        fn dispatch_source_get_data(source: *mut c_void) -> usize;
        fn dispatch_resume(object: *mut c_void);
    }

    struct Watch {
        source: *mut c_void,
        changed: Changed,
    }

    pub(super) fn watch(changed: Changed) {
        changed(now());
        // SAFETY: the source and its context live for the rest of the process:
        // neither is ever released, so the handler never sees a freed one.
        unsafe {
            let source = dispatch_source_create(
                &_dispatch_source_type_memorypressure,
                0,
                NORMAL | WARN | CRITICAL,
                dispatch_get_global_queue(0, 0),
            );
            if source.is_null() {
                return;
            }
            let watch = Box::into_raw(Box::new(Watch { source, changed }));
            dispatch_set_context(source, watch.cast());
            dispatch_source_set_event_handler_f(source, fired);
            dispatch_resume(source);
        }
    }

    extern "C" fn fired(context: *mut c_void) {
        // SAFETY: the context is the `Watch` that `watch` leaked for this source.
        let watch = unsafe { &*context.cast::<Watch>() };
        // SAFETY: the source is alive, as above.
        let level = unsafe { dispatch_source_get_data(watch.source) };
        (watch.changed)(pressure(level));
    }

    fn now() -> Pressure {
        let mut level: c_int = 0;
        let mut size = size_of::<c_int>();
        // SAFETY: the name is NUL-terminated and `level` has `size` bytes.
        let read = unsafe {
            libc::sysctlbyname(
                c"kern.memorystatus_vm_pressure_level".as_ptr(),
                (&raw mut level).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if read == 0 {
            pressure(level.max(0) as usize)
        } else {
            Pressure::Normal
        }
    }

    fn pressure(level: usize) -> Pressure {
        if level & CRITICAL != 0 {
            Pressure::Critical
        } else if level & WARN != 0 {
            Pressure::Warn
        } else {
            Pressure::Normal
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::fd::AsRawFd;

    use super::{Changed, Pressure};

    /// Some task stalled on memory for half a second in two, the finest an
    /// unprivileged process may ask the kernel to watch.
    const STALLED: &[u8] = b"some 500000 2000000\0";
    const CALM_AFTER_MS: libc::c_int = 30_000;

    pub(super) fn watch(changed: Changed) {
        changed(Pressure::Normal);
        let _ = std::thread::Builder::new()
            .name("sikemux-memory".into())
            .spawn(move || {
                let Ok(mut pressure) = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("/proc/pressure/memory")
                else {
                    return;
                };
                if pressure.write_all(STALLED).is_err() {
                    return;
                }
                let mut short = false;
                loop {
                    let mut wait = libc::pollfd {
                        fd: pressure.as_raw_fd(),
                        events: libc::POLLPRI,
                        revents: 0,
                    };
                    // SAFETY: `wait` is one valid pollfd.
                    let ready = unsafe { libc::poll(&mut wait, 1, CALM_AFTER_MS) };
                    if ready < 0 || wait.revents & libc::POLLERR != 0 {
                        return;
                    }
                    if (ready > 0) != short {
                        short = ready > 0;
                        changed(if short {
                            Pressure::Warn
                        } else {
                            Pressure::Normal
                        });
                    }
                }
            });
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    pub(super) fn watch(_: super::Changed) {}
}
