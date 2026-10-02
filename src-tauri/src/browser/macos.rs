//! The parts of a browser tab only AppKit can do: page dialogs as window
//! sheets, history without a script round trip, the address of a page that
//! moved without loading anything, and app shortcuts pressed while the page
//! owns the keyboard. Everything here runs on the main thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Imp, ProtocolObject, Sel};
use objc2::{
    define_class, msg_send, sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAutoresizingMaskOptions,
    NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSEventMask, NSEventModifierFlags, NSImage,
    NSImageCompressionFactor, NSModalResponse, NSTextField, NSView,
};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSNumber,
    NSObject, NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString,
};
use objc2_quartz_core::{kCAFillRuleEvenOdd, CALayer, CAShapeLayer, CATransaction};
use objc2_web_kit::{
    WKContentWorld, WKFrameInfo, WKMediaCaptureType, WKNavigation, WKNavigationAction,
    WKNavigationDelegate, WKNavigationResponse, WKNavigationResponsePolicy, WKOpenPanelParameters,
    WKPDFConfiguration, WKPermissionDecision, WKSecurityOrigin, WKSnapshotConfiguration,
    WKUIDelegate, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};
use tauri::{AppHandle, Emitter};

use super::burst::Burst;
use super::documents::DocumentEvent;
use super::{BrowserShortcut, PageDialog, TabStall, BROWSER_SHORTCUT_EVENT};

/// The property the tab watches to hear about a page that moved on its own.
const URL_KEY_PATH: &str = "URL";
/// WebKit's own verdict on whether the page's process still answers input.
const RESPONSIVE_KEY_PATH: &str = "_webProcessIsResponsive";
/// Past this many dialogs in `DIALOG_WINDOW`, a page's dialogs are answered
/// with Cancel unseen, the way browsers offer to stop a page's dialogs.
const DIALOG_LIMIT: usize = 3;
const DIALOG_WINDOW: Duration = Duration::from_secs(10);

struct NativeTab {
    agent_id: String,
    webview: Retained<WKWebView>,
    _delegate: Retained<TabUiDelegate>,
    _navigation: Retained<TabNavigationDelegate>,
    address_observer: Retained<AddressObserver>,
    watches_responsiveness: bool,
}

/* AppKit throws if a view is freed while anything is still watching it, so the
tab lets go of what it watches before it lets go of either of them. */
impl Drop for NativeTab {
    fn drop(&mut self) {
        let watched = [URL_KEY_PATH, RESPONSIVE_KEY_PATH];
        let count = if self.watches_responsiveness { 2 } else { 1 };
        for key_path in &watched[..count] {
            // SAFETY: `adopt` registered this observer for this key path, and the tab
            // still retains both the observer and the webview. Tabs live only on the
            // main thread.
            unsafe {
                self.webview.removeObserver_forKeyPath(
                    &self.address_observer,
                    &NSString::from_str(key_path),
                );
            }
        }
    }
}

thread_local! {
    static TABS: RefCell<HashMap<String, NativeTab>> = RefCell::new(HashMap::new());
    static SHORTCUT_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static PERSON_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static OPEN_DIALOGS: RefCell<HashMap<String, OpenDialog>> = RefCell::new(HashMap::new());
    static HOLES: RefCell<HashMap<usize, Vec<NSRect>>> = RefCell::new(HashMap::new());
    static SHADES: RefCell<HashMap<usize, Retained<CALayer>>> = RefCell::new(HashMap::new());
    static PAGE_HIT_TEST: std::cell::Cell<Option<Imp>> = const { std::cell::Cell::new(None) };
    /// WebKit tears a named world down once nothing holds it, and the element
    /// numbers the agent was given go with it.
    static HELPER_WORLD: RefCell<Option<Retained<WKContentWorld>>> = const { RefCell::new(None) };
}

/// A page dialog showing as a sheet, kept so the agent can answer it too.
struct OpenDialog {
    alert: Retained<NSAlert>,
    field: Option<Retained<NSTextField>>,
}

fn webview_from(pointer: *mut c_void) -> Option<Retained<WKWebView>> {
    // SAFETY: every caller passes `platform.inner()` from inside Tauri's `with_webview`,
    // which runs on the main thread while that WKWebView is alive.
    unsafe { Retained::retain(pointer.cast::<WKWebView>()) }
}

/// Take over the tab's UI delegate so page dialogs get a sheet, watch where the
/// page says it is, and remember the view so shortcuts can tell which tab has
/// focus. `moved` hears the new address and whether history can go either way;
/// `dialog` hears a page dialog open and close; `upload` hands over files the
/// agent picked for the next file chooser, which then never shows; `document`
/// hears each top-level load start, get its answer, and finish or fail;
/// `health` hears the page's process stop answering, crash, and recover.
#[allow(clippy::too_many_arguments)]
pub fn adopt(
    pointer: *mut c_void,
    agent_id: String,
    tab_id: String,
    moved: impl Fn(String, bool, bool) + 'static,
    dialog: impl Fn(Option<PageDialog>) + 'static,
    upload: impl Fn() -> Option<Vec<std::path::PathBuf>> + 'static,
    document: impl Fn(DocumentEvent) + 'static,
    health: impl Fn(Option<TabStall>) + 'static,
) {
    let (Some(webview), Some(mtm)) = (webview_from(pointer), MainThreadMarker::new()) else {
        return;
    };
    // SAFETY: main thread (checked just above), and `webview` is retained.
    let inner = unsafe { webview.UIDelegate() };
    let delegate = TabUiDelegate::new(
        mtm,
        inner,
        tab_id.clone(),
        Rc::new(dialog),
        Box::new(upload),
    );
    let health: Rc<dyn Fn(Option<TabStall>)> = Rc::new(health);
    let navigation = TabNavigationDelegate::new(
        mtm,
        // SAFETY: main thread, and `webview` is retained.
        unsafe { webview.navigationDelegate() },
        Box::new(document),
        health.clone(),
    );
    let address_observer = AddressObserver::new(mtm, Box::new(moved), health);
    let watches_responsiveness = webview.respondsToSelector(sel!(_webProcessIsResponsive));
    // SAFETY: main thread. WebKit holds delegates and observers weakly, so the tab keeps
    // them alive in TABS, and `Drop` removes the observer before letting go.
    unsafe {
        webview.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        webview.setNavigationDelegate(Some(ProtocolObject::from_ref(&*navigation)));
        webview.setAllowsBackForwardNavigationGestures(true);
        webview.setAllowsMagnification(true);
        webview.addObserver_forKeyPath_options_context(
            &address_observer,
            &NSString::from_str(URL_KEY_PATH),
            NSKeyValueObservingOptions::empty(),
            std::ptr::null_mut(),
        );
        if watches_responsiveness {
            webview.addObserver_forKeyPath_options_context(
                &address_observer,
                &NSString::from_str(RESPONSIVE_KEY_PATH),
                NSKeyValueObservingOptions::empty(),
                std::ptr::null_mut(),
            );
        }
    }
    TABS.with(|tabs| {
        tabs.borrow_mut().insert(
            tab_id,
            NativeTab {
                agent_id,
                webview,
                _delegate: delegate,
                _navigation: navigation,
                address_observer,
                watches_responsiveness,
            },
        );
    });
}

