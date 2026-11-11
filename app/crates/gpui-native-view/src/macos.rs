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
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSView, NSWindow, NSWindowOrderingMode};
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

thread_local! {
    static STAND_IN: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) };
}

/// Gives windows drawn offscreen, which have no view of their own, `view` to hold their native
/// views, so a tool that pictures such a window can lay those views over it. A window on screen
/// never uses it.
pub fn stand_in_for_offscreen(view: Retained<NSView>) {
    STAND_IN.with(|stand_in| *stand_in.borrow_mut() = Some(view));
}

/// The view GPUI draws the window into, which native views are added to.
pub fn window_view(window: &Window) -> Option<Retained<NSView>> {
    let own = HasWindowHandle::window_handle(window)
        .ok()
        .and_then(|handle| match handle.as_raw() {
            // SAFETY: GPUI hands out its own live NSView, and this runs on the main thread.
            RawWindowHandle::AppKit(appkit) => unsafe {
                Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>())
            },
            _ => None,
        });
    own.or_else(|| STAND_IN.with(|stand_in| stand_in.borrow().clone()))
}

/// Gives GPUI the keyboard back when the person clicks something GPUI draws, or
/// types while no view they can see holds it. AppKit hands the keyboard to a
/// clicked native view and, once that view is hidden or removed, to the window
/// itself, and never back to GPUI, so every key after that would beep.
pub(crate) fn reclaim_keyboard_on_click() {
    if CLICK_MONITOR.with(|monitor| monitor.borrow().is_some()) {
        return;
    }
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the length of the call.
        reclaim_keyboard(unsafe { event.as_ref() });
        event.as_ptr()
    });
    let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::KeyDown;
    // SAFETY: main thread, as every element's prepaint is. AppKit copies the block, and
    // the monitor it returns is kept in CLICK_MONITOR so it stays installed.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    CLICK_MONITOR.with(|slot| *slot.borrow_mut() = monitor);
}

fn reclaim_keyboard(event: &NSEvent) {
    let Some(window) = MainThreadMarker::new().and_then(|mtm| event.window(mtm)) else {
        return;
    };
    let Some(content) = window.contentView() else {
        return;
    };
    let gpui_view = if event.r#type() == NSEventType::KeyDown {
        if keyboard_is_seen(&window) {
            return;
        }
        gpui_view_in(content)
    } else {
        content
            .hitTest(event.locationInWindow())
            .and_then(drawn_by_gpui)
    };
    let Some(gpui_view) = gpui_view else {
        return;
    };
    let holds_keyboard = window
        .firstResponder()
        .is_some_and(|responder| std::ptr::eq(&*responder, gpui_view.as_ref()));
    if !holds_keyboard {
        window.makeFirstResponder(Some(&gpui_view));
    }
}

/// Whether the window's keyboard is with a view of this window that is on show.
fn keyboard_is_seen(window: &NSWindow) -> bool {
    first_responder_view(window).is_some_and(|view| {
        view.window().as_deref() == Some(window) && !view.isHiddenOrHasHiddenAncestor()
    })
}

fn first_responder_view(window: &NSWindow) -> Option<Retained<NSView>> {
    window.firstResponder()?.downcast::<NSView>().ok()
}

