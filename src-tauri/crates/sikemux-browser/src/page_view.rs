//! A page an agent showed, drawn inline in its chat reply: a web view of its
//! own that keeps no storage and never navigates. Its script can only report
//! the page's height, a link the person clicked and a scroll the page had no
//! use for; the app decides what each means.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSView;
use objc2_foundation::{
    NSNumber, NSObject, NSObjectNSKeyValueCoding, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString,
};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType,
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};

/// Sizes the view to the page and hands over the links and scrolls the person meant.
const BOOTSTRAP: &str = r#"(()=>{const post=(message)=>window.webkit.messageHandlers.sikemux.postMessage(JSON.stringify(message));const root=document.documentElement;root.setAttribute("data-sikemux-framed","");document.addEventListener("click",(event)=>{const link=event.isTrusted&&event.composedPath().find((target)=>target instanceof Element&&target.matches("a[href]"));if(!link)return;let url;try{url=new URL(link.getAttribute("href"),document.baseURI);}catch{return;}if(!/^https?:$/.test(url.protocol))return;event.preventDefault();post({kind:"link",url:url.href});},true);let reported=0;const report=()=>{const height=Math.ceil(root.getBoundingClientRect().height);if(height===reported)return;reported=height;post({kind:"height",height});};const observer=new ResizeObserver(report);observer.observe(root);document.addEventListener("DOMContentLoaded",()=>{if(document.body)observer.observe(document.body);report();});window.addEventListener("load",report);const scrollsItself=(node,down)=>{for(;node instanceof Element;node=node.parentElement){const style=getComputedStyle(node).overflowY;const scroller=(style==="auto"||style==="scroll")&&node!==document.body&&node!==root;if(scroller&&(down?node.scrollTop+node.clientHeight<node.scrollHeight-1:node.scrollTop>0))return true;}return false;};window.addEventListener("wheel",(event)=>{if(event.ctrlKey||Math.abs(event.deltaY)<=Math.abs(event.deltaX)||scrollsItself(event.target,event.deltaY>0))return;event.preventDefault();const scale=event.deltaMode===1?16:event.deltaMode===2?window.innerHeight:1;post({kind:"wheel",deltaY:event.deltaY*scale});},{passive:false});})();"#;

/// What a page's script reported.
pub enum PageEvent {
    Height(f64),
    /// A link the person clicked, which the page itself never opens.
    Link(String),
    /// A vertical scroll the page did not use, for whatever holds the page to scroll by.
    Wheel(f64),
}

pub struct PageIvars {
    events: Box<dyn Fn(PageEvent)>,
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
            let body = unsafe { message.body() };
            let Some(text) = body.downcast_ref::<NSString>().map(|text| text.to_string()) else {
                return;
            };
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&text) else {
                return;
            };
            let event = match message["kind"].as_str() {
                Some("height") => message["height"].as_f64().map(PageEvent::Height),
                Some("link") => message["url"]
                    .as_str()
                    .map(|url| PageEvent::Link(url.to_owned())),
                Some("wheel") => message["deltaY"].as_f64().map(PageEvent::Wheel),
                _ => None,
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
            let first_load = kind == WKNavigationType::Other && url == "about:blank";
            if kind == WKNavigationType::LinkActivated && url.starts_with("http") {
                (self.ivars().events)(PageEvent::Link(url));
            }
            handler.call((if first_load {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
    }
);

/// The page's view, which the app places, and the handler its events go through.
pub struct PageView {
    view: Retained<WKWebView>,
    _messages: Retained<PageMessages>,
}

impl PageView {
    /// Shows `html`, which the app has already given its theme, and tells `events` what
    /// the page's script reports.
    pub fn new(mtm: MainThreadMarker, html: &str, events: impl Fn(PageEvent) + 'static) -> Self {
        let messages = mtm.alloc::<PageMessages>().set_ivars(PageIvars {
            events: Box::new(events),
        });
        // SAFETY: NSObject's `init` takes nothing and returns the object it was sent to.
        let messages: Retained<PageMessages> = unsafe { msg_send![super(messages), init] };
        // SAFETY: main thread; the configuration and its parts are fresh and only ours,
        // and every value set is one WebKit reads with that type.
        let view = unsafe {
            let configuration = WKWebViewConfiguration::new(mtm);
            configuration.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let controller = configuration.userContentController();
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(BOOTSTRAP),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            );
            controller.addUserScript(&script);
            controller.addScriptMessageHandler_name(
                ProtocolObject::from_ref(&*messages),
                &NSString::from_str("sikemux"),
            );
            let frame = NSRect::new(NSPoint::ZERO, NSSize::new(1.0, 1.0));
            let view = WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                frame,
                &configuration,
            );
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*messages)));
            let no = NSNumber::numberWithBool(false);
            view.setValue_forKey(Some(&no), &NSString::from_str("drawsBackground"));
            view.loadHTMLString_baseURL(&NSString::from_str(html), None);
            view
        };
        Self {
            view,
            _messages: messages,
        }
    }

    pub fn view(&self) -> Retained<NSView> {
        Retained::into_super(self.view.clone())
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

impl Drop for PageView {
    fn drop(&mut self) {
        // SAFETY: main thread; the controller holds its handler strongly, so it lets go here.
        unsafe {
            self.view
                .configuration()
                .userContentController()
                .removeScriptMessageHandlerForName(&NSString::from_str("sikemux"));
        }
    }
}
