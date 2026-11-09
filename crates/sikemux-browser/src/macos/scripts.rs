//! Scripts a tab runs in its page, and how their answers come back. WebKit
//! crashed the app on macOS 26 while describing a failed script to a completion
//! handler, so no script is given one: each posts its answer to a message
//! handler under an id of its own, and a script that never answers is left to
//! its caller's time limit.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};
use objc2_web_kit::{
    WKContentWorld, WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
};

use super::{view_key, World};

const HANDLER: &str = "sikemuxAnswer";

/// A function posting `[id, value as JSON]` to the answer handler, or `[id]` when the
/// value has no JSON form. WebKit drops a message holding anything else without a word.
const POST: &str = "((handler, stringify) => { const post = handler.postMessage.bind(handler); \
     return (id, value) => { let json; try { json = stringify(value); } catch {} \
     post(typeof json === \"string\" ? [id, json] : [id]); }; })\
     (window.webkit.messageHandlers.sikemuxAnswer, JSON.stringify)";

/// The page's own scripts can replace anything a later script would reach
/// for, so each document gets the poster before they run, on a global they
/// can neither change nor shadow.
const PAGE_POSTER: &str = "__sikemuxAnswer";

thread_local! {
    static ANSWERS: Retained<Answers> = Answers::new(
        MainThreadMarker::new().expect("tab scripts run on the main thread"),
    );
    static WAITING: RefCell<HashMap<String, Waiting>> = RefCell::new(HashMap::new());
    /// WebKit tears a named world down once nothing holds it, and the element
    /// numbers the agent was given go with it.
    static HELPER_WORLD: RefCell<Option<Retained<WKContentWorld>>> = const { RefCell::new(None) };
}

/// What to do with a script's value as JSON, or with nothing when it had none to send.
type Answered = Box<dyn FnOnce(Option<String>)>;

struct Waiting {
    view: usize,
    answered: Answered,
}

define_class!(
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct Answers;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for Answers {}

    // SAFETY: the method has the signature WebKit declares, and WebKit calls it on the
    // main thread with the script's message.
    unsafe impl WKScriptMessageHandler for Answers {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn received(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            // SAFETY: main thread, and WebKit keeps `message` alive for the call.
            let body = unsafe { message.body() };
            let Some(parts) = body.downcast_ref::<NSArray>() else {
                return;
            };
            let Some(id) = (parts.count() > 0)
                .then(|| parts.objectAtIndex(0))
                .and_then(|id| id.downcast::<NSString>().ok())
            else {
                return;
            };
            let json = (parts.count() > 1)
                .then(|| parts.objectAtIndex(1))
                .and_then(|json| json.downcast::<NSString>().ok())
                .map(|json| json.to_string());
            let waiting = WAITING.with(|waiting| waiting.borrow_mut().remove(&id.to_string()));
            if let Some(waiting) = waiting {
                (waiting.answered)(json);
            }
        }
    }
);

impl Answers {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let answers = mtm.alloc::<Answers>().set_ivars(());
        // SAFETY: NSObject's `init` takes nothing and returns the object it was sent to.
        unsafe { msg_send![super(answers), init] }
    }
}

fn helper_world(mtm: MainThreadMarker) -> Retained<WKContentWorld> {
    HELPER_WORLD.with(|slot| {
        slot.borrow_mut()
            // SAFETY: main thread, as `mtm` proves; HELPER_WORLD keeps the world alive.
            .get_or_insert_with(|| unsafe {
                WKContentWorld::worldWithName(&NSString::from_str("sikemux"), mtm)
            })
            .clone()
    })
}

fn content_world(world: World, mtm: MainThreadMarker) -> Retained<WKContentWorld> {
    match world {
        // SAFETY: main thread, as `mtm` proves.
        World::Page => unsafe { WKContentWorld::pageWorld(mtm) },
        World::Helper => helper_world(mtm),
    }
}