/// WebKit stops a page's animation frames while another app covers the
/// window. A tab that is loading or being driven must keep them running.
pub fn keep_running_when_covered(pointer: *mut c_void, keep_running: bool) {
    let Some(webview) = webview_from(pointer) else {
        return;
    };
    let selector = sel!(_setWindowOcclusionDetectionEnabled:);
    if webview.respondsToSelector(selector) {
        let enabled = Bool::new(!keep_running);
        // SAFETY: `respondsToSelector` just confirmed this private method exists; it takes
        // one BOOL and returns nothing.
        let _: () = unsafe { msg_send![&*webview, _setWindowOcclusionDetectionEnabled: enabled] };
    }
}

pub fn forget(tab_id: &str) {
    let _ = answer_dialog(tab_id, false, None);
    TABS.with(|tabs| {
        if let Some(tab) = tabs.borrow_mut().remove(tab_id) {
            HOLES.with(|holes| holes.borrow_mut().remove(&view_key(&tab.webview)));
            SHADES.with(|shades| shades.borrow_mut().remove(&view_key(&tab.webview)));
        }
    });
}

struct AddressObserverIvars {
    moved: Box<dyn Fn(String, bool, bool)>,
    health: Rc<dyn Fn(Option<TabStall>)>,
}

define_class!(
    /// What tells the app that a page changed its address without loading a new
    /// document — a web app routing between its own screens, or a jump to an
    /// anchor. The navigation hooks never hear about either one. It also hears
    /// WebKit decide that the page's process stopped answering, or answers again.
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AddressObserverIvars]
    struct AddressObserver;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for AddressObserver {}

    impl AddressObserver {
        // SAFETY: the signature matches KVO's `observeValueForKeyPath:ofObject:change:context:`,
        // which fires on the main thread for the webview's address.
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        unsafe fn address_changed(
            &self,
            key_path: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            let Some(webview) = object.and_then(|object| object.downcast_ref::<WKWebView>()) else {
                return;
            };
            if key_path.is_some_and(|path| path.to_string() == RESPONSIVE_KEY_PATH) {
                // SAFETY: `adopt` only watches this key path on a webview that answers
                // `_webProcessIsResponsive`, which takes nothing and returns a BOOL.
                let responsive: bool = unsafe { msg_send![webview, _webProcessIsResponsive] };
                (self.ivars().health)((!responsive).then_some(TabStall::Unresponsive));
                return;
            }
            /* A page that is fetching a document reports that address itself
               when the load commits, and may yet be sent somewhere else or fail
               outright. Leaving those to the navigation hook keeps the bar from
               ever showing a page that never arrived. */
            // SAFETY: `webview` is the observed view, alive for this call on the main thread.
            if unsafe { webview.isLoading() } {
                return;
            }
            // SAFETY: same live webview, same thread.
            let Some(address) = (unsafe { webview.URL() }) else {
                return;
            };
            let Some(address) = address.absoluteString() else {
                return;
            };
            // SAFETY: same live webview, same thread.
            (self.ivars().moved)(address.to_string(), unsafe { webview.canGoBack() }, unsafe {
                webview.canGoForward()
            });
        }
    }
);

impl AddressObserver {
    fn new(
        mtm: MainThreadMarker,
        moved: Box<dyn Fn(String, bool, bool)>,
        health: Rc<dyn Fn(Option<TabStall>)>,
    ) -> Retained<Self> {
        let observer = mtm
            .alloc::<AddressObserver>()
            .set_ivars(AddressObserverIvars { moved, health });
        // SAFETY: `observer` is freshly allocated with its ivars set, and NSObject's `init`
        // takes nothing and returns that same object.
        unsafe { msg_send![super(observer), init] }
    }
}

pub fn history(pointer: *mut c_void, delta: i32) {
    let Some(webview) = webview_from(pointer) else {
        return;
    };
    // SAFETY: main thread (see `webview_from`), and `webview` is retained.
    unsafe {
        if delta < 0 {
            webview.goBack();
        } else {
            webview.goForward();
        }
    }
}

/// Send `agent` as the tab's agent string, reloading so the site sees the change.
pub fn introduce_as(pointer: *mut c_void, agent: &str) {
    let Some(webview) = webview_from(pointer) else {
        return;
    };
    // SAFETY: main thread (see `webview_from`), and `webview` is retained.
    unsafe {
        if webview
            .customUserAgent()
            .is_some_and(|current| current.to_string() == agent)
        {
            return;
        }
        webview.setCustomUserAgent(Some(&NSString::from_str(agent)));
        let _ = webview.reload();
    }
}

