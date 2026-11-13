use std::cell::Cell;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventPhase, NSImage, NSView};
use objc2_foundation::{
    NSError, NSNumber, NSObject, NSObjectNSKeyValueCoding, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString, NSURL,
};
use objc2_web_kit::{
    WKContentWorld, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationType, WKScriptMessage, WKScriptMessageHandler, WKSnapshotConfiguration,
    WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView,
    WKWebViewConfiguration, WKWebsiteDataStore,
};

use super::{PageEvent, Room, BOOTSTRAP, PERSON_LINKS, PERSON_LINKS_WORLD, PERSON_LINK_CATCHER};

pub struct PageIvars {
    events: Rc<dyn Fn(PageEvent)>,
    /// The address the page's text is read from.
    loaded: String,
    /// Whether a link the page reports, or goes to, is taken as one the person clicked.
    trusts_page_links: bool,
}

define_class!(
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = PageIvars]
    struct PageMessages;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for PageMessages {}

    // SAFETY: the method has the signature WebKit declares, and WebKit calls it on the
    // main thread with the page's message.
    unsafe impl WKScriptMessageHandler for PageMessages {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn received(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            // SAFETY: main thread, and WebKit keeps `message` alive for the call.
            let (name, body) = unsafe { (message.name().to_string(), message.body()) };
            let Some(text) = body.downcast_ref::<NSString>().map(|text| text.to_string()) else {
                return;
            };
            let event = match name == PERSON_LINKS {
                true => Some(PageEvent::Link(text)),
                false => PageEvent::read(&text).filter(|event| {
                    self.ivars().trusts_page_links || !matches!(event, PageEvent::Link(_))
                }),
            };
            if let Some(event) = event {
                (self.ivars().events)(event);
            }
        }
    }

    // SAFETY: the method has the signature WebKit declares, and WebKit calls it on the
    // main thread with a live webview and action.
    unsafe impl WKNavigationDelegate for PageMessages {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn decide(
            &self,
            _webview: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: main thread, and WebKit keeps `action` alive for the callback.
            let (kind, url) = unsafe {
                (
                    action.navigationType(),
                    action
                        .request()
                        .URL()
                        .and_then(|url| url.absoluteString())
                        .map(|url| url.to_string())
                        .unwrap_or_default(),
                )
            };
            let stays = super::stays(&url, &self.ivars().loaded, kind == WKNavigationType::Other);
            if self.ivars().trusts_page_links
                && kind == WKNavigationType::LinkActivated
                && url.starts_with("http")
            {
                (self.ivars().events)(PageEvent::Link(url));
            }
            handler.call((if stays {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
    }
);

pub struct WebIvars {
    /// Whether the page fills its room and scrolls itself.
    fills: Cell<bool>,
    /// Whether a click on the page takes the keyboard.
    takes_keys: bool,
    /// Where an app's Escape is told, which the page may also use.
    events: Option<Rc<dyn Fn(PageEvent)>>,
    /// Whether the scroll under way goes up or down.
    vertical: Cell<bool>,
}

define_class!(
    /// A page drawn at its full height never scrolls up or down itself: those scrolls go
    /// straight on to the app, and only sideways ones and pinches stay with WebKit.
    // SAFETY: WKWebView subclassed with plain ivars and no dealloc, used on the main thread.
    #[unsafe(super(WKWebView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = WebIvars]
    struct PageWebView;

    impl PageWebView {
        // SAFETY: matches NSResponder's `scrollWheel:`: an event in, nothing out.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let (phase, momentum) = (event.phase(), event.momentumPhase());
            if phase.contains(NSEventPhase::Began) || (phase.is_empty() && momentum.is_empty()) {
                self.ivars().vertical.set(is_vertical(event));
            }
            // SAFETY: main thread, where AppKit delivers scrolls; the next responder holds the page.
            let next = (!self.ivars().fills.get() && self.ivars().vertical.get())
                .then(|| unsafe { self.nextResponder() })
                .flatten();
            match next {
                Some(next) => next.scrollWheel(event),
                // SAFETY: hands the event on to WKWebView's own handler, on the main thread.
                None => unsafe { msg_send![super(self), scrollWheel: event] },
            }
        }

        /// A page in a reply is looked at, not typed into: a click on it leaves the keyboard
        /// with the composer, where keys the page has no use for would otherwise beep.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            self.ivars().takes_keys
        }

        // SAFETY: matches NSResponder's `keyDown:`: an event in, nothing out.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if event.keyCode() == ESCAPE_KEY {
                if let Some(events) = &self.ivars().events {
                    events(PageEvent::Escape);
                }
            }
            // SAFETY: hands the event on to WKWebView's own handler, on the main thread.
            unsafe { msg_send![super(self), keyDown: event] }
        }
    }
);