/// Lets scripts in pages made with `configuration` answer, from either world. A page
/// another page opens is made with a copy that shares this one's handlers.
pub(super) fn listen(configuration: &WKWebViewConfiguration, mtm: MainThreadMarker) {
    let source = format!("Object.defineProperty(window, \"{PAGE_POSTER}\", {{ value: {POST} }});");
    // SAFETY: main thread, as `mtm` proves; the configuration is fresh and only ours, and
    // ANSWERS lives as long as the thread, so the controller's strong hold on it is safe.
    unsafe {
        let controller = configuration.userContentController();
        let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            WKUserScript::alloc(mtm),
            &NSString::from_str(&source),
            WKUserScriptInjectionTime::AtDocumentStart,
            true,
        );
        controller.addUserScript(&script);
        ANSWERS.with(|answers| {
            for world in [World::Page, World::Helper] {
                controller.addScriptMessageHandler_contentWorld_name(
                    ProtocolObject::from_ref(&**answers),
                    &content_world(world, mtm),
                    &NSString::from_str(HANDLER),
                );
            }
        });
    }
}

/// Starts in `world` of the page's main frame the script `source` makes from a poster
/// and an id, which it must answer by calling the poster with that id.
fn start(
    webview: &WKWebView,
    world: World,
    source: impl FnOnce(&str, &str) -> String,
    answered: Answered,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let id = uuid::Uuid::new_v4().simple().to_string();
    let poster = match world {
        World::Page => format!("(typeof {PAGE_POSTER} === \"function\" ? {PAGE_POSTER} : {POST})"),
        World::Helper => POST.to_owned(),
    };
    let source = source(&poster, &id);
    WAITING.with(|waiting| {
        waiting.borrow_mut().insert(
            id,
            Waiting {
                view: view_key(webview),
                answered,
            },
        )
    });
    // SAFETY: main thread, as `mtm` proves, and `webview` and the world are alive.
    unsafe {
        webview.evaluateJavaScript_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&source),
            None,
            &content_world(world, mtm),
            None,
        );
    }
}

/// Lets go of the answers a closed tab's scripts still owed.
pub(super) fn forget(view: usize) {
    WAITING.with(|waiting| waiting.borrow_mut().retain(|_, entry| entry.view != view));
}

/// Runs the expression `script` in the page's own world and hands back its value as
/// JSON, or nothing when it has none or throws.
pub fn evaluate(webview: &WKWebView, script: &str, done: Box<dyn FnOnce(String) + Send>) {
    let source = |post: &str, id: &str| {
        format!(
            "(() => {{ let value; try {{ value = (\n{script}\n); }} catch {{}} \
             {post}(\"{id}\", value); }})();"
        )
    };
    let answered = Box::new(move |json: Option<String>| done(json.unwrap_or_default()));
    start(webview, World::Page, source, answered);
}

/// Runs `body` as the body of an async function in the page, awaiting any
/// promise it returns. The body must return a string; what it throws comes
/// back as the error.
pub fn call_async(
    webview: &WKWebView,
    body: &str,
    world: World,
    done: Box<dyn FnOnce(Result<String, String>) + Send>,
) {
    let source = |post: &str, id: &str| {
        format!(
            "(async () => {{ let answer; \
             try {{ const value = await (async () => {{\n{body}\n}})(); \
             answer = typeof value === \"string\" ? \"o\" + value : \"n\"; }} \
             catch (error) {{ try {{ answer = \"e\" + String((error && error.message) || error); }} \
             catch {{ answer = \"ethe script threw\"; }} }} \
             {post}(\"{id}\", answer); }})();"
        )
    };
    let answered = Box::new(move |json: Option<String>| {
        let answer = json
            .and_then(|json| serde_json::from_str::<String>(&json).ok())
            .unwrap_or_default();
        done(match answer.split_at_checked(1) {
            Some(("o", text)) => Ok(text.to_owned()),
            Some(("e", message)) => Err(message.to_owned()),
            _ => Err("the script returned nothing readable".into()),
        });
    });
    start(webview, world, source, answered);
}
