use std::cell::{Cell, RefCell};
use std::ffi::{c_void, CStr};
use std::ptr::NonNull;
use std::rc::{self, Rc};

use block2::RcBlock;
use futures::channel::mpsc;
use futures::StreamExt;
use gpui::{App, Bounds, FocusHandle, Pixels, Subscription, WeakFocusHandle, Window};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{define_class, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSEventMask, NSView, NSWindow, NSWindowOrderingMode};
use objc2_foundation::{
    ns_string, NSDictionary, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSObject,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::{Frame, NativeSurface, Shared, Surface};

/// A view of your own, such as a PDF view, added under the window's overlays
/// and cut to the part of the element that shows.
pub struct OwnedView {
    clip: Retained<NSView>,
    view: Retained<NSView>,
}

impl OwnedView {
    /// Adds `view` to the window, out of sight until an element places it.
    pub fn new(view: Retained<NSView>, window: &Window) -> Option<Self> {
        let parent = window_view(window)?;
        let clip = NSView::initWithFrame(MainThreadMarker::from(&*parent).alloc(), NSRect::ZERO);
        clip.setWantsLayer(true);
        clip.layer()?.setMasksToBounds(true);
        clip.setHidden(true);
        clip.addSubview(&view);
        parent.addSubview_positioned_relativeTo(&clip, NSWindowOrderingMode::Below, None);
        Some(Self { clip, view })
    }

    pub fn view(&self) -> &NSView {
        &self.view
    }
}

impl Surface for OwnedView {
    fn place(&self, frame: Option<Frame>) {
        // SAFETY: main thread, and `clip` is a live view.
        let parent = unsafe { self.clip.superview() };
        let (Some(frame), Some(parent)) = (frame, parent) else {
            self.clip.setHidden(true);
            return;
        };
        self.clip.setFrame(rect_in(&parent, frame.visible));
        let left = f64::from(frame.bounds.left() - frame.visible.left());
        let top = f64::from(frame.bounds.top() - frame.visible.top());
        let (width, height) = size_of(frame.bounds);
        let visible_height = size_of(frame.visible).1;
        let y = if self.clip.isFlipped() {
            top
        } else {
            visible_height - top - height
        };
        self.view.setFrame(NSRect::new(
            NSPoint::new(left, y),
            NSSize::new(width, height),
        ));
        self.clip.setHidden(false);
    }

    fn responder(&self) -> Option<Retained<NSView>> {
        Some(self.view.clone())
    }
}

impl Drop for OwnedView {
    fn drop(&mut self) {
        self.clip.removeFromSuperview();
    }
}

fn size_of(bounds: Bounds<Pixels>) -> (f64, f64) {
    (f64::from(bounds.size.width), f64::from(bounds.size.height))
}

/// `bounds`, measured from the window's top left, in `parent`'s own coordinates.
fn rect_in(parent: &NSView, bounds: Bounds<Pixels>) -> NSRect {
    let (width, height) = size_of(bounds);
    let top = f64::from(bounds.top());
    let y = if parent.isFlipped() {
        top
    } else {
        parent.frame().size.height - top - height
    };
    NSRect::new(
        NSPoint::new(f64::from(bounds.left()), y),
        NSSize::new(width, height),
    )
}

/// The view GPUI draws the window into, which native views are added to.
pub fn window_view(window: &Window) -> Option<Retained<NSView>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    // SAFETY: GPUI hands out its own live NSView, and this runs on the main thread.
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
}

/// Gives GPUI the keyboard back whenever the person clicks something GPUI draws.
/// AppKit hands it to a clicked native view, but never back on a click elsewhere,
/// so after clicking into a page every key would go to the page.
pub(crate) fn reclaim_keyboard_on_click() {
    if CLICK_MONITOR.with(|monitor| monitor.borrow().is_some()) {
        return;
    }
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the length of the call.
        reclaim_keyboard(unsafe { event.as_ref() });
        event.as_ptr()
    });
    let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown;
    // SAFETY: main thread, as every element's prepaint is. AppKit copies the block, and
    // the monitor it returns is kept in CLICK_MONITOR so it stays installed.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    CLICK_MONITOR.with(|slot| *slot.borrow_mut() = monitor);
}

fn reclaim_keyboard(event: &NSEvent) {
    let Some(window) = MainThreadMarker::new().and_then(|mtm| event.window(mtm)) else {
        return;
    };
    let Some(clicked) = window
        .contentView()
        .and_then(|content| content.hitTest(event.locationInWindow()))
    else {
        return;
    };
    let Some(gpui_view) = drawn_by_gpui(clicked) else {
        return;
    };
    let holds_keyboard = window
        .firstResponder()
        .is_some_and(|responder| std::ptr::eq(&*responder, gpui_view.as_ref()));
    if !holds_keyboard {
        window.makeFirstResponder(Some(&gpui_view));
    }
}

/// The GPUI view `view` belongs to, when it is that view or the overlay GPUI
/// draws inside it rather than a native view.
fn drawn_by_gpui(view: Retained<NSView>) -> Option<Retained<NSView>> {
    let is = |name: &CStr| AnyClass::get(name).is_some_and(|class| view.isKindOfClass(class));
    if is(c"GPUIView") {
        Some(view)
    } else if is(c"GPUIOverlayView") {
        // SAFETY: main thread, and `view` is a live view.
        unsafe { view.superview() }
    } else {
        None
    }
}

/// Keeps a surface's native keyboard focus and its element's focus handle together.
pub(crate) struct Keyboard {
    mark: u64,
    surface: NativeSurface,
    gpui_view: Retained<NSView>,
    _focus_changes: [Subscription; 2],
}

