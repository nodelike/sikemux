//! The parts of a browser tab only AppKit can do: page dialogs as window
//! sheets, history without a script round trip, the address of a page that
//! moved without loading anything, and app shortcuts pressed while the page
//! owns the keyboard. Everything here runs on the main thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once, OnceLock};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, ProtocolObject, Sel};
use objc2::{
    define_class, msg_send, sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAutoresizingMaskOptions,
    NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventPhase, NSEventType, NSImage, NSImageCompressionFactor, NSModalResponse,
    NSModalResponseOK, NSOpenPanel, NSTextField, NSView, NSWindowOrderingMode,
};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSJSONSerialization, NSJSONWritingOptions, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSNumber, NSObject, NSObjectNSKeyValueCoding,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSURLRequest, NSURLResponse, NSURL,
};
use objc2_quartz_core::{kCAFillRuleEvenOdd, CALayer, CAShapeLayer, CATransaction};
use objc2_web_kit::{
    WKContentWorld, WKDownload, WKDownloadDelegate, WKFrameInfo, WKMediaCaptureType, WKNavigation,
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationResponse,
    WKNavigationResponsePolicy, WKOpenPanelParameters, WKPDFConfiguration, WKPermissionDecision,
    WKSecurityOrigin, WKSnapshotConfiguration, WKUIDelegate, WKUserScript,
    WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};
use sikemux_process::sync::Locked;

use crate::burst::Burst;
use crate::documents::DocumentEvent;
use crate::{BrowserShortcut, PageDialog, PageLoad, TabEvents, TabStall};

/// The property the tab watches to hear about a page that moved on its own.
const URL_KEY_PATH: &str = "URL";
const TITLE_KEY_PATH: &str = "title";
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

define_class!(
    /// A tab's page as a child of the app's window. App chords go on to the
    /// window's menu rather than stopping in the page, and the first click on
    /// a page in a window that is not key reaches the page.
    // SAFETY: WKWebView subclassed with no ivars and no dealloc, used on the main thread.
    #[unsafe(super(WKWebView))]
    #[thread_kind = MainThreadOnly]
    struct TabWebView;

    impl TabWebView {
        // SAFETY: matches NSView's `performKeyEquivalent:`: an event in, a BOOL out.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, _event: &NSEvent) -> Bool {
            Bool::NO
        }

        // SAFETY: matches NSView's `acceptsFirstMouse:`: an event in, a BOOL out.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> Bool {
            Bool::YES
        }

        // SAFETY: matches NSResponder's `otherMouseDown:`: an event in, nothing out.
        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            if !history_button(self, event, true) {
                // SAFETY: hands the event on to WKWebView's own handler, on the main thread.
                let _: () = unsafe { msg_send![super(self), otherMouseDown: event] };
            }
        }

        // SAFETY: matches NSResponder's `otherMouseUp:`: an event in, nothing out.
        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            if !history_button(self, event, false) {
                // SAFETY: as above.
                let _: () = unsafe { msg_send![super(self), otherMouseUp: event] };
            }
        }
    }
);

/// WebKit gives a mouse's back and forward buttons to nobody, so the page hears
/// them as DOM events, and one it leaves alone goes back or forward on release.
fn history_button(view: &TabWebView, event: &NSEvent, down: bool) -> bool {
    let button = match event.buttonNumber() {
        3 => 3,
        4 => 4,
        _ => return false,
    };
    let flags = event.modifierFlags();
    let point = view.convertPoint_fromView(event.locationInWindow(), None);
    let (x, y) = (point.x.round() as i64, point.y.round() as i64);
    let name = if down { "mousedown" } else { "mouseup" };
    let script = format!(
        r#"(() => {{
  const target = document.elementFromPoint({x}, {y}) || document.body || document.documentElement;
  const event = new MouseEvent("{name}", {{
    view: window, bubbles: true, cancelable: true, composed: true,
    button: {button}, buttons: {buttons}, detail: {clicks},
    clientX: {x}, clientY: {y}, screenX: window.screenX + {x}, screenY: window.screenY + {y},
    ctrlKey: {ctrl}, metaKey: {meta}, shiftKey: {shift}, altKey: {alt},
  }});
  target.dispatchEvent(event);
  if (!event.defaultPrevented && "{name}" === "mouseup") {{
    if ({button} === 3) history.back();
    if ({button} === 4) history.forward();
  }}
}})()"#,
        buttons = NSEvent::pressedMouseButtons(),
        clicks = event.clickCount(),
        ctrl = flags.contains(NSEventModifierFlags::Control),
        meta = flags.contains(NSEventModifierFlags::Command),
        shift = flags.contains(NSEventModifierFlags::Shift),
        alt = flags.contains(NSEventModifierFlags::Option),
    );
    // SAFETY: main thread, where AppKit delivers mouse events, and the view is alive.
    unsafe { view.evaluateJavaScript_completionHandler(&NSString::from_str(&script), None) };
    true
}

