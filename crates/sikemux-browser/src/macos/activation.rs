//! Keeping a tab being made, or a page, from bringing the app to the front.

use super::*;

pub(super) static TABS_BEING_MADE: AtomicUsize = AtomicUsize::new(0);
pub(super) static ORIGINAL_ACTIVATE: OnceLock<Imp> = OnceLock::new();
pub(super) static ORIGINAL_ACTIVATE_IGNORING: OnceLock<Imp> = OnceLock::new();

pub(super) type Activate = unsafe extern "C-unwind" fn(*mut AnyObject, Sel);
pub(super) type ActivateIgnoring = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Bool);

/// Runs `make` without letting it bring Sikemux to the front. Making a
/// webview activates the whole app, which would take the keyboard from
/// whatever other app the person is typing in.
pub fn without_activating<T>(make: impl FnOnce() -> T) -> T {
    static GUARD: Once = Once::new();
    GUARD.call_once(guard_activation);
    TABS_BEING_MADE.fetch_add(1, Ordering::AcqRel);
    let made = make();
    TABS_BEING_MADE.fetch_sub(1, Ordering::AcqRel);
    made
}

// SAFETY: only the Objective-C runtime calls this, as NSApplication's `activate`.
unsafe extern "C-unwind" fn activate(app: *mut AnyObject, selector: Sel) {
    if TABS_BEING_MADE.load(Ordering::Acquire) > 0 {
        return;
    }
    if let Some(original) = ORIGINAL_ACTIVATE.get() {
        // SAFETY: `original` is the implementation this function replaced, so it
        // takes the same arguments.
        unsafe { std::mem::transmute::<Imp, Activate>(*original)(app, selector) };
    }
}

// SAFETY: only the Objective-C runtime calls this, as NSApplication's
// `activateIgnoringOtherApps:`.
unsafe extern "C-unwind" fn activate_ignoring(app: *mut AnyObject, selector: Sel, ignoring: Bool) {
    if TABS_BEING_MADE.load(Ordering::Acquire) > 0 {
        return;
    }
    if let Some(original) = ORIGINAL_ACTIVATE_IGNORING.get() {
        // SAFETY: `original` is the implementation this function replaced, so it
        // takes the same arguments.
        unsafe { std::mem::transmute::<Imp, ActivateIgnoring>(*original)(app, selector, ignoring) };
    }
}

pub(super) fn guard_activation() {
    let Some(class) = AnyClass::get(c"NSApplication") else {
        return;
    };
    if let Some(method) = class.instance_method(sel!(activate)) {
        let replacement: Activate = activate;
        // SAFETY: `activate` has the signature of `-[NSApplication activate]` and
        // calls the implementation it replaces, which is stored before anything
        // can call it.
        let original =
            unsafe { method.set_implementation(std::mem::transmute::<Activate, Imp>(replacement)) };
        let _ = ORIGINAL_ACTIVATE.set(original);
    }
    if let Some(method) = class.instance_method(sel!(activateIgnoringOtherApps:)) {
        let replacement: ActivateIgnoring = activate_ignoring;
        // SAFETY: `activate_ignoring` has the signature of `-[NSApplication
        // activateIgnoringOtherApps:]` and calls the implementation it replaces.
        let original = unsafe {
            method.set_implementation(std::mem::transmute::<ActivateIgnoring, Imp>(replacement))
        };
        let _ = ORIGINAL_ACTIVATE_IGNORING.set(original);
    }
}

/// The person's own pointer and modifier keys, which AppKit delivers through
/// the app, unlike the agent's events sent straight to a tab.
pub(super) fn watch_the_person() {
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the length of the call.
        crate::input::note_person(unsafe { event.as_ref() });
        event.as_ptr()
    });
    let mask = NSEventMask::MouseMoved
        | NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::RightMouseDown
        | NSEventMask::OtherMouseDown
        | NSEventMask::ScrollWheel
        | NSEventMask::FlagsChanged;
    // SAFETY: main thread, as `install_shortcuts` is. AppKit copies the block, and the
    // monitor it returns is kept in PERSON_MONITOR so it stays installed.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    PERSON_MONITOR.with(|slot| *slot.borrow_mut() = monitor);
    crate::input::guard_cursor();
    crate::input::guard_keyboard();
}
