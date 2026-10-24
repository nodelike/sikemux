//! How see-through the window is, as the person set it. Below full opacity the window
//! lets the desktop through, blurred by the radius they chose, and the shell's ground
//! takes the theme's colour at that opacity. The rails clear outright and panes paint
//! nothing already, so what is read sits on the ground; tab pills keep a fill that only
//! thins with the window, so they stay legible.

use std::cell::Cell;

use gpui::{App, Global, Hsla, Window, WindowBackgroundAppearance};
use sikemux_wire::protocol::Preferences;

#[derive(Clone, Copy, PartialEq)]
pub struct SeeThrough {
    opacity: f32,
    blur: i32,
}

impl Global for SeeThrough {}

thread_local! {
    /// What the window was last given, so it changes only when the preferences do.
    static SHOWN: Cell<Option<SeeThrough>> = const { Cell::new(None) };
}

/// Takes the window's opacity and blur from the preferences, redrawing when they change.
pub fn set(preferences: &Preferences, cx: &mut App) {
    let next = SeeThrough {
        opacity: preferences.window_opacity.clamp(0., 1.) as f32,
        blur: preferences.window_blur.round() as i32,
    };
    if cx.try_global::<SeeThrough>() != Some(&next) {
        cx.set_global(next);
        cx.refresh_windows();
    }
}

fn opacity(cx: &App) -> f32 {
    cx.try_global::<SeeThrough>()
        .map_or(1., |shown| shown.opacity)
}

/// Whether the window lets the desktop through, which clears the rails.
pub fn clear(cx: &App) -> bool {
    opacity(cx) < 1.
}

/// The shell's ground: the theme's colour at the window's opacity.
pub fn ground(color: Hsla, cx: &App) -> Hsla {
    Hsla {
        a: color.a * opacity(cx),
        ..color
    }
}

/// A tab pill's fill, most of the way solid however clear the window is.
pub fn pill(color: Hsla, cx: &App) -> Hsla {
    match clear(cx) {
        true => Hsla {
            a: color.a * (0.56 + opacity(cx) * 0.38),
            ..color
        },
        false => color,
    }
}

/// Gives the window the look the preferences ask for, when they have changed since.
pub fn follow(window: &mut Window, cx: &App) {
    let Some(&wanted) = cx.try_global::<SeeThrough>() else {
        return;
    };
    if SHOWN.get() == Some(wanted) {
        return;
    }
    SHOWN.set(Some(wanted));
    let clear = wanted.opacity < 1.;
    window.set_background_appearance(match clear {
        true => WindowBackgroundAppearance::Transparent,
        false => WindowBackgroundAppearance::Opaque,
    });
    let radius = if clear { wanted.blur } else { 0 };
    blur(window, radius);
    #[cfg(feature = "drive")]
    crate::drive::trace(format_args!(
        "see-through opacity {} blur {radius}",
        wanted.opacity
    ));
}

/// Blurs the desktop behind the window by `radius`, a plain gaussian as Terminal and
/// Ghostty use rather than AppKit's frosted vibrancy, which GPUI's own blur is.
#[cfg(target_os = "macos")]
fn blur(window: &Window, radius: i32) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGSDefaultConnectionForThread() -> *mut std::ffi::c_void;
        fn CGSSetWindowBackgroundBlurRadius(
            connection: *mut std::ffi::c_void,
            window_number: u32,
            radius: i32,
        ) -> i32;
    }

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: the handle is this window's live content view on the main thread, and the
    // window it answers is live while the view is; the radius call takes plain values.
    unsafe {
        let view = handle.ns_view.as_ptr().cast::<AnyObject>();
        let ns_window: *mut AnyObject = msg_send![&*view, window];
        if ns_window.is_null() {
            return;
        }
        let number: isize = msg_send![&*ns_window, windowNumber];
        CGSSetWindowBackgroundBlurRadius(CGSDefaultConnectionForThread(), number as u32, radius);
    }
}

#[cfg(not(target_os = "macos"))]
fn blur(_: &Window, _: i32) {}
