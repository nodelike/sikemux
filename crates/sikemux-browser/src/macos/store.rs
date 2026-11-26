//! Keeps tabs signed in across isolated per-profile WebKit data stores, as far as
//! WebKit allows. Its tracking prevention clears what sites keep in page storage after
//! a week, which is switched off here. It still clears everything of a host it classes
//! as a tracker, which is not. Cookies only reach disk a few seconds after a site sets
//! them, so a crash in between brings back a session the site has already replaced.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;

use dispatch2::{DispatchQueue, DispatchTime};
use objc2_foundation::{NSDate, NSUserDefaults, NSUUID};
use objc2_web_kit::{WKHTTPCookieStore, WKHTTPCookieStoreObserver, WKWebsiteDataStore};

use crate::ProfileStore;

use super::*;

const SAVE_AFTER: Duration = Duration::from_secs(1);

thread_local! {
    static SAVERS: RefCell<HashMap<ProfileStore, Retained<CookieSaver>>> = RefCell::new(HashMap::new());
}

struct CookieSaverIvars {
    store: ProfileStore,
    pending: Cell<bool>,
}

define_class!(
    /// Writes a profile's cookie jar to disk shortly after any site changes it.
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = CookieSaverIvars]
    struct CookieSaver;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for CookieSaver {}

    // SAFETY: the method has the signature WebKit declares, and WebKit calls it on
    // the main thread.
    unsafe impl WKHTTPCookieStoreObserver for CookieSaver {
        #[unsafe(method(cookiesDidChangeInCookieStore:))]
        fn cookies_did_change(&self, _store: &WKHTTPCookieStore) {
            if self.ivars().pending.replace(true) {
                return;
            }
            let target = self.ivars().store;
            let Ok(when) = DispatchTime::try_from(SAVE_AFTER) else {
                return;
            };
            let _ = DispatchQueue::main().after(when, move || {
                SAVERS.with(|savers| {
                    if let Some(saver) = savers.borrow().get(&target) {
                        saver.ivars().pending.set(false);
                    }
                });
                flush_store(target);
            });
        }
    }
);

pub(super) fn data_store(
    mtm: MainThreadMarker,
    profile: ProfileStore,
) -> Retained<WKWebsiteDataStore> {
    match profile {
        ProfileStore::Default => {
            // SAFETY: main thread, as `mtm` proves.
            unsafe { WKWebsiteDataStore::defaultDataStore(mtm) }
        }
        ProfileStore::Named(uuid) => {
            let class = WKWebsiteDataStore::class();
            // SAFETY: class selector check guards macOS 14+ API on macOS 11+ targets.
            let supported: bool =
                unsafe { msg_send![class, respondsToSelector: sel!(dataStoreForIdentifier:)] };
            if supported {
                let nsuuid = NSUUID::from_bytes(*uuid.as_bytes());
                // SAFETY: main thread and selector availability verified above.
                unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&nsuuid, mtm) }
            } else {
                // SAFETY: main thread, fallback on macOS < 14.
                unsafe { WKWebsiteDataStore::defaultDataStore(mtm) }
            }
        }
    }
}

/// Sets up the store for `profile` once before the first tab uses it, and returns it.
pub(super) fn prepare(
    mtm: MainThreadMarker,
    profile: ProfileStore,
) -> Retained<WKWebsiteDataStore> {
    static DEFAULTS_REGISTERED: Once = Once::new();
    DEFAULTS_REGISTERED.call_once(keep_page_storage);
    let store = data_store(mtm, profile);
    SAVERS.with(|savers| {
        let mut savers = savers.borrow_mut();
        savers.entry(profile).or_insert_with(|| {
            let saver: Retained<CookieSaver> =
                // SAFETY: a fresh CookieSaver, initialised as NSObject documents.
                unsafe {
                    msg_send![
                        super(CookieSaver::alloc(mtm).set_ivars(CookieSaverIvars {
                            store: profile,
                            pending: Cell::new(false),
                        })),
                        init
                    ]
                };
            // SAFETY: main thread. The cookie store holds observers weakly, and SAVERS
            // keeps this one alive while the profile is active.
            unsafe {
                store
                    .httpCookieStore()
                    .addObserver(ProtocolObject::from_ref(&*saver));
            }
            saver
        });
    });
    store
}

/// Tracking prevention stays on, since WebKit drops an embedded frame's `Partitioned`
/// cookies without it. This turns off only its weekly clearing of page storage.
fn keep_page_storage() {
    let key = NSString::from_str("WebKitExperimentalIsFirstPartyWebsiteDataRemovalDisabled");
    let yes = NSNumber::numberWithBool(true);
    let defaults: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&[&*key], &[&*yes]);
    // SAFETY: the dictionary maps a string to an NSNumber, as defaults take. A
    // registered default lasts for this run only.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
}

fn flush_store(profile: ProfileStore) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let store = data_store(mtm, profile);
    // SAFETY: main thread, as `mtm` proves.
    let cookies = unsafe { store.httpCookieStore() };
    if !cookies.respondsToSelector(sel!(_flushCookiesToDiskWithCompletionHandler:)) {
        return;
    }
    let done = RcBlock::new(|| {});
    // SAFETY: WebKit's own call to write cookies out, taking a completion block with
    // no arguments; checked above that this WebKit still has it.
    let _: () = unsafe { msg_send![&*cookies, _flushCookiesToDiskWithCompletionHandler: &*done] };
}

/// Asks WebKit to write every cookie to disk now across all active profile stores.
pub fn save_cookies() {
    let Some(_) = MainThreadMarker::new() else {
        return;
    };
    let mut stores: Vec<ProfileStore> =
        SAVERS.with(|savers| savers.borrow().keys().copied().collect());
    if !stores.contains(&ProfileStore::Default) {
        stores.push(ProfileStore::Default);
    }
    for profile in stores {
        flush_store(profile);
    }
}

/// Clears cookies, localStorage, IndexedDB, and cache for `profile`.
pub fn clear_data(profile: ProfileStore) {
    let run = move || {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let store = data_store(mtm, profile);
        // SAFETY: main thread, as `mtm` proves.
        let types = unsafe { WKWebsiteDataStore::allWebsiteDataTypes(mtm) };
        let since = NSDate::distantPast();
        let done = RcBlock::new(|| {});
        // SAFETY: main thread, valid data types set and completion handler.
        unsafe {
            store.removeDataOfTypes_modifiedSince_completionHandler(&types, &since, &done);
        }
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        DispatchQueue::main().exec_async(run);
    }
}

/// Permanently deletes a named profile's OS data store (or clears data on older macOS).
pub fn remove_data_store(profile: ProfileStore) {
    let run = move || {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        SAVERS.with(|savers| {
            savers.borrow_mut().remove(&profile);
        });
        let ProfileStore::Named(uuid) = profile else {
            clear_data(profile);
            return;
        };
        let class = WKWebsiteDataStore::class();
        // SAFETY: class selector check guards macOS 14+ API on macOS 11+ targets.
        let supported: bool = unsafe {
            msg_send![
                class,
                respondsToSelector: sel!(removeDataStoreForIdentifier:completionHandler:)
            ]
        };
        if supported {
            let nsuuid = NSUUID::from_bytes(*uuid.as_bytes());
            let done = RcBlock::new(|_error: *mut NSError| {});
            // SAFETY: main thread and selector verified above.
            unsafe {
                WKWebsiteDataStore::removeDataStoreForIdentifier_completionHandler(
                    &nsuuid, &done, mtm,
                );
            }
        } else {
            clear_data(profile);
        }
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        DispatchQueue::main().exec_async(run);
    }
}