/* AppKit throws if a view is freed while anything is still watching it, so the
tab lets go of what it watches before it lets go of either of them. */
impl Drop for NativeTab {
    fn drop(&mut self) {
        let watched = [URL_KEY_PATH, TITLE_KEY_PATH, RESPONSIVE_KEY_PATH];
        let count = if self.watches_responsiveness { 3 } else { 2 };
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

/// What a page is cut to: its visible part alone, or that less the holes over it.
enum PageMask {
    Rect(Retained<CALayer>),
    Shape(Retained<CAShapeLayer>),
}

thread_local! {
    static TABS: RefCell<HashMap<String, NativeTab>> = RefCell::new(HashMap::new());
    static SHORTCUT_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static PERSON_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static OPEN_DIALOGS: RefCell<HashMap<String, OpenDialog>> = RefCell::new(HashMap::new());
    static HOLES: RefCell<HashMap<usize, Vec<NSRect>>> = RefCell::new(HashMap::new());
    static SHADES: RefCell<HashMap<usize, Retained<CALayer>>> = RefCell::new(HashMap::new());
    static MASKS: RefCell<HashMap<usize, PageMask>> = RefCell::new(HashMap::new());
    static PAGE_HIT_TEST: std::cell::Cell<Option<Imp>> = const { std::cell::Cell::new(None) };
    static PAGE_UNUSED_GESTURE: std::cell::Cell<Option<Imp>> = const { std::cell::Cell::new(None) };
    /// Whether the swipe under way belongs to the stage, once its first sideways move says so.
    static SWIPE_TO_STAGE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// WebKit tears a named world down once nothing holds it, and the element
    /// numbers the agent was given go with it.
    static HELPER_WORLD: RefCell<Option<Retained<WKContentWorld>>> = const { RefCell::new(None) };
}

/// A page dialog showing as a sheet, kept so the agent can answer it too.
struct OpenDialog {
    alert: Retained<NSAlert>,
    field: Option<Retained<NSTextField>>,
}

/// Makes the tab's page as a child of `parent`, hidden and laid out at `size`
/// off screen, and starts loading `url`. Page dialogs get a sheet, the page's
/// address, title, loads and downloads go to `events`, and the view is
/// remembered so shortcuts can tell which tab has focus.
/// The window view a tab goes into, and whether under its other children or on top.
pub struct Parent<'a> {
    pub view: &'a NSView,
    pub beneath: bool,
}

impl Parent<'_> {
    fn adopt(&self, child: &NSView) {
        if self.beneath {
            self.view
                .addSubview_positioned_relativeTo(child, NSWindowOrderingMode::Below, None);
        } else {
            self.view.addSubview(child);
        }
    }
}

pub fn open(
    parent: Parent<'_>,
    agent_id: String,
    tab_id: String,
    url: &str,
    user_agent: &str,
    size: (f64, f64),
    events: Arc<dyn TabEvents>,
) -> Retained<WKWebView> {
    let mtm = parent.view.mtm();
    // SAFETY: main thread, as `mtm` proves; the configuration is fresh and only ours.
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    let yes = NSNumber::numberWithBool(true);
    // SAFETY: main thread, and every value set is one WebKit reads with that type.
    unsafe {
        let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            WKUserScript::alloc(mtm),
            &NSString::from_str(crate::RECORDER_SCRIPT),
            WKUserScriptInjectionTime::AtDocumentStart,
            true,
        );
        configuration.userContentController().addUserScript(&script);
        let preferences = configuration.preferences();
        preferences.setElementFullscreenEnabled(true);
        preferences.setValue_forKey(Some(&yes), &NSString::from_str("tabFocusesLinks"));
        preferences.setValue_forKey(
            Some(&yes),
            &NSString::from_str("allowsPictureInPictureMediaPlayback"),
        );
        if cfg!(debug_assertions) {
            preferences.setValue_forKey(Some(&yes), &NSString::from_str("developerExtrasEnabled"));
        }
    }
    let frame = NSRect::new(
        NSPoint::new(crate::PARKED_ORIGIN, crate::PARKED_ORIGIN),
        NSSize::new(size.0, size.1),
    );
    let webview: Retained<WKWebView> = without_activating(|| {
        // SAFETY: a fresh TabWebView, initialised as WKWebView documents, on the main thread.
        let view: Retained<TabWebView> = unsafe {
            msg_send![super(TabWebView::alloc(mtm).set_ivars(())), initWithFrame: frame, configuration: &*configuration]
        };
        Retained::into_super(view)
    });
    webview.setHidden(true);
    webview.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
    // SAFETY: main thread, and `webview` is retained.
    unsafe {
        webview.setCustomUserAgent(Some(&NSString::from_str(user_agent)));
        if cfg!(debug_assertions) && webview.respondsToSelector(sel!(setInspectable:)) {
            webview.setInspectable(true);
        }
    }
    let delegate = TabUiDelegate::new(mtm, tab_id.clone(), events.clone());
    let download = TabDownloadDelegate::new(mtm, events.clone());
    let navigation = TabNavigationDelegate::new(mtm, events.clone(), download);
    let address_observer = AddressObserver::new(mtm, events);
    let watches_responsiveness = webview.respondsToSelector(sel!(_webProcessIsResponsive));
    // SAFETY: main thread. WebKit holds delegates and observers weakly, so the tab keeps
    // them alive in TABS, and `Drop` removes the observers before letting go.
    unsafe {
        webview.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        webview.setNavigationDelegate(Some(ProtocolObject::from_ref(&*navigation)));
        webview.setAllowsBackForwardNavigationGestures(true);
        webview.setAllowsMagnification(true);
        let watched: &[&str] = if watches_responsiveness {
            &[URL_KEY_PATH, TITLE_KEY_PATH, RESPONSIVE_KEY_PATH]
        } else {
            &[URL_KEY_PATH, TITLE_KEY_PATH]
        };
        for key_path in watched {
            webview.addObserver_forKeyPath_options_context(
                &address_observer,
                &NSString::from_str(key_path),
                NSKeyValueObservingOptions::empty(),
                std::ptr::null_mut(),
            );
        }
    }
    parent.adopt(&webview);
    let _ = hand_unused_swipes_to_the_stage(&webview);
    TABS.with(|tabs| {
        tabs.borrow_mut().insert(
            tab_id,
            NativeTab {
                agent_id,
                webview: webview.clone(),
                _delegate: delegate,
                _navigation: navigation,
                address_observer,
                watches_responsiveness,
            },
        );
    });
    load(&webview, url);
    webview
}