/// Turns off WebRTC, whose connections no Content-Security-Policy governs, by WebKit's own
/// private switch, as it offers no public one.
fn close_peer_connections(configuration: &WKWebViewConfiguration) {
    // SAFETY: main thread, as the configuration is only made there; the setter is sent only
    // when WebKit has it, and takes a BOOL.
    unsafe {
        let preferences = configuration.preferences();
        let setter = sel!(_setPeerConnectionEnabled:);
        if preferences.respondsToSelector(setter) {
            let _: () = msg_send![&*preferences, _setPeerConnectionEnabled: false];
        }
    }
}

/// A still as Core Graphics holds it, which is never changed once made, so any thread may read it.
struct Picture(Retained<objc2_core_graphics::CGImage>);

// SAFETY: a CGImage is immutable, and Core Graphics lets any thread read one.
unsafe impl Send for Picture {}

/// The Escape key's code on a Mac keyboard.
const ESCAPE_KEY: u16 = 53;

/// Whether a scroll that starts with `event` goes up or down rather than sideways or to zoom.
fn is_vertical(event: &NSEvent) -> bool {
    !event
        .modifierFlags()
        .contains(NSEventModifierFlags::Control)
        && event.scrollingDeltaY().abs() >= event.scrollingDeltaX().abs()
}

/// The page's view, which the app places, and the handler its events go through.
pub struct PageView {
    view: Retained<PageWebView>,
    base: Option<Retained<NSURL>>,
    _messages: Retained<PageMessages>,
}

