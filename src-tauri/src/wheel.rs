//! macOS is the only thing that knows whether a hand is still on the trackpad.
//!
//! A `wheel` event in the window carries how far the scroll went and nothing
//! about the hand behind it, so the same silence means both "holding still part
//! way through" and "let go". The scroll events macOS builds those from say
//! which it is, so watch them and tell the window each time a hand lands or
//! lifts.

/// Carries `true` when a hand lands on the trackpad and `false` when it lifts.
pub const TOUCH_EVENT: &str = "wheel-touch";

#[cfg(target_os = "macos")]
mod imp {
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicI8, Ordering};

    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase};
    use tauri::{AppHandle, Emitter, Runtime};

    /// The phases a scroll event carries while the fingers are still down. The
    /// glide after they lift carries none of them, and a mouse wheel has no
    /// phase at all, so both read as nothing being held.
    fn touching(phase: NSEventPhase) -> bool {
        phase.intersects(
            NSEventPhase::MayBegin
                | NSEventPhase::Began
                | NSEventPhase::Stationary
                | NSEventPhase::Changed,
        )
    }

    pub fn watch<R: Runtime>(app: &AppHandle<R>) {
        let app = app.clone();
        // Starts as neither, so the first scroll of all is reported whichever way
        // it goes and the window stops having to guess whether anyone is watching.
        let was = AtomicI8::new(-1);
        let monitor = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the monitor a live event for the length of the call.
            let down = touching(unsafe { event.as_ref().phase() });
            if was.swap(i8::from(down), Ordering::Relaxed) != i8::from(down) {
                let _ = app.emit_to("main", super::TOUCH_EVENT, down);
            }
            // Handing the event straight back leaves the scroll itself untouched.
            event.as_ptr()
        });
        // SAFETY: `watch` runs in Tauri's setup, on the main thread, and the block is
        // never freed (see below), so AppKit can keep calling it.
        unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::ScrollWheel,
                &monitor,
            );
        }
        // AppKit calls this for as long as the app runs and there is no taking it
        // off again, so the block outlives everything that could own it here.
        std::mem::forget(monitor);
    }
}

#[cfg(target_os = "macos")]
pub use imp::watch;

#[cfg(not(target_os = "macos"))]
pub fn watch<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}