fn gpui_view_in(content: Retained<NSView>) -> Option<Retained<NSView>> {
    let class = AnyClass::get(c"GPUIView")?;
    let inside = content.subviews();
    std::iter::once(content)
        .chain(inside.to_vec())
        .find(|view| view.isKindOfClass(class))
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

/// GPUI lays one cursor rect over its whole view, and WebKit sets no cursor of its own while a
/// cursor rect is in force, so a page's cursors never showed. GPUI's rects now leave out the native
/// views on show, worked out again whenever the pointer crosses between GPUI and one of them.
pub(crate) fn keep_cursor_rects_off_native_views() {
    if POINTER_MONITOR.with(|monitor| monitor.borrow().is_some()) {
        return;
    }
    let (Some(gpui_class), Some(view_class)) =
        (AnyClass::get(c"GPUIView"), AnyClass::get(c"NSView"))
    else {
        return;
    };
    let selector = objc2::sel!(addCursorRect:cursor:);
    let Some(inherited) = view_class.instance_method(selector) else {
        return;
    };
    let _ = ADD_CURSOR_RECT.set(inherited.implementation());
    let replacement: AddCursorRect = add_cursor_rect_around_native_views;
    // SAFETY: `replacement` has `addCursorRect:cursor:`'s signature and types, which are copied from
    // NSView's own method, and it calls NSView's implementation, stored above.
    unsafe {
        let types = objc2::ffi::method_getTypeEncoding(inherited);
        objc2::ffi::class_addMethod(
            (gpui_class as *const AnyClass).cast_mut(),
            selector,
            std::mem::transmute::<AddCursorRect, objc2::runtime::Imp>(replacement),
            types,
        );
    }
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the length of the call.
        pointer_moved(unsafe { event.as_ref() });
        event.as_ptr()
    });
    // SAFETY: main thread, as every element's prepaint is. AppKit copies the block, and the
    // monitor it returns is kept in POINTER_MONITOR so it stays installed.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::MouseMoved, &block)
    };
    POINTER_MONITOR.with(|slot| *slot.borrow_mut() = monitor);
}

type AddCursorRect =
    unsafe extern "C-unwind" fn(&NSView, objc2::runtime::Sel, NSRect, *mut AnyObject);

static ADD_CURSOR_RECT: std::sync::OnceLock<objc2::runtime::Imp> = std::sync::OnceLock::new();

unsafe extern "C-unwind" fn add_cursor_rect_around_native_views(
    view: &NSView,
    selector: objc2::runtime::Sel,
    rect: NSRect,
    cursor: *mut AnyObject,
) {
    let Some(inherited) = ADD_CURSOR_RECT.get() else {
        return;
    };
    // SAFETY: `inherited` is NSView's `addCursorRect:cursor:`, which takes these arguments.
    let add = unsafe { std::mem::transmute::<objc2::runtime::Imp, AddCursorRect>(*inherited) };
    let overlay = AnyClass::get(c"GPUIOverlayView");
    let mut pieces = vec![rect];
    for native in view.subviews().iter() {
        if native.isHidden() || overlay.is_some_and(|class| native.isKindOfClass(class)) {
            continue;
        }
        let hole = native.frame();
        pieces = pieces
            .into_iter()
            .flat_map(|piece| outside(piece, hole))
            .collect();
    }
    for piece in pieces {
        // SAFETY: AppKit's own arguments, with the rect cut smaller.
        unsafe { add(view, selector, piece, cursor) };
    }
}

/// The parts of `rect` that `hole` does not cover.
fn outside(rect: NSRect, hole: NSRect) -> Vec<NSRect> {
    let (left, right) = (rect.origin.x, rect.origin.x + rect.size.width);
    let (low, high) = (rect.origin.y, rect.origin.y + rect.size.height);
    let cut_left = hole.origin.x.max(left);
    let cut_right = (hole.origin.x + hole.size.width).min(right);
    let cut_low = hole.origin.y.max(low);
    let cut_high = (hole.origin.y + hole.size.height).min(high);
    if cut_left >= cut_right || cut_low >= cut_high {
        return vec![rect];
    }
    let part = |x0: f64, y0: f64, x1: f64, y1: f64| {
        (x1 > x0 && y1 > y0)
            .then(|| NSRect::new(NSPoint::new(x0, y0), NSSize::new(x1 - x0, y1 - y0)))
    };
    [
        part(left, low, cut_left, high),
        part(cut_right, low, right, high),
        part(cut_left, low, cut_right, cut_low),
        part(cut_left, cut_high, cut_right, high),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn pointer_moved(event: &NSEvent) {
    let Some(window) = MainThreadMarker::new().and_then(|mtm| event.window(mtm)) else {
        return;
    };
    let Some(content) = window.contentView() else {
        return;
    };
    let over_native = content
        .hitTest(event.locationInWindow())
        .is_some_and(|view| drawn_by_gpui(view).is_none());
    if OVER_NATIVE.with(|over| over.replace(over_native)) == over_native {
        return;
    }
    if let Some(gpui_view) = gpui_view_in(content) {
        window.invalidateCursorRectsForView(&gpui_view);
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
    static POINTER_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static OVER_NATIVE: Cell<bool> = const { Cell::new(false) };
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
