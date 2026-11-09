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
use std::time::Instant;

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
    NSData, NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSNumber,
    NSObject, NSObjectNSKeyValueCoding, NSObjectNSKeyValueObserverRegistration, NSObjectProtocol,
    NSPoint, NSRect, NSSize, NSString, NSURLRequest, NSURLResponse, NSURL,
};
use objc2_quartz_core::{kCAFillRuleEvenOdd, CACornerMask, CALayer, CAShapeLayer, CATransaction};
use objc2_web_kit::{
    WKDownload, WKDownloadDelegate, WKFrameInfo, WKFullscreenState, WKMediaCaptureType,
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationResponse, WKNavigationResponsePolicy, WKOpenPanelParameters, WKPDFConfiguration,
    WKPermissionDecision, WKSecurityOrigin, WKSnapshotConfiguration, WKUIDelegate, WKUserScript,
    WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};
use sikemux_process::sync::Locked;

mod activation;
mod delegates;
mod dialogs;
mod frame;
mod scripts;
mod shortcuts;
mod snapshot;

pub use activation::*;
use delegates::*;
pub use dialogs::*;
pub use frame::*;
pub use scripts::{call_async, evaluate};
pub use shortcuts::*;
pub use snapshot::*;

use crate::burst::Burst;
use crate::documents::DocumentEvent;
use crate::{BrowserShortcut, Opener, PageDialog, PageLoad, TabEvents, TabStall};

/// The property the tab watches to hear about a page that moved on its own.
const URL_KEY_PATH: &str = "URL";
const TITLE_KEY_PATH: &str = "title";
/// WebKit's own verdict on whether the page's process still answers input.
const RESPONSIVE_KEY_PATH: &str = "_webProcessIsResponsive";

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
}

/// A page dialog showing as a sheet, kept so the agent can answer it too.
struct OpenDialog {
    alert: Retained<NSAlert>,
    field: Option<Retained<NSTextField>>,
}

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

pub enum Start<'a> {
    Load(&'a str),
    /// The window another page opened, which WebKit loads itself and keeps linked to that page.
    Opened(Opener),
}

/// Makes the tab's page as a child of `parent`, hidden and laid out at `size`
/// off screen, and starts loading `start`. Page dialogs get a sheet, the page's
/// address, title, loads and downloads go to `events`, and the view is
/// remembered so shortcuts can tell which tab has focus.
pub fn open(
    parent: Parent<'_>,
    agent_id: String,
    tab_id: String,
    start: Start<'_>,
    user_agent: &str,
    size: (f64, f64),
    events: Arc<dyn TabEvents>,
) -> Retained<WKWebView> {
    let mtm = parent.view.mtm();
    let (configuration, url) = match start {
        Start::Load(url) => (tab_configuration(mtm), Some(url)),
        Start::Opened(Opener(configuration)) => (configuration, None),
    };
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
    leave_insets_to_the_app(&webview);
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
    if let Some(url) = url {
        load(&webview, url);
    }
    webview
}

/// What a tab's page is made with: the recorder in every document, and the preferences
/// tabs browse with.
fn tab_configuration(mtm: MainThreadMarker) -> Retained<WKWebViewConfiguration> {
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
        scripts::listen(&configuration, mtm);
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
    configuration
}

pub fn load(webview: &WKWebView, url: &str) {
    let Some(address) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return;
    };
    out_of_fullscreen(webview, move |webview| {
        // SAFETY: main thread, where `Tab` runs every call, and `webview` is alive.
        unsafe {
            let _ = webview.loadRequest(&NSURLRequest::requestWithURL(&address));
        }
    });
}

/// WebKit crashes when a page going fullscreen is navigated away or taken out of
/// its window, so `then` runs only once any fullscreen video or element is closed.
pub fn out_of_fullscreen(webview: &WKWebView, then: impl FnOnce(&WKWebView) + 'static) {
    // SAFETY: main thread, where `Tab` runs every call; the selector check covers
    // macOS before 13.3, which has no fullscreen state to ask for.
    let in_fullscreen = webview.respondsToSelector(sel!(fullscreenState))
        && unsafe { webview.fullscreenState() } != WKFullscreenState::NotInFullscreen;
    if !in_fullscreen {
        then(webview);
        return;
    }
    let held = webview.retain();
    let then = Cell::new(Some(then));
    let block = RcBlock::new(move || {
        if let Some(then) = then.take() {
            then(&held);
        }
    });
    // SAFETY: main thread, and WebKit copies the block and calls it there.
    unsafe { webview.closeAllMediaPresentationsWithCompletionHandler(Some(&block)) };
}

/// Under an opaque title bar WebKit shortens the page by however far the view
/// reaches above the window's content area, and a parked tab reaches 100,000
/// points above it, so its page laid out with a negative height. Tabs are
/// placed exactly where they show, so WebKit is told to leave the page whole.
pub fn leave_insets_to_the_app(webview: &WKWebView) {
    let selector = sel!(_setAutomaticallyAdjustsContentInsets:);
    if webview.respondsToSelector(selector) {
        // SAFETY: `respondsToSelector` just confirmed this private method exists; it takes
        // one BOOL and returns nothing.
        let _: () = unsafe { msg_send![webview, _setAutomaticallyAdjustsContentInsets: Bool::NO] };
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

/// With history swipes on, WebKit snapshots the page on every navigation for the
/// swipe to show. A hidden parked tab never swipes, and those snapshots have
/// aborted the app inside QuartzCore when the GPU could not take another.
pub fn swipe_through_history(webview: &WKWebView, allowed: bool) {
    // SAFETY: main thread (where `Tab` runs every call).
    unsafe {
        if webview.allowsBackForwardNavigationGestures() != allowed {
            webview.setAllowsBackForwardNavigationGestures(allowed);
        }
    }
}

pub fn forget(tab_id: &str) {
    let _ = answer_dialog(tab_id, false, None);
    TABS.with(|tabs| {
        if let Some(tab) = tabs.borrow_mut().remove(tab_id) {
            scripts::forget(view_key(&tab.webview));
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
    out_of_fullscreen(webview, move |webview| {
        // SAFETY: main thread (where `Tab` runs every call), and `webview` is alive.
        unsafe {
            if delta < 0 {
                webview.goBack();
            } else {
                webview.goForward();
            }
        }
    });
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
    }
    // SAFETY: as above.
    out_of_fullscreen(&webview, |webview| unsafe {
        let _ = webview.reload();
    });
}

pub fn history_state(webview: &WKWebView) -> (bool, bool) {
    // SAFETY: main thread, where `Tab` runs every call, and `webview` is alive.
    unsafe { (webview.canGoBack(), webview.canGoForward()) }
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
