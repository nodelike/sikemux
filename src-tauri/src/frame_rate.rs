//! WebKit draws a page at 60 frames a second even on a 120Hz display, unless
//! the web view is made with its "prefer 60fps" switch off. The switch is only
//! read when a web view is made, and Tauri makes them all, so it is turned off
//! in WKWebView's own initializer: the window and every browser tab pass
//! through it.

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::OnceLock;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
    use objc2::{msg_send, sel};
    use objc2_foundation::{NSRect, NSString};

    const FEATURE: &str = "PreferPageRenderingUpdatesNear60FPSEnabled";

    type Init =
        unsafe extern "C-unwind" fn(*mut AnyObject, Sel, NSRect, *mut AnyObject) -> *mut AnyObject;

    static MADE_AT_60: OnceLock<Init> = OnceLock::new();

    pub fn render_at_display_rate() {
        let Some(class) = AnyClass::get(c"WKWebView") else {
            return;
        };
        let Some(method) = class.instance_method(sel!(initWithFrame:configuration:)) else {
            return;
        };
        // SAFETY: `initWithFrame:configuration:` takes a rect and a configuration
        // and returns the view, which is exactly `Init`.
        let inherited = unsafe { std::mem::transmute::<Imp, Init>(method.implementation()) };
        if MADE_AT_60.set(inherited).is_err() {
            return;
        }
        let ours: Init = init_at_display_rate;
        // SAFETY: `ours` has the initializer's exact signature and calls the
        // implementation it replaces.
        unsafe { method.set_implementation(std::mem::transmute::<Init, Imp>(ours)) };
    }

    // SAFETY: only the Objective-C runtime calls this, as WKWebView's initializer,
    // with a fresh view and the configuration it is being made with.
    unsafe extern "C-unwind" fn init_at_display_rate(
        view: *mut AnyObject,
        selector: Sel,
        frame: NSRect,
        configuration: *mut AnyObject,
    ) -> *mut AnyObject {
        // SAFETY: WebKit requires a configuration here, so it is live for the call.
        if let Some(configuration) = unsafe { configuration.as_ref() } {
            prefer_display_rate(configuration);
        }
        let inherited = MADE_AT_60.get().expect("installed before it can be called");
        // SAFETY: the arguments are the runtime's own, passed straight through.
        unsafe { inherited(view, selector, frame, configuration) }
    }

    /// Turns the switch off through WebKit's feature list, which is private and
    /// so is asked about at every step rather than assumed.
    fn prefer_display_rate(configuration: &AnyObject) {
        let Some(preferences_class) = AnyClass::get(c"WKPreferences") else {
            return;
        };
        // SAFETY: a plain `respondsToSelector:` query, sent to the class itself
        // because `_features` is a class method.
        let listed: bool =
            unsafe { msg_send![preferences_class, respondsToSelector: sel!(_features)] };
        if !listed {
            return;
        }
        // SAFETY: every WKWebViewConfiguration answers `preferences` with its WKPreferences.
        let preferences: Option<Retained<AnyObject>> =
            unsafe { msg_send![configuration, preferences] };
        let Some(preferences) = preferences else {
            return;
        };
        // SAFETY: checked just above; it answers with an array of WebKit's features.
        let features: Option<Retained<AnyObject>> =
            unsafe { msg_send![preferences_class, _features] };
        let Some(features) = features else {
            return;
        };
        // SAFETY: `features` is an NSArray.
        let count: usize = unsafe { msg_send![&*features, count] };
        let key_selector = sel!(key);
        for index in 0..count {
            // SAFETY: `index` is within the array.
            let feature: Retained<AnyObject> =
                unsafe { msg_send![&*features, objectAtIndex: index] };
            // SAFETY: a plain `respondsToSelector:` query.
            let keyed: bool = unsafe { msg_send![&*feature, respondsToSelector: key_selector] };
            if !keyed {
                return;
            }
            // SAFETY: confirmed above; it answers with an NSString.
            let key: Option<Retained<NSString>> = unsafe { msg_send![&*feature, key] };
            if key.is_some_and(|key| key.to_string() == FEATURE) {
                let setter = sel!(_setEnabled:forFeature:);
                // SAFETY: a plain `respondsToSelector:` query.
                let settable: bool =
                    unsafe { msg_send![&*preferences, respondsToSelector: setter] };
                if settable {
                    // SAFETY: confirmed above; it takes a BOOL and a feature and returns nothing.
                    let _: () = unsafe {
                        msg_send![&*preferences, _setEnabled: Bool::NO, forFeature: &*feature]
                    };
                }
                return;
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::render_at_display_rate;

#[cfg(not(target_os = "macos"))]
pub fn render_at_display_rate() {}