/// Puts the tab's view at `frame`, given top-down in the window's content
/// view. With `page`, the page lays out at that size and the view draws it
/// scaled into `frame`: its bounds take the page's size, so AppKit scales the
/// drawing and maps every event point back into page pixels on its own.
pub fn place(pointer: *mut c_void, frame: NSRect, page: Option<NSSize>) {
    let Some(webview) = webview_from(pointer) else {
        return;
    };
    let view: &NSView = &webview;
    // SAFETY: main thread (see `webview_from`), and `webview` is retained.
    let Some(parent) = (unsafe { view.superview() }) else {
        return;
    };
    let origin = if parent.isFlipped() {
        frame.origin
    } else {
        NSPoint::new(
            frame.origin.x,
            parent.frame().size.height - frame.origin.y - frame.size.height,
        )
    };
    let page_size = page.unwrap_or(frame.size);
    if view.frame().size != frame.size || view.bounds().size != page_size {
        // WebKit lays the page out at whatever size its own `setFrameSize:` is
        // given, so it hears the page's size, and the view then takes the
        // smaller frame through NSView's method, which WebKit never sees.
        view.setFrameSize(page_size);
        if page.is_some() {
            // SAFETY: `view` is a live NSView on the main thread, and NSView's
            // `setFrameSize:` takes one NSSize and returns nothing.
            let _: () =
                unsafe { msg_send![super(view, NSView::class()), setFrameSize: frame.size] };
        }
        view.setBoundsSize(page_size);
        // WebKit's own content view fills the view by resizing with it, and a
        // change of bounds alone resizes nothing.
        let whole = view.bounds();
        let fills = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;
        for child in view.subviews() {
            if child.autoresizingMask().contains(fills) {
                child.setFrame(whole);
            }
        }
    }
    view.setFrameOrigin(origin);
}

/// Draw the page less `clip_left` and `clip_right` off its sides and less the
/// `holes`, all in the page's own top-down pixels. A native view is not cut
/// off by the DOM around it, so a swipe would carry the page over the rails
/// and it would cover any toast.
pub fn clip(pointer: *mut c_void, clip_left: f64, clip_right: f64, holes: Vec<(NSRect, f64)>) {
    let Some(webview) = webview_from(pointer) else {
        return;
    };
    let view: &NSView = &webview;
    // SAFETY: every NSView answers `layer` with a CALayer or nil, and the result is retained.
    let Some(layer): Option<Retained<CALayer>> = (unsafe { msg_send![view, layer] }) else {
        return;
    };
    let whole = view.bounds();
    let visible = NSRect::new(
        NSPoint::new(whole.origin.x + clip_left, whole.origin.y),
        NSSize::new(
            (whole.size.width - clip_left - clip_right).max(0.0),
            whole.size.height,
        ),
    );
    let holes: Vec<(NSRect, f64)> = holes
        .into_iter()
        .filter_map(|(hole, radius)| {
            let hole = intersection(hole, visible)?;
            Some((
                hole,
                radius
                    .min(hole.size.width / 2.0)
                    .min(hole.size.height / 2.0),
            ))
        })
        .collect();
    HOLES.with(|all| {
        let mut all = all.borrow_mut();
        if holes.is_empty() {
            all.remove(&view_key(&webview));
        } else {
            all.insert(
                view_key(&webview),
                holes.iter().map(|(hole, _)| *hole).collect(),
            );
        }
    });
    if !holes.is_empty() {
        let _ = pass_clicks_through_holes(view);
    }
    let flip = |rect: NSRect| {
        if layer.isGeometryFlipped() {
            rect
        } else {
            NSRect::new(
                NSPoint::new(
                    rect.origin.x,
                    whole.size.height - rect.origin.y - rect.size.height,
                ),
                rect.size,
            )
        }
    };
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    if visible == whole && holes.is_empty() {
        // SAFETY: main thread, and `layer` is retained for the whole function.
        unsafe { layer.setMask(None) };
    } else {
        let path = CGMutablePath::new();
        // SAFETY: a null transform means none, and `path` is a fresh path only we hold.
        unsafe {
            CGMutablePath::add_rect(Some(&path), std::ptr::null(), flip(visible));
            for (hole, radius) in &holes {
                CGMutablePath::add_rounded_rect(
                    Some(&path),
                    std::ptr::null(),
                    flip(*hole),
                    *radius,
                    *radius,
                );
            }
        }
        let mask = CAShapeLayer::new();
        mask.setFrame(layer.bounds());
        // SAFETY: an immutable constant string QuartzCore sets up when it loads.
        mask.setFillRule(unsafe { kCAFillRuleEvenOdd });
        mask.setPath(Some(&path));
        // SAFETY: main thread, and the layer retains `mask` from here on.
        unsafe { layer.setMask(Some(&mask)) };
    }
    CATransaction::commit();
}

/// Lay a black shade of `alpha` over the page, or take it off at zero, so an
/// app panel floating on the page stands apart from it. The shade lives in the
/// page's own layer, so the mask `clip` sets cuts it where the panel is.
pub fn dim(pointer: *mut c_void, alpha: f64) {
    let Some(webview) = webview_from(pointer) else {
        return;
    };
    let view: &NSView = &webview;
    // SAFETY: every NSView answers `layer` with a CALayer or nil, and the result is retained.
    let Some(layer): Option<Retained<CALayer>> = (unsafe { msg_send![view, layer] }) else {
        return;
    };
    let key = view_key(&webview);
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    SHADES.with(|shades| {
        let mut shades = shades.borrow_mut();
        if alpha <= 0.0 {
            if let Some(shade) = shades.remove(&key) {
                shade.removeFromSuperlayer();
            }
            return;
        }
        let shade = shades.entry(key).or_insert_with(|| {
            let shade = CALayer::new();
            // Above whatever layers WebKit draws the page into.
            shade.setZPosition(1.0e6);
            layer.addSublayer(&shade);
            shade
        });
        shade.setFrame(layer.bounds());
        shade.setBackgroundColor(Some(&CGColor::new_generic_gray(0.0, alpha)));
    });
    CATransaction::commit();
}

fn view_key(view: &NSView) -> usize {
    view as *const NSView as usize
}

fn intersection(a: NSRect, b: NSRect) -> Option<NSRect> {
    let left = a.origin.x.max(b.origin.x);
    let top = a.origin.y.max(b.origin.y);
    let right = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let bottom = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    (right > left && bottom > top).then(|| {
        NSRect::new(
            NSPoint::new(left, top),
            NSSize::new(right - left, bottom - top),
        )
    })
}