/// Runs `script` in the page's own world and hands back its value as JSON, or
/// nothing when it has none or throws.
pub fn evaluate(webview: &WKWebView, script: &str, done: Box<dyn FnOnce(String) + Send>) {
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
        let Some(done) = done.locked().take() else {
            return;
        };
        // SAFETY: WebKit passes a live result or nil, valid until the block returns.
        let json = unsafe { value.as_ref() }
            .and_then(|value| {
                // SAFETY: WebKit only hands back values JSON can hold: strings, numbers,
                // booleans, arrays, dictionaries and null.
                unsafe {
                    NSJSONSerialization::dataWithJSONObject_options_error(
                        value,
                        NSJSONWritingOptions::FragmentsAllowed,
                    )
                }
                .ok()
            })
            .map(|data| String::from_utf8_lossy(&data.to_vec()).into_owned())
            .unwrap_or_default();
        done(json);
    });
    // SAFETY: main thread, where `Tab` runs every call; WebKit copies the block.
    unsafe {
        webview.evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&block));
    }
}

/// Starts loading `url` in the tab, if it is one.
pub fn load(webview: &WKWebView, url: &str) {
    let Some(address) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return;
    };
    // SAFETY: main thread, where `Tab` runs every call, and `webview` is alive.
    unsafe {
        let _ = webview.loadRequest(&NSURLRequest::requestWithURL(&address));
    }
}

/// WebKit stops a page's animation frames while another app covers the
/// window. A tab that is loading or being driven must keep them running.
pub fn keep_running_when_covered(webview: &WKWebView, keep_running: bool) {
    let webview = webview.retain();
    let selector = sel!(_setWindowOcclusionDetectionEnabled:);
    if webview.respondsToSelector(selector) {
        let enabled = Bool::new(!keep_running);
        // SAFETY: `respondsToSelector` just confirmed this private method exists; it takes
        // one BOOL and returns nothing.
        let _: () = unsafe { msg_send![&*webview, _setWindowOcclusionDetectionEnabled: enabled] };
    }
}

/// Keeps a page already on screen running while other windows cover it. WebKit
/// only looks again at whether a page shows when its view is hidden or shown.
pub fn keep_shown_page_running_when_covered(webview: &WKWebView) {
    keep_running_when_covered(webview, true);
    let webview = webview.retain();
    webview.setHidden(true);
    webview.setHidden(false);
}

pub fn forget(tab_id: &str) {
    let _ = answer_dialog(tab_id, false, None);
    TABS.with(|tabs| {
        if let Some(tab) = tabs.borrow_mut().remove(tab_id) {
            HOLES.with(|holes| holes.borrow_mut().remove(&view_key(&tab.webview)));
            SHADES.with(|shades| shades.borrow_mut().remove(&view_key(&tab.webview)));
            MASKS.with(|masks| masks.borrow_mut().remove(&view_key(&tab.webview)));
        }
    });
}

struct AddressObserverIvars {
    events: Arc<dyn TabEvents>,
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
                self.ivars().events.health((!responsive).then_some(TabStall::Unresponsive));
                return;
            }
            if key_path.is_some_and(|path| path.to_string() == TITLE_KEY_PATH) {
                // SAFETY: `webview` is the observed view, alive for this call on the main thread.
                let title = unsafe { webview.title() }.map(|title| title.to_string());
                self.ivars().events.titled(title.unwrap_or_default());
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
            self.ivars().events.moved(address.to_string(), unsafe { webview.canGoBack() }, unsafe {
                webview.canGoForward()
            });
        }
    }
);

impl AddressObserver {
    fn new(mtm: MainThreadMarker, events: Arc<dyn TabEvents>) -> Retained<Self> {
        let observer = mtm
            .alloc::<AddressObserver>()
            .set_ivars(AddressObserverIvars { events });
        // SAFETY: `observer` is freshly allocated with its ivars set, and NSObject's `init`
        // takes nothing and returns that same object.
        unsafe { msg_send![super(observer), init] }
    }
}

