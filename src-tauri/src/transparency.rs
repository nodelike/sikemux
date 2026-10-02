// macOS see-through window with adjustable background blur.
//
// Matches nackle's approach (and Terminal.app / iTerm2 / Ghostty): use the
// private CoreGraphics SPI `CGSSetWindowBackgroundBlurRadius` instead of
// `NSVisualEffectView`. CGS blur is a plain gaussian — none of the heavy
// "frosted" vibrancy effect — and the radius is a free integer the user can
// dial.
//
// On non-macOS this module is a no-op.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;

    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGSDefaultConnectionForThread() -> *mut c_void;
        fn CGSSetWindowBackgroundBlurRadius(
            connection: *mut c_void,
            window_number: u32,
            radius: i32,
        ) -> i32;
    }

    /// Apply transparency + CGS blur to an NSWindow.
    ///
    /// `blur_radius` of 0 disables the blur (pure transparency); 20–40 is a
    /// pleasant frosted range; 60+ is heavy.
    ///
    /// SAFETY: `ns_window` must be a live `NSWindow*`.
    pub unsafe fn apply(ns_window: *mut c_void, blur_radius: i32) {
        let window = ns_window as *mut AnyObject;
        if window.is_null() {
            return;
        }

        // [window setOpaque:NO]
        let _: () = msg_send![&*window, setOpaque: false];

        // [window setBackgroundColor:[NSColor.whiteColor colorWithAlphaComponent:0.001]]
        // Same trick Ghostty uses — alpha 0.001 keeps the window's hit-test
        // working while letting CGS punch the blur through.
        let ns_color = objc2::class!(NSColor);
        let white: *mut AnyObject = msg_send![ns_color, whiteColor];
        let clear: *mut AnyObject = msg_send![&*white, colorWithAlphaComponent: 0.001_f64];
        let _: () = msg_send![&*window, setBackgroundColor: &*clear];

        // CGS blur via the private connection
        let window_number: i64 = msg_send![&*window, windowNumber];
        let conn = CGSDefaultConnectionForThread();
        CGSSetWindowBackgroundBlurRadius(conn, window_number as u32, blur_radius);

        // Recompute shadow now that the window is non-opaque
        let _: () = msg_send![&*window, invalidateShadow];
    }
}

#[cfg(target_os = "macos")]
pub use imp::apply;

#[cfg(not(target_os = "macos"))]
// SAFETY: does nothing; it is unsafe only to match the macOS signature.
#[allow(dead_code)]
pub unsafe fn apply(_ns_window: *mut std::ffi::c_void, _blur_radius: i32) {}

/// Tauri command — re-apply transparency + blur at runtime from the
/// settings panel. Takes a clamped radius (0–80).
#[tauri::command]
pub fn set_window_blur(window: tauri::Window, radius: i32) -> crate::error::AppResult<()> {
    // No caps — pass through whatever the user typed. CGS clamps internally
    // anyway, and negative values just disable blur.
    #[cfg(target_os = "macos")]
    {
        let handle = window
            .ns_window()
            .map_err(|e| crate::error::AppError::Window(e.to_string()))?;
        // SAFETY: `ns_window()` is this window's live NSWindow, and a synchronous Tauri
        // command runs on the main thread.
        unsafe {
            apply(handle, radius);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, radius);
    }
    Ok(())
}