/* A mask only changes what the page draws; clicks over a hole still land on the
page. So the page's class learns to pass a point in a hole on to the app below. */
fn pass_clicks_through_holes(view: &NSView) -> Option<()> {
    if PAGE_HIT_TEST.with(|cell| cell.get()).is_some() {
        return Some(());
    }
    let class = view.class();
    let selector = sel!(hitTest:);
    let inherited = class.instance_method(selector)?;
    // SAFETY: `inherited` is a real method of the view's class, so its type string lives
    // as long as the class does.
    let types = unsafe { objc2::ffi::method_getTypeEncoding(inherited) };
    let hit_test: HitTest = hit_test_outside_holes;
    // SAFETY: `hit_test_outside_holes` has `hitTest:`'s exact signature and reuses its
    // type string. `class_addMethod` only adds to the class, never replaces a method.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (class as *const objc2::runtime::AnyClass).cast_mut(),
            selector,
            std::mem::transmute::<HitTest, Imp>(hit_test),
            types,
        )
    };
    added
        .as_bool()
        .then(|| PAGE_HIT_TEST.with(|cell| cell.set(Some(inherited.implementation()))))
}

type HitTest = unsafe extern "C-unwind" fn(&NSView, Sel, NSPoint) -> *mut NSView;

// SAFETY: only AppKit calls this, as the `hitTest:` method installed above, with a
// live view on the main thread.
unsafe extern "C-unwind" fn hit_test_outside_holes(
    view: &NSView,
    selector: Sel,
    point: NSPoint,
) -> *mut NSView {
    let holes = HOLES.with(|all| all.borrow().get(&view_key(view)).cloned());
    if let Some(holes) = holes {
        // SAFETY: main thread, and `view` is alive for this call.
        let superview = unsafe { view.superview() };
        let local = view.convertPoint_fromView(point, superview.as_deref());
        let local = if view.isFlipped() {
            local
        } else {
            NSPoint::new(local.x, view.bounds().size.height - local.y)
        };
        let inside = |hole: &NSRect| {
            local.x >= hole.origin.x
                && local.x < hole.origin.x + hole.size.width
                && local.y >= hole.origin.y
                && local.y < hole.origin.y + hole.size.height
        };
        if holes.iter().any(inside) {
            return std::ptr::null_mut();
        }
    }
    let Some(inherited) = PAGE_HIT_TEST.with(|cell| cell.get()) else {
        return std::ptr::null_mut();
    };
    // SAFETY: `inherited` is the `hitTest:` implementation this function stands in
    // front of, so it has the same signature.
    let inherited = unsafe { std::mem::transmute::<Imp, HitTest>(inherited) };
    // SAFETY: passes AppKit's own arguments straight to the original `hitTest:`.
    unsafe { inherited(view, selector, point) }
}

pub fn history_state(pointer: *mut c_void) -> (bool, bool) {
    webview_from(pointer)
        // SAFETY: main thread (see `webview_from`), and `webview` is retained.
        .map(|webview| unsafe { (webview.canGoBack(), webview.canGoForward()) })
        .unwrap_or((false, false))
}

/// The visible page as a JPEG at 1x: plenty for a model to read at a fraction
/// of the bytes of a Retina PNG.
/// `area` is a part of the viewport in CSS pixels: left, top, width, height.
pub fn snapshot_jpeg(
    pointer: *mut c_void,
    area: Option<(f64, f64, f64, f64)>,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let (Some(webview), Some(mtm)) = (webview_from(pointer), MainThreadMarker::new()) else {
        done(Err("the tab is gone".into()));
        return;
    };
    // SAFETY: main thread, as `mtm` proves.
    let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
    let scale = webview
        .window()
        .map(|window| window.backingScaleFactor())
        .unwrap_or(1.0)
        .max(1.0);
    // SAFETY: main thread, and `webview` is retained.
    let zoom = unsafe { webview.pageZoom() }.max(0.01);
    let width = match area {
        Some((_, _, width, _)) => (width / scale).max(1.0),
        None => (webview.bounds().size.width / zoom / scale).max(1.0),
    };
    // SAFETY: `configuration` is ours and not yet handed to WebKit.
    unsafe {
        configuration.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width)));
        configuration.setAfterScreenUpdates(true);
        if let Some((left, top, width, height)) = area {
            configuration.setRect(NSRect::new(
                NSPoint::new(left * zoom, top * zoom),
                NSSize::new(width * zoom, height * zoom),
            ));
        }
    }
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let Some(done) = done.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        // SAFETY: WebKit passes a live image or nil; `retain` takes our own reference.
        let image = unsafe { Retained::retain(image) };
        let Some(image) = image else {
            // SAFETY: WebKit passes a live error or nil; `retain` takes our own reference.
            done(Err(unsafe { Retained::retain(error) }
                .map(|error| error.localizedDescription().to_string())
                .unwrap_or_else(|| "the page could not be captured".into())));
            return;
        };
        let Some(pixels) = image.TIFFRepresentation().map(|tiff| tiff.to_vec()) else {
            done(Err("could not encode the page image".into()));
            return;
        };
        // Compressing the picture takes milliseconds, and this block runs on
        // the thread that draws every window.
        std::thread::spawn(move || {
            done(jpeg_bytes(&pixels).ok_or_else(|| "could not encode the page image".to_string()));
        });
    });
    // SAFETY: main thread; WebKit copies the block and holds `configuration` for the call.
    unsafe {
        webview.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block)
    };
}

/// Where a script runs. The page world is the page's own, where scripts see
/// and can change each other's globals. The helper world shares the page's DOM
/// but none of its JavaScript, so the page cannot tamper with what runs there.
#[derive(Clone, Copy)]
pub enum World {
    Page,
    Helper,
}

/// Runs `body` as the body of an async function in the page, awaiting any
/// promise it returns. The body must return a string.
pub fn call_async(
    pointer: *mut c_void,
    body: &str,
    world: World,
    done: Box<dyn FnOnce(Result<String, String>) + Send>,
) {
    let (Some(webview), Some(mtm)) = (webview_from(pointer), MainThreadMarker::new()) else {
        done(Err("the tab is gone".into()));
        return;
    };
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let Some(done) = done.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        // SAFETY: WebKit passes a live error or nil; `retain` takes our own reference.
        if let Some(error) = unsafe { Retained::retain(error) } {
            done(Err(script_error(&error)));
            return;
        }
        // SAFETY: WebKit passes a live result or nil, valid until the block returns.
        let text = unsafe { value.as_ref() }
            .and_then(|value| value.downcast_ref::<NSString>())
            .map(|text| text.to_string());
        done(text.ok_or_else(|| "the script returned nothing readable".into()));
    });
    let world = match world {
        // SAFETY: main thread, as `mtm` proves.
        World::Page => unsafe { WKContentWorld::pageWorld(mtm) },
        World::Helper => HELPER_WORLD.with(|slot| {
            slot.borrow_mut()
                // SAFETY: main thread, as `mtm` proves; HELPER_WORLD keeps the world alive.
                .get_or_insert_with(|| unsafe {
                    WKContentWorld::worldWithName(&NSString::from_str("sikemux"), mtm)
                })
                .clone()
        }),
    };
    // SAFETY: main thread, `webview` and `world` are retained, and WebKit copies the block.
    unsafe {
        webview.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(body),
            None,
            None,
            &world,
            Some(&block),
        );
    }
}