impl PageView {
    /// Shows `html` in `room`, and tells `events` what the page's script reports. A reply's
    /// page has already been given the app's theme.
    pub fn new(
        mtm: MainThreadMarker,
        html: &str,
        room: Room,
        events: impl Fn(PageEvent) + 'static,
    ) -> Self {
        let events: Rc<dyn Fn(PageEvent)> = Rc::new(events);
        let (fills, clear, base) = match room {
            Room::Grows | Room::App => (false, true, None),
            Room::Panel => (true, true, None),
            Room::Fills(path) => (
                true,
                false,
                path.map(|path| {
                    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
                }),
            ),
        };
        let loaded = base
            .as_ref()
            .and_then(|base| base.absoluteString())
            .map(|base| base.to_string());
        let messages = mtm.alloc::<PageMessages>().set_ivars(PageIvars {
            events: events.clone(),
            loaded: super::loads_as(loaded.as_deref()).to_owned(),
            trusts_page_links: room.trusts_page_links(),
        });
        // SAFETY: NSObject's `init` takes nothing and returns the object it was sent to.
        let messages: Retained<PageMessages> = unsafe { msg_send![super(messages), init] };
        // SAFETY: main thread; the configuration and its parts are fresh and only ours,
        // and every value set is one WebKit reads with that type.
        let view = unsafe {
            let configuration = WKWebViewConfiguration::new(mtm);
            configuration.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let controller = configuration.userContentController();
            if !fills {
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &NSString::from_str(BOOTSTRAP),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                );
                controller.addUserScript(&script);
            }
            controller.addScriptMessageHandler_name(
                ProtocolObject::from_ref(&*messages),
                &NSString::from_str("sikemux"),
            );
            if !room.trusts_page_links() {
                let world =
                    WKContentWorld::worldWithName(&NSString::from_str(PERSON_LINKS_WORLD), mtm);
                let script =
                    WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                        WKUserScript::alloc(mtm),
                        &NSString::from_str(PERSON_LINK_CATCHER),
                        WKUserScriptInjectionTime::AtDocumentStart,
                        true,
                        &world,
                    );
                controller.addUserScript(&script);
                controller.addScriptMessageHandler_contentWorld_name(
                    ProtocolObject::from_ref(&*messages),
                    &world,
                    &NSString::from_str(PERSON_LINKS),
                );
                close_peer_connections(&configuration);
            }
            let frame = NSRect::new(NSPoint::ZERO, NSSize::new(1.0, 1.0));
            let view: Retained<PageWebView> = msg_send![
                super(PageWebView::alloc(mtm).set_ivars(WebIvars {
                    fills: Cell::new(fills),
                    takes_keys: fills || matches!(room, Room::App),
                    events: matches!(room, Room::App).then(|| events.clone()),
                    vertical: Cell::new(false),
                })),
                initWithFrame: frame,
                configuration: &*configuration
            ];
            view.unregisterDraggedTypes();
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*messages)));
            if clear {
                let no = NSNumber::numberWithBool(false);
                view.setValue_forKey(Some(&no), &NSString::from_str("drawsBackground"));
            }
            view
        };
        let page = Self {
            view,
            base,
            _messages: messages,
        };
        page.show(html);
        page
    }

    pub fn show(&self, html: &str) {
        // SAFETY: main thread, as the view is only made there.
        unsafe {
            self.view
                .loadHTMLString_baseURL(&NSString::from_str(html), self.base.as_deref())
        };
    }

    pub fn view(&self) -> Retained<NSView> {
        Retained::into_super(Retained::into_super(self.view.clone()))
    }

    /// Runs `script` in the page, as when the window's theme changes under it.
    pub fn run_script(&self, script: &str) {
        // SAFETY: main thread, as the view is only made there, and no answer is wanted.
        unsafe {
            self.view
                .evaluateJavaScript_completionHandler(&NSString::from_str(script), None)
        };
    }
}

impl PageView {
    /// Lets the page fill its room and scroll itself, as an app put over its whole chat does,
    /// or puts it back to scrolling with what holds it.
    pub fn fill(&self, fills: bool) {
        self.view.ivars().fills.set(fills);
    }

    /// A still of what the view shows now, as sharp as the screen draws it, as a JPEG made off
    /// the main thread. The page must paint its own ground, as a JPEG keeps no see-through.
    pub fn still(&self, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        let Some(mtm) = MainThreadMarker::new() else {
            done(None);
            return;
        };
        // SAFETY: main thread, as `mtm` proves; the configuration is ours until handed over.
        let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
        // SAFETY: main thread, and `configuration` is not yet handed to WebKit.
        unsafe {
            configuration.setSnapshotWidth(Some(&NSNumber::numberWithDouble(
                self.view.bounds().size.width,
            )));
            configuration.setAfterScreenUpdates(false);
        }
        let done = std::sync::Mutex::new(Some(done));
        let block = RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
            let Some(done) = done.lock().ok().and_then(|mut done| done.take()) else {
                return;
            };
            // SAFETY: WebKit passes a live image or nil; `retain` takes our own reference.
            let picture = unsafe { Retained::retain(image) }
                // SAFETY: a null rect and no context or hints ask for the image at its own size.
                .and_then(|image| unsafe {
                    image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
                })
                .map(Picture);
            // Encoding takes milliseconds, and this block runs on the thread that draws the
            // window.
            match picture {
                Some(picture) => {
                    std::thread::spawn(move || done(crate::macos::picture_jpeg(&picture.0)));
                }
                None => done(None),
            }
        });
        // SAFETY: main thread; WebKit copies the block and holds `configuration` for the call.
        unsafe {
            self.view
                .takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block)
        };
    }
}

impl Drop for PageView {
    fn drop(&mut self) {
        // SAFETY: main thread; the controller holds its handler strongly, so it lets go here.
        unsafe {
            self.view
                .configuration()
                .userContentController()
                .removeAllScriptMessageHandlers();
        }
    }
}