pub fn history(webview: &WKWebView, delta: i32) {
    let webview = webview.retain();
    // SAFETY: main thread (where `Tab` runs every call), and `webview` is retained.
    unsafe {
        if delta < 0 {
            webview.goBack();
        } else {
            webview.goForward();
        }
    }
}

/// Send `agent` as the tab's agent string, reloading so the site sees the change.
pub fn introduce_as(webview: &WKWebView, agent: &str) {
    let webview = webview.retain();
    // SAFETY: main thread (where `Tab` runs every call), and `webview` is retained.
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
pub fn place(webview: &WKWebView, frame: NSRect, page: Option<NSSize>) {
    let webview = webview.retain();
    let view: &NSView = &webview;
    // SAFETY: main thread (where `Tab` runs every call), and `webview` is retained.
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
pub fn clip(webview: &WKWebView, clip_left: f64, clip_right: f64, holes: Vec<(NSRect, f64)>) {
    let webview = webview.retain();
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
    let holes: Vec<(NSRect, NSRect, f64)> = holes
        .into_iter()
        .filter_map(|(whole_hole, radius)| {
            let hole = intersection(whole_hole, visible)?;
            Some((
                hole,
                whole_hole,
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
                holes.iter().map(|(hole, _, _)| *hole).collect(),
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
    let key = view_key(&webview);
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    MASKS.with(|masks| {
        let mut masks = masks.borrow_mut();
        if visible == whole && holes.is_empty() {
            masks.remove(&key);
            // SAFETY: main thread, and `layer` is retained for the whole function.
            unsafe { layer.setMask(None) };
            return;
        }
        /* A swipe clips the page on every frame it is partly off stage. The mask
        is kept and moved rather than rebuilt, and a plain rectangle is a layer's
        frame rather than a path that has to be drawn again. */
        let mask: &CALayer = if holes.is_empty() {
            if !matches!(masks.get(&key), Some(PageMask::Rect(_))) {
                let rect = CALayer::new();
                rect.setBackgroundColor(Some(&CGColor::new_generic_gray(0.0, 1.0)));
                masks.insert(key, PageMask::Rect(rect));
            }
            let Some(PageMask::Rect(rect)) = masks.get(&key) else {
                return;
            };
            rect.setFrame(flip(visible));
            rect
        } else {
            let path = CGMutablePath::new();
            // SAFETY: a null transform means none, and `path` is a fresh path only we hold.
            unsafe {
                CGMutablePath::add_rect(Some(&path), std::ptr::null(), flip(visible));
                for (hole, whole_hole, radius) in &holes {
                    add_hole(&path, flip(*hole), flip(*whole_hole), *radius);
                }
            }
            if !matches!(masks.get(&key), Some(PageMask::Shape(_))) {
                let shape = CAShapeLayer::new();
                // SAFETY: an immutable constant string QuartzCore sets up when it loads.
                shape.setFillRule(unsafe { kCAFillRuleEvenOdd });
                masks.insert(key, PageMask::Shape(shape));
            }
            let Some(PageMask::Shape(shape)) = masks.get(&key) else {
                return;
            };
            shape.setFrame(layer.bounds());
            shape.setPath(Some(&path));
            shape
        };
        let current = layer.mask();
        if current.as_deref().map(|layer| layer as *const CALayer) != Some(mask as *const CALayer) {
            // SAFETY: main thread, and the layer retains `mask` from here on.
            unsafe { layer.setMask(Some(mask)) };
        }
    });
    CATransaction::commit();
}

/// Fade the whole page, so it goes with the pane it sits in when that fades.
pub fn fade(webview: &WKWebView, opacity: f64) {
    let webview = webview.retain();
    let view: &NSView = &webview;
    // SAFETY: `setAlphaValue:` is an NSView method taking a CGFloat.
    let _: () = unsafe { msg_send![view, setAlphaValue: opacity] };
}

/// Lay a black shade of `alpha` over the page, or take it off at zero, so an
/// app panel floating on the page stands apart from it. The shade lives in the
/// page's own layer, so the mask `clip` sets cuts it where the panel is.
pub fn dim(webview: &WKWebView, alpha: f64) {
    let webview = webview.retain();
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

/// Round only the corners `hole` shares with `whole_hole`. Where the page's edge
/// cuts a menu short, the cut stays square instead of curving into the menu.
fn add_hole(path: &CGMutablePath, hole: NSRect, whole_hole: NSRect, radius: f64) {
    let same = |a: f64, b: f64| (a - b).abs() < 0.5;
    let (min_x, min_y) = (hole.origin.x, hole.origin.y);
    let (max_x, max_y) = (min_x + hole.size.width, min_y + hole.size.height);
    let left = same(min_x, whole_hole.origin.x);
    let right = same(max_x, whole_hole.origin.x + whole_hole.size.width);
    let low = same(min_y, whole_hole.origin.y);
    let high = same(max_y, whole_hole.origin.y + whole_hole.size.height);
    let corner = |x: bool, y: bool| if x && y { radius } else { 0.0 };
    let path = Some(path);
    let none = std::ptr::null();
    // SAFETY: a null transform means none.
    unsafe {
        CGMutablePath::move_to_point(path, none, (min_x + max_x) / 2.0, min_y);
        CGMutablePath::add_arc_to_point(path, none, max_x, min_y, max_x, max_y, corner(right, low));
        CGMutablePath::add_arc_to_point(
            path,
            none,
            max_x,
            max_y,
            min_x,
            max_y,
            corner(right, high),
        );
        CGMutablePath::add_arc_to_point(path, none, min_x, max_y, min_x, min_y, corner(left, high));
        CGMutablePath::add_arc_to_point(path, none, min_x, min_y, max_x, min_y, corner(left, low));
    }
    CGMutablePath::close_subpath(path);
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

/* A two-finger swipe that starts over a page goes to the page, so the stage
never hears it and cannot move. WebKit reports each scroll the page did not
use, which is what its own back and forward swipe is built on. A sideways
swipe the page's history cannot take is handed to the app's own view
instead, where it moves the stage as it would anywhere else. */
fn hand_unused_swipes_to_the_stage(view: &NSView) -> Option<()> {
    if PAGE_UNUSED_GESTURE.with(|cell| cell.get()).is_some() {
        return Some(());
    }
    let class = view.class();
    let selector = sel!(_gestureEventWasNotHandledByWebCore:);
    let inherited = class.instance_method(selector)?;
    // SAFETY: `inherited` is a real method of the view's class, so its type string lives
    // as long as the class does.
    let types = unsafe { objc2::ffi::method_getTypeEncoding(inherited) };
    let unused: UnusedGesture = gesture_unused_by_page;
    // SAFETY: `gesture_unused_by_page` has the method's exact signature and reuses its
    // type string. `class_addMethod` only adds to the class, never replaces a method.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (class as *const objc2::runtime::AnyClass).cast_mut(),
            selector,
            std::mem::transmute::<UnusedGesture, Imp>(unused),
            types,
        )
    };
    added
        .as_bool()
        .then(|| PAGE_UNUSED_GESTURE.with(|cell| cell.set(Some(inherited.implementation()))))
}

type UnusedGesture = unsafe extern "C-unwind" fn(&NSView, Sel, *mut NSEvent);

// SAFETY: only WebKit calls this, as `_gestureEventWasNotHandledByWebCore:` on a
// live view on the main thread, with an event or nil.
unsafe extern "C-unwind" fn gesture_unused_by_page(
    view: &NSView,
    selector: Sel,
    event: *mut NSEvent,
) {
    // SAFETY: WebKit passes a live event or nil for the length of the call.
    let stage = unsafe { event.as_ref() }.and_then(|event| stage_for_swipe(view, event));
    if let Some(stage) = stage {
        // SAFETY: `scrollWheel:` takes one event and returns nothing; the stage's view
        // is live and on the main thread.
        let _: () = unsafe { msg_send![&*stage, scrollWheel: event] };
        return;
    }
    let Some(inherited) = PAGE_UNUSED_GESTURE.with(|cell| cell.get()) else {
        return;
    };
    // SAFETY: `inherited` is the implementation this function stands in front of.
    let inherited = unsafe { std::mem::transmute::<Imp, UnusedGesture>(inherited) };
    // SAFETY: passes WebKit's own arguments straight through.
    unsafe { inherited(view, selector, event) }
}

/// The app's own view, when this scroll is part of a sideways swipe the page
/// had no use for and its history cannot take either.
fn stage_for_swipe(view: &NSView, event: &NSEvent) -> Option<Retained<NSView>> {
    if event.r#type() != NSEventType::ScrollWheel {
        return None;
    }
    let tab = TABS.with(|tabs| {
        tabs.borrow()
            .values()
            .find(|tab| view_key(&tab.webview) == view_key(view))
            .map(|tab| tab.webview.clone())
    })?;
    let phase = event.phase();
    if phase.intersects(NSEventPhase::MayBegin | NSEventPhase::Began) {
        SWIPE_TO_STAGE.with(|cell| cell.set(None));
    }
    let to_stage = match SWIPE_TO_STAGE.with(|cell| cell.get()) {
        Some(decided) => decided,
        None => {
            let (across, down) = (event.scrollingDeltaX(), event.scrollingDeltaY());
            if across == 0.0 || across.abs() <= down.abs() {
                return None;
            }
            // Fingers moving right ask for the page before; left for the one after.
            // SAFETY: main thread, and `tab` is retained.
            let history = unsafe {
                if across > 0.0 {
                    tab.canGoBack()
                } else {
                    tab.canGoForward()
                }
            };
            SWIPE_TO_STAGE.with(|cell| cell.set(Some(!history)));
            !history
        }
    };
    let momentum = event.momentumPhase();
    if momentum.contains(NSEventPhase::Ended)
        || (momentum.is_empty() && phase.intersects(NSEventPhase::Ended | NSEventPhase::Cancelled))
    {
        SWIPE_TO_STAGE.with(|cell| cell.set(None));
    }
    if !to_stage {
        return None;
    }
    // SAFETY: main thread, and `view` is alive for this call.
    let parent = unsafe { view.superview() }?;
    let pages: Vec<usize> = TABS.with(|tabs| {
        tabs.borrow()
            .values()
            .map(|tab| view_key(&tab.webview))
            .collect()
    });
    let web_view_class = objc2::runtime::AnyClass::get(c"WKWebView")?;
    let app_page = parent.subviews().iter().find(|sibling| {
        !pages.contains(&view_key(sibling)) && sibling.isKindOfClass(web_view_class)
    });
    Some(app_page.unwrap_or(parent))
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

pub fn history_state(webview: &WKWebView) -> (bool, bool) {
    // SAFETY: main thread, where `Tab` runs every call, and `webview` is alive.
    unsafe { (webview.canGoBack(), webview.canGoForward()) }
}

/// The visible page as a JPEG at 1x: plenty for a model to read at a fraction
/// of the bytes of a Retina PNG.
/// `area` is a part of the viewport in CSS pixels: left, top, width, height.
pub fn snapshot_jpeg(
    webview: &WKWebView,
    area: Option<(f64, f64, f64, f64)>,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let webview = webview.retain();
    let Some(mtm) = MainThreadMarker::new() else {
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
        let Some(done) = done.locked().take() else {
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

/// The page as it stands, as a JPEG at the screen's own resolution, for the app
/// to slide in its place while the stage moves. The image is taken off the main
/// thread before it is compressed, so the capture costs the window no frame.
pub fn still_jpeg(webview: &WKWebView, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
    let webview = webview.retain();
    let Some(mtm) = MainThreadMarker::new() else {
        done(None);
        return;
    };
    // SAFETY: main thread, as `mtm` proves.
    let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
        let Some(done) = done.locked().take() else {
            return;
        };
        // SAFETY: WebKit passes a live image or nil; `retain` takes our own reference.
        let Some(image) = (unsafe { Retained::retain(image) }) else {
            done(None);
            return;
        };
        // SAFETY: a null rect asks for the whole image, and no context or hints are needed.
        let picture =
            unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) };
        let Some(picture) = picture else {
            done(None);
            return;
        };
        std::thread::spawn(move || {
            // SAFETY: allocating an image rep has no thread requirement; AppKit's
            // bindings only mark the class main-thread for its drawing methods.
            let blank: objc2::rc::Allocated<NSBitmapImageRep> =
                unsafe { msg_send![NSBitmapImageRep::class(), alloc] };
            let bitmap = NSBitmapImageRep::initWithCGImage(blank, &picture);
            let quality = NSNumber::numberWithDouble(0.8);
            let properties: Retained<NSDictionary<NSString, AnyObject>> =
                // SAFETY: an immutable constant string AppKit sets up when it loads.
                NSDictionary::from_slices(&[unsafe { NSImageCompressionFactor }], &[&*quality]);
            // SAFETY: NSBitmapImageRep works off the main thread, and `properties` maps the
            // compression key to an NSNumber as AppKit expects.
            let jpeg = unsafe {
                bitmap.representationUsingType_properties(NSBitmapImageFileType::JPEG, &properties)
            };
            done(jpeg.map(|data| data.to_vec()));
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
    webview: &WKWebView,
    body: &str,
    world: World,
    done: Box<dyn FnOnce(Result<String, String>) + Send>,
) {
    let webview = webview.retain();
    let Some(mtm) = MainThreadMarker::new() else {
        done(Err("the tab is gone".into()));
        return;
    };
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let Some(done) = done.locked().take() else {
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
    webview: &WKWebView,
    height: f64,
    most: f64,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let webview = webview.retain();
    let Some(mtm) = MainThreadMarker::new() else {
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
        let Some(done) = done.locked().take() else {
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
    events: Arc<dyn TabEvents>,
    download: Retained<TabDownloadDelegate>,
}

define_class!(
    /// Decides where the page may go and hears how each top-level load goes.
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TabNavigationDelegateIvars]
    struct TabNavigationDelegate;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for TabNavigationDelegate {}

    // SAFETY: each method below has the signature WebKit declares for its selector, and
    // WebKit calls them on the main thread with a live webview and arguments.
    unsafe impl WKNavigationDelegate for TabNavigationDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn decide(
            &self,
            _webview: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: main thread, and WebKit keeps `action` alive for the callback.
            let (download, url) = unsafe {
                (
                    action.shouldPerformDownload(),
                    action
                        .request()
                        .URL()
                        .and_then(|url| url.absoluteString())
                        .map(|url| url.to_string())
                        .unwrap_or_default(),
                )
            };
            let policy = if download {
                WKNavigationActionPolicy::Download
            } else if self.ivars().events.may_load(&url) {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            };
            handler.call((policy,));
        }

        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        unsafe fn started(&self, webview: &WKWebView, navigation: Option<&WKNavigation>) {
            let events = &self.ivars().events;
            events.health(None);
            events.document(DocumentEvent::Started {
                navigation: navigation_id(navigation),
                url: address_of(webview),
            });
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        unsafe fn responded(
            &self,
            _webview: &WKWebView,
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
                self.ivars().events.document(DocumentEvent::Responded {
                    url: answer
                        .URL()
                        .and_then(|url| url.absoluteString())
                        .map(|url| url.to_string())
                        .unwrap_or_default(),
                    status,
                    mime_type: answer
                        .MIMEType()
                        .map(|mime| mime.to_string())
                        .unwrap_or_default(),
                });
            }
            // SAFETY: same live response, same thread.
            let policy = if unsafe { response.canShowMIMEType() } {
                WKNavigationResponsePolicy::Allow
            } else {
                WKNavigationResponsePolicy::Download
            };
            handler.call((policy,));
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        unsafe fn committed(&self, webview: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().events.loaded(page_load(webview, true));
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        unsafe fn finished(&self, webview: &WKWebView, navigation: Option<&WKNavigation>) {
            let events = &self.ivars().events;
            events.document(DocumentEvent::Finished {
                navigation: navigation_id(navigation),
            });
            events.loaded(page_load(webview, false));
        }

        #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
        unsafe fn action_downloads(
            &self,
            _webview: &WKWebView,
            _action: &WKNavigationAction,
            download: &WKDownload,
        ) {
            // SAFETY: main thread; the tab keeps the download delegate alive.
            unsafe {
                download.setDelegate(Some(ProtocolObject::from_ref(&*self.ivars().download)))
            };
        }

        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        unsafe fn response_downloads(
            &self,
            _webview: &WKWebView,
            _response: &WKNavigationResponse,
            download: &WKDownload,
        ) {
            // SAFETY: as above.
            unsafe {
                download.setDelegate(Some(ProtocolObject::from_ref(&*self.ivars().download)))
            };
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        unsafe fn crashed(&self, _webview: &WKWebView) {
            self.ivars().events.health(Some(TabStall::Crashed));
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        unsafe fn failed_before_commit(
            &self,
            _webview: &WKWebView,
            navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.ivars().events.document(DocumentEvent::Failed {
                navigation: navigation_id(navigation),
                error: load_error(error),
            });
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        unsafe fn failed(
            &self,
            _webview: &WKWebView,
            navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.ivars().events.document(DocumentEvent::Failed {
                navigation: navigation_id(navigation),
                error: load_error(error),
            });
        }
    }
);

impl TabNavigationDelegate {
    fn new(
        mtm: MainThreadMarker,
        events: Arc<dyn TabEvents>,
        download: Retained<TabDownloadDelegate>,
    ) -> Retained<Self> {
        let delegate = mtm
            .alloc::<TabNavigationDelegate>()
            .set_ivars(TabNavigationDelegateIvars { events, download });
        // SAFETY: `delegate` is freshly allocated with its ivars set, and NSObject's `init`
        // takes nothing and returns that same object.
        unsafe { msg_send![super(delegate), init] }
    }
}

struct TabDownloadDelegateIvars {
    events: Arc<dyn TabEvents>,
}

define_class!(
    /// Asks where each file a page saves goes, and says when it is done.
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TabDownloadDelegateIvars]
    struct TabDownloadDelegate;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for TabDownloadDelegate {}

    // SAFETY: each method below has the signature WebKit declares for its selector, and
    // WebKit calls them on the main thread with a live download.
    unsafe impl WKDownloadDelegate for TabDownloadDelegate {
        #[unsafe(method(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:))]
        unsafe fn destination(
            &self,
            download: &WKDownload,
            _response: &NSURLResponse,
            suggested: &NSString,
            handler: &block2::DynBlock<dyn Fn(*mut NSURL)>,
        ) {
            let suggested = std::path::PathBuf::from(suggested.to_string());
            match self
                .ivars()
                .events
                .download_started(download_address(download), &suggested)
            {
                Some(path) => {
                    let file = NSURL::fileURLWithPath_isDirectory(
                        &NSString::from_str(&path.to_string_lossy()),
                        false,
                    );
                    handler.call((Retained::as_ptr(&file) as *mut NSURL,));
                }
                None => handler.call((std::ptr::null_mut(),)),
            }
        }

        #[unsafe(method(downloadDidFinish:))]
        unsafe fn finished(&self, download: &WKDownload) {
            self.ivars()
                .events
                .download_finished(download_address(download), true);
        }

        #[unsafe(method(download:didFailWithError:resumeData:))]
        unsafe fn failed(&self, download: &WKDownload, _error: &NSError, _resume: Option<&NSData>) {
            self.ivars()
                .events
                .download_finished(download_address(download), false);
        }
    }
);

impl TabDownloadDelegate {
    fn new(mtm: MainThreadMarker, events: Arc<dyn TabEvents>) -> Retained<Self> {
        let delegate = mtm
            .alloc::<TabDownloadDelegate>()
            .set_ivars(TabDownloadDelegateIvars { events });
        // SAFETY: `delegate` is freshly allocated with its ivars set, and NSObject's `init`
        // takes nothing and returns that same object.
        unsafe { msg_send![super(delegate), init] }
    }
}

fn download_address(download: &WKDownload) -> String {
    // SAFETY: main thread, and WebKit keeps `download` alive for the callback.
    unsafe { download.originalRequest() }
        .and_then(|request| request.URL())
        .and_then(|url| url.absoluteString())
        .map(|url| url.to_string())
        .unwrap_or_default()
}

fn address_of(webview: &WKWebView) -> String {
    // SAFETY: main thread, and WebKit keeps `webview` alive for the callback.
    unsafe { webview.URL() }
        .and_then(|url| url.absoluteString())
        .map(|url| url.to_string())
        .unwrap_or_default()
}

fn page_load(webview: &WKWebView, started: bool) -> PageLoad {
    let (can_go_back, can_go_forward) = history_state(webview);
    PageLoad {
        started,
        url: address_of(webview),
        can_go_back,
        can_go_forward,
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
    tab_id: String,
    dialogs: RefCell<Burst>,
    events: Arc<dyn TabEvents>,
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
            if let Some(mut paths) = self.ivars().events.upload() {
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
            // The file chooser blocks the whole app and brings it to the front, so
            // an agent uploads through `browser_upload` instead.
            if crate::input::agent_input_is_latest() {
                handler.call((std::ptr::null(),));
                return;
            }
            let panel = NSOpenPanel::openPanel(webview.mtm());
            panel.setCanChooseFiles(true);
            panel.setAllowsMultipleSelection(parameters.allowsMultipleSelection());
            panel.setCanChooseDirectories(parameters.allowsDirectories());
            if panel.runModal() == NSModalResponseOK {
                let chosen = panel.URLs();
                handler.call((Retained::as_ptr(&chosen),));
            } else {
                handler.call((std::ptr::null(),));
            }
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        unsafe fn create_web_view(
            &self,
            _webview: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            // SAFETY: main thread, and WebKit keeps `action` alive for the callback.
            let url = unsafe { action.request() }
                .URL()
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string());
            if let Some(url) = url {
                self.ivars().events.new_window(url);
            }
            None
        }
    }
);

impl TabUiDelegate {
    fn new(mtm: MainThreadMarker, tab_id: String, events: Arc<dyn TabEvents>) -> Retained<Self> {
        let delegate = mtm.alloc::<TabUiDelegate>().set_ivars(TabUiDelegateIvars {
            tab_id,
            dialogs: RefCell::new(Burst::new(DIALOG_LIMIT, DIALOG_WINDOW)),
            events,
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
    let answer = AnswerOnce::new(answer);
    let (Some(window), Some(mtm)) = (webview.window(), MainThreadMarker::new()) else {
        answer.give(false, String::new());
        return;
    };
    if !tab.dialogs.borrow_mut().admit(Instant::now()) {
        answer.give(false, String::new());
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
    tab.events.dialog(Some(described));
    let (tab_id, closed) = (tab.tab_id.clone(), tab.events.clone());
    let block = RcBlock::new(move |response: NSModalResponse| {
        OPEN_DIALOGS.with(|open| open.borrow_mut().remove(&tab_id));
        closed.dialog(None);
        let accepted = response == NSAlertFirstButtonReturn;
        let text = field
            .as_ref()
            .map(|field| field.stringValue().to_string())
            .unwrap_or_default();
        answer.give(accepted, text);
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&block));
}

/// WebKit throws when a dialog's answer is dropped unanswered, which aborts the
/// app, so a sheet AppKit discards without running its block answers Cancel.
struct AnswerOnce<F: Fn(bool, String)> {
    answer: F,
    given: Cell<bool>,
}

impl<F: Fn(bool, String)> AnswerOnce<F> {
    fn new(answer: F) -> Self {
        Self {
            answer,
            given: Cell::new(false),
        }
    }

    fn give(&self, accepted: bool, text: String) {
        if !self.given.replace(true) {
            (self.answer)(accepted, text);
        }
    }
}

impl<F: Fn(bool, String)> Drop for AnswerOnce<F> {
    fn drop(&mut self) {
        self.give(false, String::new());
    }
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

/// The tab `view` belongs to, if it is part of one.
pub(super) fn tab_holding(view: &NSView) -> Option<Retained<WKWebView>> {
    TABS.with(|tabs| {
        tabs.borrow()
            .values()
            .find(|tab| view.isDescendantOf(&tab.webview))
            .map(|tab| tab.webview.clone())
    })
}

/// Watches the keyboard for app chords pressed while a page has focus, and
/// hands each one to `shortcut` instead of the page.
pub fn install_shortcuts(shortcut: impl Fn(BrowserShortcut) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        let pass = event.as_ptr();
        // SAFETY: AppKit hands the monitor a live event for the length of the call.
        let event = unsafe { event.as_ref() };
        if crate::input::stop_returned_key(event) {
            return std::ptr::null_mut();
        }
        crate::input::note_person(event);
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
        shortcut(BrowserShortcut {
            agent_id,
            tab_id,
            code: dom_code(event.keyCode(), &key),
            key,
            shift: flags.contains(NSEventModifierFlags::Shift),
            alt: false,
        });
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

static TABS_BEING_MADE: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_ACTIVATE: OnceLock<Imp> = OnceLock::new();
static ORIGINAL_ACTIVATE_IGNORING: OnceLock<Imp> = OnceLock::new();

type Activate = unsafe extern "C-unwind" fn(*mut AnyObject, Sel);
type ActivateIgnoring = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Bool);

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

fn guard_activation() {
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
fn watch_the_person() {
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
