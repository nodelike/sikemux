//! Keeps tabs signed in. WebKit's tracking prevention treats sign-in hosts as
//! trackers and clears what sites keep in page storage after a week, and cookies
//! only reach disk a few seconds after a site sets them, so a crash in between
//! brings back a session the site has already replaced.

use std::cell::{Cell, OnceCell};
use std::time::Duration;

use dispatch2::{DispatchQueue, DispatchTime};
use objc2_web_kit::{WKHTTPCookieStore, WKHTTPCookieStoreObserver, WKWebsiteDataStore};

use super::*;

const SAVE_AFTER: Duration = Duration::from_secs(1);

thread_local! {
    static SAVER: OnceCell<Retained<CookieSaver>> = const { OnceCell::new() };
}

define_class!(
    /// Writes the cookie jar to disk shortly after any site changes it.
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Cell<bool>]
    struct CookieSaver;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for CookieSaver {}

    // SAFETY: the method has the signature WebKit declares, and WebKit calls it on
    // the main thread.
    unsafe impl WKHTTPCookieStoreObserver for CookieSaver {
        #[unsafe(method(cookiesDidChangeInCookieStore:))]
        fn cookies_did_change(&self, _store: &WKHTTPCookieStore) {
            if self.ivars().replace(true) {
                return;
            }
            let Ok(when) = DispatchTime::try_from(SAVE_AFTER) else {
                return;
            };
            let _ = DispatchQueue::main().after(when, || {
                SAVER.with(|saver| {
                    if let Some(saver) = saver.get() {
                        saver.ivars().set(false);
                    }
                });
                save_cookies();
            });
        }
    }
);

/// Sets up the store every signed-in tab shares, once, before the first tab uses it.
pub(super) fn prepare(mtm: MainThreadMarker) {
    SAVER.with(|saver| {
        saver.get_or_init(|| {
            // SAFETY: main thread, as `mtm` proves.
            let store = unsafe { WKWebsiteDataStore::defaultDataStore(mtm) };
            if store.respondsToSelector(sel!(_setResourceLoadStatisticsEnabled:)) {
                // SAFETY: WebKit's own switch for tracking prevention, taking a BOOL;
                // checked above that this WebKit still has it.
                let _: () = unsafe {
                    msg_send![&*store, _setResourceLoadStatisticsEnabled: Bool::NO]
                };
            }
            let saver: Retained<CookieSaver> =
                // SAFETY: a fresh CookieSaver, initialised as NSObject documents.
                unsafe { msg_send![super(CookieSaver::alloc(mtm).set_ivars(Cell::new(false))), init] };
            // SAFETY: main thread. The cookie store holds observers weakly, and SAVER
            // keeps this one for the life of the app.
            unsafe {
                store
                    .httpCookieStore()
                    .addObserver(ProtocolObject::from_ref(&*saver));
            }
            saver
        });
    });
}

/// Asks WebKit to write every cookie to disk now. It goes on without waiting, so it
/// is safe while the app quits: the request reaches WebKit before the app is gone.
pub fn save_cookies() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: main thread, as `mtm` proves.
    let cookies = unsafe { WKWebsiteDataStore::defaultDataStore(mtm).httpCookieStore() };
    if !cookies.respondsToSelector(sel!(_flushCookiesToDiskWithCompletionHandler:)) {
        return;
    }
    let done = RcBlock::new(|| {});
    // SAFETY: WebKit's own call to write cookies out, taking a completion block with
    // no arguments; checked above that this WebKit still has it.
    let _: () = unsafe { msg_send![&*cookies, _flushCookiesToDiskWithCompletionHandler: &*done] };
}