struct Follower {
    mark: u64,
    surface: rc::Weak<Shared>,
    focus: WeakFocusHandle,
}

thread_local! {
    static FOLLOWERS: RefCell<Vec<Follower>> = const { RefCell::new(Vec::new()) };
    static WATCHED: RefCell<Vec<(Weak<NSWindow>, Retained<FirstResponderObserver>)>> =
        const { RefCell::new(Vec::new()) };
    static MARKS: Cell<u64> = const { Cell::new(0) };
    static CLICK_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

impl Keyboard {
    pub(crate) fn new(
        surface: &NativeSurface,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Self> {
        let gpui_view = window_view(window)?;
        watch_first_responder(&gpui_view, window, cx);
        let mark = MARKS.with(|marks| {
            marks.set(marks.get() + 1);
            marks.get()
        });
        FOLLOWERS.with(|followers| {
            followers.borrow_mut().push(Follower {
                mark,
                surface: Rc::downgrade(&surface.0),
                focus: focus.downgrade(),
            })
        });
        let focus_in = {
            let surface = surface.clone();
            window.on_focus_in(focus, cx, move |_, _| give_keyboard(&surface))
        };
        let focus_out = {
            let (surface, gpui_view) = (surface.clone(), gpui_view.clone());
            window.on_focus_out(focus, cx, move |_, _, _| {
                take_keyboard_back(&surface, &gpui_view)
            })
        };
        if focus.is_focused(window) {
            give_keyboard(surface);
        }
        Some(Self {
            mark,
            surface: surface.clone(),
            gpui_view,
            _focus_changes: [focus_in, focus_out],
        })
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        FOLLOWERS.with(|followers| {
            followers
                .borrow_mut()
                .retain(|follower| follower.mark != self.mark)
        });
        take_keyboard_back(&self.surface, &self.gpui_view);
    }
}

fn give_keyboard(surface: &NativeSurface) {
    let Some(view) = surface.surface().responder() else {
        return;
    };
    if holds_keyboard(&view) {
        return;
    }
    if let Some(window) = view.window() {
        window.makeFirstResponder(Some(&view));
    }
}

fn take_keyboard_back(surface: &NativeSurface, gpui_view: &NSView) {
    let Some(view) = surface.surface().responder() else {
        return;
    };
    if !holds_keyboard(&view) {
        return;
    }
    if let Some(window) = gpui_view.window() {
        window.makeFirstResponder(Some(gpui_view));
    }
}

/// Whether `view` or a view inside it takes the window's key presses.
fn holds_keyboard(view: &NSView) -> bool {
    first_responder(view).is_some_and(|responder| responder.isDescendantOf(view))
}

fn first_responder(view: &NSView) -> Option<Retained<NSView>> {
    view.window()?.firstResponder()?.downcast::<NSView>().ok()
}

/// Hears the window's keyboard move to a native view, which GPUI never sees, and
/// focuses the element that shows it.
fn watch_first_responder(gpui_view: &NSView, window: &mut Window, cx: &mut App) {
    let Some(ns_window) = gpui_view.window() else {
        return;
    };
    let watched = WATCHED.with(|watched| {
        let mut watched = watched.borrow_mut();
        watched.retain(|(window, _)| window.load().is_some());
        watched
            .iter()
            .any(|(window, _)| window.load().as_deref() == Some(&*ns_window))
    });
    if watched {
        return;
    }
    let (changed, mut changes) = mpsc::unbounded();
    let observer = FirstResponderObserver::new(MainThreadMarker::from(gpui_view), changed);
    // SAFETY: `observer` stays alive in `WATCHED` for as long as the window does, and
    // NSWindow's first responder is observable.
    unsafe {
        ns_window.addObserver_forKeyPath_options_context(
            &observer,
            ns_string!("firstResponder"),
            NSKeyValueObservingOptions::empty(),
            std::ptr::null_mut(),
        )
    };
    WATCHED.with(|watched| {
        watched
            .borrow_mut()
            .push((Weak::from_retained(&ns_window), observer))
    });
    window
        .spawn(cx, async move |cx| {
            while changes.next().await.is_some() {
                if cx.update(follow_native_focus).is_err() {
                    break;
                }
            }
        })
        .detach();
}

fn follow_native_focus(window: &mut Window, cx: &mut App) {
    let Some(responder) = window_view(window).and_then(|view| first_responder(&view)) else {
        return;
    };
    let focus = FOLLOWERS.with(|followers| {
        followers.borrow().iter().find_map(|follower| {
            let shown = follower.surface.upgrade()?.surface.responder()?;
            responder
                .isDescendantOf(&shown)
                .then(|| follower.focus.upgrade())
                .flatten()
        })
    });
    if let Some(focus) = focus.filter(|focus| !focus.is_focused(window)) {
        focus.focus(window, cx);
    }
}

define_class!(
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = mpsc::UnboundedSender<()>]
    struct FirstResponderObserver;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for FirstResponderObserver {}

    impl FirstResponderObserver {
        // SAFETY: the signature matches KVO's `observeValueForKeyPath:ofObject:change:context:`,
        // which fires on the main thread when the window's first responder changes.
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn first_responder_changed(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            let _ = self.ivars().unbounded_send(());
        }
    }
);

impl FirstResponderObserver {
    fn new(mtm: MainThreadMarker, changed: mpsc::UnboundedSender<()>) -> Retained<Self> {
        let observer = mtm.alloc::<Self>().set_ivars(changed);
        // SAFETY: NSObject's `init` takes nothing and returns the object it was sent to.
        unsafe { objc2::msg_send![super(observer), init] }
    }
}