/// WebKit's own description of a thrown exception is only "A JavaScript
/// exception occurred"; the page's message sits in the error's details.
fn script_error(error: &NSError) -> String {
    let details = error.userInfo();
    let message = details
        .objectForKey(&NSString::from_str("WKJavaScriptExceptionMessage"))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|text| text.to_string());
    message.unwrap_or_else(|| error.localizedDescription().to_string())
}

/// The whole page rather than the part on screen, as a JPEG at 1x. WebKit
/// lays it out as one tall PDF page, which is then drawn as a picture. A page
/// taller than `most` is cut at that height.
pub fn full_page_jpeg(
    pointer: *mut c_void,
    height: f64,
    most: f64,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let (Some(webview), Some(mtm)) = (webview_from(pointer), MainThreadMarker::new()) else {
        done(Err("the tab is gone".into()));
        return;
    };
    // SAFETY: main thread, as `mtm` proves.
    let configuration = unsafe { WKPDFConfiguration::new(mtm) };
    if height > most {
        let width = webview.bounds().size.width;
        // SAFETY: `configuration` is ours and not yet handed to WebKit.
        unsafe {
            configuration.setRect(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, most),
            ))
        };
    }
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
        let Some(done) = done.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        // SAFETY: WebKit passes live data or nil; `retain` takes our own reference.
        let Some(data) = (unsafe { Retained::retain(data) }) else {
            // SAFETY: WebKit passes a live error or nil; `retain` takes our own reference.
            done(Err(unsafe { Retained::retain(error) }
                .map(|error| error.localizedDescription().to_string())
                .unwrap_or_else(|| "the page could not be captured".into())));
            return;
        };
        let pixels = NSImage::initWithData(mtm.alloc::<NSImage>(), &data)
            .and_then(|image| image.TIFFRepresentation())
            .map(|tiff| tiff.to_vec());
        let Some(pixels) = pixels else {
            done(Err("could not draw the page".into()));
            return;
        };
        std::thread::spawn(move || {
            done(jpeg_bytes(&pixels).ok_or_else(|| "could not encode the page image".to_string()));
        });
    });
    // SAFETY: main thread; WebKit copies the block and holds `configuration` for the call.
    unsafe { webview.createPDFWithConfiguration_completionHandler(Some(&configuration), &block) };
}

fn jpeg_bytes(image: &[u8]) -> Option<Vec<u8>> {
    let bitmap = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(image))?;
    let quality = NSNumber::numberWithDouble(0.82);
    let properties: Retained<NSDictionary<NSString, AnyObject>> =
        // SAFETY: an immutable constant string AppKit sets up when it loads.
        NSDictionary::from_slices(&[unsafe { NSImageCompressionFactor }], &[&*quality]);
    // SAFETY: NSBitmapImageRep works off the main thread, and `properties` maps the
    // compression key to an NSNumber as AppKit expects.
    let jpeg = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::JPEG, &properties)
    }?;
    Some(jpeg.to_vec())
}

struct TabNavigationDelegateIvars {
    inner: Option<Retained<ProtocolObject<dyn WKNavigationDelegate>>>,
    document: Box<dyn Fn(DocumentEvent)>,
    health: Rc<dyn Fn(Option<TabStall>)>,
}

define_class!(
    /// Sits in front of the navigation delegate the webview came with, hearing
    /// how each top-level load goes and passing every call on to it.
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TabNavigationDelegateIvars]
    struct TabNavigationDelegate;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for TabNavigationDelegate {}

    impl TabNavigationDelegate {
        // SAFETY: matches NSObject's `respondsToSelector:`: a selector in, a BOOL out.
        #[unsafe(method(respondsToSelector:))]
        fn responds_to_selector(&self, selector: Sel) -> bool {
            // SAFETY: asks NSObject's own answer for the selector we were given.
            let own: bool = unsafe { msg_send![super(self), respondsToSelector: selector] };
            own || self.inner_responds(selector)
        }

        // SAFETY: matches `forwardingTargetForSelector:`. The returned object is not retained
        // for the caller, as the method requires, and our ivars keep it alive.
        #[unsafe(method(forwardingTargetForSelector:))]
        fn forwarding_target(&self, _selector: Sel) -> *mut AnyObject {
            self.ivars()
                .inner
                .as_ref()
                .map_or(std::ptr::null_mut(), |inner| {
                    Retained::as_ptr(inner) as *mut AnyObject
                })
        }
    }

    // SAFETY: each method below has the signature WebKit declares for its selector, and
    // WebKit calls them on the main thread with a live webview and arguments.
    unsafe impl WKNavigationDelegate for TabNavigationDelegate {
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        unsafe fn started(&self, webview: &WKWebView, navigation: Option<&WKNavigation>) {
            // SAFETY: main thread, and WebKit keeps `webview` alive for the callback.
            let url = unsafe { webview.URL() }
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string())
                .unwrap_or_default();
            (self.ivars().health)(None);
            (self.ivars().document)(DocumentEvent::Started {
                navigation: navigation_id(navigation),
                url,
            });
            if let Some(inner) = self.forward_to(sel!(webView:didStartProvisionalNavigation:)) {
                let _: () = msg_send![inner, webView: webview, didStartProvisionalNavigation: navigation];
            }
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        unsafe fn responded(
            &self,
            webview: &WKWebView,
            response: &WKNavigationResponse,
            handler: &block2::DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            // SAFETY: main thread, and WebKit keeps `response` alive for the callback.
            if unsafe { response.isForMainFrame() } {
                // SAFETY: same live response, same thread.
                let answer = unsafe { response.response() };
                let status = answer
                    .downcast_ref::<objc2_foundation::NSHTTPURLResponse>()
                    .and_then(|http| u16::try_from(http.statusCode()).ok());
                (self.ivars().document)(DocumentEvent::Responded {
                    url: answer.URL()
                        .and_then(|url| url.absoluteString())
                        .map(|url| url.to_string())
                        .unwrap_or_default(),
                    status,
                    mime_type: answer.MIMEType()
                        .map(|mime| mime.to_string())
                        .unwrap_or_default(),
                });
            }
            match self.forward_to(sel!(webView:decidePolicyForNavigationResponse:decisionHandler:)) {
                Some(inner) => {
                    let _: () = msg_send![
                        inner,
                        webView: webview,
                        decidePolicyForNavigationResponse: response,
                        decisionHandler: handler
                    ];
                }
                None => handler.call((WKNavigationResponsePolicy::Allow,)),
            }
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        unsafe fn finished(&self, webview: &WKWebView, navigation: Option<&WKNavigation>) {
            (self.ivars().document)(DocumentEvent::Finished {
                navigation: navigation_id(navigation),
            });
            if let Some(inner) = self.forward_to(sel!(webView:didFinishNavigation:)) {
                let _: () = msg_send![inner, webView: webview, didFinishNavigation: navigation];
            }
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        unsafe fn crashed(&self, webview: &WKWebView) {
            (self.ivars().health)(Some(TabStall::Crashed));
            if let Some(inner) = self.forward_to(sel!(webViewWebContentProcessDidTerminate:)) {
                let _: () = msg_send![inner, webViewWebContentProcessDidTerminate: webview];
            }
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        unsafe fn failed_before_commit(
            &self,
            webview: &WKWebView,
            navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            (self.ivars().document)(DocumentEvent::Failed {
                navigation: navigation_id(navigation),
                error: load_error(error),
            });
            if let Some(inner) = self.forward_to(sel!(webView:didFailProvisionalNavigation:withError:)) {
                let _: () = msg_send![
                    inner,
                    webView: webview,
                    didFailProvisionalNavigation: navigation,
                    withError: error
                ];
            }
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        unsafe fn failed(
            &self,
            webview: &WKWebView,
            navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            (self.ivars().document)(DocumentEvent::Failed {
                navigation: navigation_id(navigation),
                error: load_error(error),
            });
            if let Some(inner) = self.forward_to(sel!(webView:didFailNavigation:withError:)) {
                let _: () = msg_send![
                    inner,
                    webView: webview,
                    didFailNavigation: navigation,
                    withError: error
                ];
            }
        }
    }
);

impl TabNavigationDelegate {
    fn new(
        mtm: MainThreadMarker,
        inner: Option<Retained<ProtocolObject<dyn WKNavigationDelegate>>>,
        document: Box<dyn Fn(DocumentEvent)>,
        health: Rc<dyn Fn(Option<TabStall>)>,
    ) -> Retained<Self> {
        let delegate = mtm
            .alloc::<TabNavigationDelegate>()
            .set_ivars(TabNavigationDelegateIvars {
                inner,
                document,
                health,
            });
        // SAFETY: `delegate` is freshly allocated with its ivars set, and NSObject's `init`
        // takes nothing and returns that same object.
        unsafe { msg_send![super(delegate), init] }
    }

    fn inner_responds(&self, selector: Sel) -> bool {
        self.ivars().inner.as_ref().is_some_and(|inner| {
            // SAFETY: our ivars retain `inner`, and any object may be asked `respondsToSelector:`.
            let responds: bool = unsafe { msg_send![&**inner, respondsToSelector: selector] };
            responds
        })
    }

    fn forward_to(&self, selector: Sel) -> Option<&ProtocolObject<dyn WKNavigationDelegate>> {
        self.inner_responds(selector)
            .then(|| self.ivars().inner.as_deref())
            .flatten()
    }
}

fn navigation_id(navigation: Option<&WKNavigation>) -> usize {
    navigation.map_or(0, |navigation| navigation as *const WKNavigation as usize)
}

/// WebKit's description of why a load failed, with its code, since the same
/// words cover several causes. A load stopped for another is only "cancelled".
fn load_error(error: &NSError) -> String {
    let (domain, code) = (error.domain().to_string(), error.code());
    if domain == "NSURLErrorDomain" && code == -999 {
        return "cancelled".into();
    }
    format!("{} ({domain} {code})", error.localizedDescription())
}

struct TabUiDelegateIvars {
    inner: Option<Retained<ProtocolObject<dyn WKUIDelegate>>>,
    tab_id: String,
    dialogs: RefCell<Burst>,
    dialog: Rc<dyn Fn(Option<PageDialog>)>,
    upload: Box<dyn Fn() -> Option<Vec<std::path::PathBuf>>>,
}

define_class!(
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TabUiDelegateIvars]
    struct TabUiDelegate;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for TabUiDelegate {}

    // SAFETY: each method below has the signature WebKit declares for its selector, and
    // WebKit calls them on the main thread with a live webview and arguments.
    unsafe impl WKUIDelegate for TabUiDelegate {
        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        unsafe fn alert(
            &self,
            webview: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let done = handler.copy();
            present_sheet(
                self.ivars(),
                webview,
                frame,
                &message.to_string(),
                Sheet::Alert,
                move |_, _| {
                    done.call(());
                },
            );
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        unsafe fn confirm(
            &self,
            webview: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(Bool)>,
        ) {
            let done = handler.copy();
            present_sheet(
                self.ivars(),
                webview,
                frame,
                &message.to_string(),
                Sheet::Confirm,
                move |accepted, _| {
                    done.call((Bool::new(accepted),));
                },
            );
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        unsafe fn prompt(
            &self,
            webview: &WKWebView,
            prompt: &NSString,
            default_text: Option<&NSString>,
            frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(*mut NSString)>,
        ) {
            let done = handler.copy();
            let default = default_text
                .map(|text| text.to_string())
                .unwrap_or_default();
            present_sheet(
                self.ivars(),
                webview,
                frame,
                &prompt.to_string(),
                Sheet::Prompt(default),
                move |accepted, text| {
                    if accepted {
                        let answer = NSString::from_str(&text);
                        done.call((Retained::as_ptr(&answer) as *mut NSString,));
                    } else {
                        done.call((std::ptr::null_mut(),));
                    }
                },
            );
        }

        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        unsafe fn media_capture(
            &self,
            _webview: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            _kind: WKMediaCaptureType,
            decision: &block2::DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            decision.call((WKPermissionDecision::Deny,));
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        unsafe fn open_panel(
            &self,
            webview: &WKWebView,
            parameters: &WKOpenPanelParameters,
            frame: &WKFrameInfo,
            handler: &block2::DynBlock<
                dyn Fn(*const objc2_foundation::NSArray<objc2_foundation::NSURL>),
            >,
        ) {
            if let Some(mut paths) = (self.ivars().upload)() {
                if !parameters.allowsMultipleSelection() {
                    paths.truncate(1);
                }
                let urls: Vec<Retained<objc2_foundation::NSURL>> = paths
                    .iter()
                    .map(|path| {
                        objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(
                            &path.to_string_lossy(),
                        ))
                    })
                    .collect();
                let chosen = objc2_foundation::NSArray::from_retained_slice(&urls);
                handler.call((Retained::as_ptr(&chosen),));
                return;
            }
            match &self.ivars().inner {
                Some(inner) => {
                    let _: () = msg_send![
                        &**inner,
                        webView: webview,
                        runOpenPanelWithParameters: parameters,
                        initiatedByFrame: frame,
                        completionHandler: handler
                    ];
                }
                None => handler.call((std::ptr::null(),)),
            }
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        unsafe fn create_web_view(
            &self,
            webview: &WKWebView,
            configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            match self.ivars().inner.as_ref() {
                Some(inner) => msg_send![
                    &**inner,
                    webView: webview,
                    createWebViewWithConfiguration: configuration,
                    forNavigationAction: action,
                    windowFeatures: features
                ],
                None => None,
            }
        }
    }
);

impl TabUiDelegate {
    fn new(
        mtm: MainThreadMarker,
        inner: Option<Retained<ProtocolObject<dyn WKUIDelegate>>>,
        tab_id: String,
        dialog: Rc<dyn Fn(Option<PageDialog>)>,
        upload: Box<dyn Fn() -> Option<Vec<std::path::PathBuf>>>,
    ) -> Retained<Self> {
        let delegate = mtm.alloc::<TabUiDelegate>().set_ivars(TabUiDelegateIvars {
            inner,
            tab_id,
            dialogs: RefCell::new(Burst::new(DIALOG_LIMIT, DIALOG_WINDOW)),
            dialog,
            upload,
        });
        // SAFETY: `delegate` is freshly allocated with its ivars set, and NSObject's `init`
        // takes nothing and returns that same object.
        unsafe { msg_send![super(delegate), init] }
    }
}

enum Sheet {
    Alert,
    Confirm,
    Prompt(String),
}

/// A page dialog as a sheet on the app window, the way Safari shows them. The
/// page waits on `answer`, so every path must call it exactly once. The agent
/// answers through `answer_dialog`, which ends the same sheet.
fn present_sheet(
    tab: &TabUiDelegateIvars,
    webview: &WKWebView,
    frame: &WKFrameInfo,
    message: &str,
    sheet: Sheet,
    answer: impl Fn(bool, String) + 'static,
) {
    let (Some(window), Some(mtm)) = (webview.window(), MainThreadMarker::new()) else {
        answer(false, String::new());
        return;
    };
    if !tab.dialogs.borrow_mut().admit(Instant::now()) {
        answer(false, String::new());
        return;
    }
    // SAFETY: main thread, and WebKit keeps `frame` alive while its dialog callback runs.
    let host = unsafe { frame.securityOrigin().host().to_string() };
    let alert = NSAlert::new(mtm);
    let title = if host.is_empty() {
        "This page says".to_owned()
    } else {
        format!("{host} says")
    };
    alert.setMessageText(&NSString::from_str(&title));
    alert.setInformativeText(&NSString::from_str(message));
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    if !matches!(sheet, Sheet::Alert) {
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    }
    let field = match &sheet {
        Sheet::Prompt(default) => {
            let field = NSTextField::initWithFrame(
                mtm.alloc::<NSTextField>(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 24.0)),
            );
            field.setStringValue(&NSString::from_str(default));
            alert.setAccessoryView(Some(&field));
            let _ = window.makeFirstResponder(Some(&field));
            Some(field)
        }
        _ => None,
    };
    let described = PageDialog {
        kind: match &sheet {
            Sheet::Alert => "alert",
            Sheet::Confirm => "confirm",
            Sheet::Prompt(_) => "prompt",
        },
        message: message.to_owned(),
        default_text: match &sheet {
            Sheet::Prompt(default) => Some(default.clone()),
            _ => None,
        },
    };
    OPEN_DIALOGS.with(|open| {
        open.borrow_mut().insert(
            tab.tab_id.clone(),
            OpenDialog {
                alert: alert.clone(),
                field: field.clone(),
            },
        );
    });
    (tab.dialog)(Some(described));
    let (tab_id, closed) = (tab.tab_id.clone(), tab.dialog.clone());
    let block = RcBlock::new(move |response: NSModalResponse| {
        OPEN_DIALOGS.with(|open| open.borrow_mut().remove(&tab_id));
        closed(None);
        let accepted = response == NSAlertFirstButtonReturn;
        let text = field
            .as_ref()
            .map(|field| field.stringValue().to_string())
            .unwrap_or_default();
        answer(accepted, text);
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&block));
}

/// Ends the tab's open dialog as if the person pressed OK or Cancel, typing
/// `text` into a prompt first.
pub fn answer_dialog(tab_id: &str, accept: bool, text: Option<&str>) -> Result<(), String> {
    let (alert, field) = OPEN_DIALOGS
        .with(|open| {
            open.borrow()
                .get(tab_id)
                .map(|dialog| (dialog.alert.clone(), dialog.field.clone()))
        })
        .ok_or("this tab has no open dialog")?;
    if let (Some(field), Some(text)) = (field, text) {
        field.setStringValue(&NSString::from_str(text));
    }
    let sheet = alert.window();
    let parent = sheet.sheetParent().ok_or("the dialog is not showing")?;
    parent.endSheet_returnCode(
        &sheet,
        if accept {
            NSAlertFirstButtonReturn
        } else {
            NSAlertSecondButtonReturn
        },
    );
    Ok(())
}

/// Command chords are the app's, not the page's, apart from the editing set
/// every text field expects to keep and the menu's own Quit, Hide and Minimize.
fn forwards_chord(key: &str, flags: NSEventModifierFlags) -> bool {
    if flags.contains(NSEventModifierFlags::Control) || flags.contains(NSEventModifierFlags::Option)
    {
        return false;
    }
    let mut chars = key.chars();
    let (Some(char), None) = (chars.next(), chars.next()) else {
        return false;
    };
    if !char.is_ascii_graphic() {
        return false;
    }
    !matches!(
        char.to_ascii_lowercase(),
        'a' | 'c' | 'v' | 'x' | 'z' | 'y' | 'q' | 'h' | 'm'
    )
}

fn dom_code(key_code: u16, key: &str) -> String {
    let named = match key_code {
        36 => "Enter",
        48 => "Tab",
        49 => "Space",
        51 => "Backspace",
        53 => "Escape",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        23 => "Digit5",
        22 => "Digit6",
        26 => "Digit7",
        28 => "Digit8",
        25 => "Digit9",
        29 => "Digit0",
        33 => "BracketLeft",
        30 => "BracketRight",
        27 => "Minus",
        24 => "Equal",
        50 => "Backquote",
        _ => "",
    };
    if !named.is_empty() {
        return named.into();
    }
    let Some(char) = key.chars().next() else {
        return String::new();
    };
    match char {
        'a'..='z' | 'A'..='Z' => format!("Key{}", char.to_ascii_uppercase()),
        '0'..='9' => format!("Digit{char}"),
        '[' => "BracketLeft".into(),
        ']' => "BracketRight".into(),
        ',' => "Comma".into(),
        '.' => "Period".into(),
        '/' => "Slash".into(),
        ';' => "Semicolon".into(),
        '\'' => "Quote".into(),
        '-' => "Minus".into(),
        '=' => "Equal".into(),
        '`' => "Backquote".into(),
        '\\' => "Backslash".into(),
        _ => String::new(),
    }
}

/// Tab under the key window's first responder, if any.
fn focused_tab(event: &NSEvent, mtm: MainThreadMarker) -> Option<(String, String)> {
    let window = event.window(mtm)?;
    let responder = window.firstResponder()?;
    let view = responder.downcast_ref::<NSView>()?;
    TABS.with(|tabs| {
        tabs.borrow()
            .iter()
            .find(|(_, tab)| view.isDescendantOf(&tab.webview))
            .map(|(id, tab)| (id.clone(), tab.agent_id.clone()))
    })
}

pub fn install_shortcuts(app: AppHandle) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        let pass = event.as_ptr();
        // SAFETY: AppKit hands the monitor a live event for the length of the call.
        let event = unsafe { event.as_ref() };
        if super::input::stop_returned_key(event) {
            return std::ptr::null_mut();
        }
        super::input::note_person(event);
        let flags = event.modifierFlags();
        if !flags.contains(NSEventModifierFlags::Command) {
            return pass;
        }
        let key = event
            .charactersIgnoringModifiers()
            .map(|chars| chars.to_string())
            .unwrap_or_default();
        if !forwards_chord(&key, flags) {
            return pass;
        }
        let Some((tab_id, agent_id)) = focused_tab(event, mtm) else {
            return pass;
        };
        let _ = app.emit(
            BROWSER_SHORTCUT_EVENT,
            BrowserShortcut {
                agent_id,
                tab_id,
                code: dom_code(event.keyCode(), &key),
                key,
                shift: flags.contains(NSEventModifierFlags::Shift),
                alt: false,
            },
        );
        std::ptr::null_mut()
    });
    // SAFETY: main thread (checked above). AppKit copies the block, and the monitor it
    // returns is kept in SHORTCUT_MONITOR so it stays installed.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
    };
    SHORTCUT_MONITOR.with(|slot| *slot.borrow_mut() = monitor);
    watch_the_person();
}

/// The person's own pointer and modifier keys, which AppKit delivers through
/// the app, unlike the agent's events sent straight to a tab.
fn watch_the_person() {
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the length of the call.
        super::input::note_person(unsafe { event.as_ref() });
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
    super::input::guard_cursor();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_chords_forward_and_editing_chords_stay_with_the_page() {
        let plain = NSEventModifierFlags::Command;
        assert!(forwards_chord("t", plain));
        assert!(forwards_chord("[", plain));
        assert!(forwards_chord("1", plain));
        assert!(!forwards_chord("c", plain));
        assert!(!forwards_chord("Z", plain | NSEventModifierFlags::Shift));
        assert!(!forwards_chord("t", plain | NSEventModifierFlags::Option));
        assert!(!forwards_chord("", plain));
        assert!(!forwards_chord("\u{F729}", plain));
        assert!(!forwards_chord("q", plain));
        assert!(!forwards_chord("h", plain));
        assert!(!forwards_chord("m", plain));
    }

    #[test]
    fn key_codes_become_dom_codes_the_keymap_matches_on() {
        assert_eq!(dom_code(17, "t"), "KeyT");
        assert_eq!(dom_code(18, "1"), "Digit1");
        assert_eq!(dom_code(33, "["), "BracketLeft");
        assert_eq!(dom_code(33, "{"), "BracketLeft");
        assert_eq!(dom_code(18, "!"), "Digit1");
        assert_eq!(dom_code(36, "\r"), "Enter");
        assert_eq!(dom_code(99, "\u{F704}"), "");
    }

    /// One red pixel, the smallest picture AppKit will decode.
    const PIXEL: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn a_page_image_is_encoded_away_from_the_main_thread() {
        let encoded = std::thread::spawn(|| jpeg_bytes(PIXEL))
            .join()
            .expect("the encoder thread finished")
            .expect("the picture is encoded");
        assert_eq!(&encoded[..2], &[0xff, 0xd8], "that is not a JPEG");
        assert_eq!(
            std::thread::spawn(|| jpeg_bytes(b"not a picture"))
                .join()
                .expect("the encoder thread finished"),
            None
        );
    }
}
