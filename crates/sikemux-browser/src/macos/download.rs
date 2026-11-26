//! Authenticated read-only GET asset downloads through WKWebView's native WKDownload API.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSObject, NSObjectProtocol, NSString, NSURLRequest,
    NSURLResponse, NSURL,
};
use objc2_web_kit::{WKDownload, WKDownloadDelegate, WKDownloadRedirectPolicy, WKWebView};
use url::Url;

use super::delegates::load_error;
use crate::downloads::{contained_download_path, quarantine, DownloadedAsset};

type DownloadDone = Box<dyn FnOnce(Result<DownloadedAsset, String>) + Send>;

thread_local! {
    static ACTIVE_DOWNLOADS: RefCell<HashMap<usize, Retained<AssetDownloadDelegate>>> =
        RefCell::new(HashMap::new());
}

struct AssetDownloadIvars {
    dir: PathBuf,
    url: Url,
    filename: Option<String>,
    max_bytes: u64,
    dest: RefCell<Option<PathBuf>>,
    mime_type: RefCell<String>,
    done: RefCell<Option<DownloadDone>>,
}

define_class!(
    // SAFETY: plain NSObject subclass with no custom dealloc, main-thread only.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AssetDownloadIvars]
    struct AssetDownloadDelegate;

    // SAFETY: every NSObject subclass conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for AssetDownloadDelegate {}

    // SAFETY: each method matches WKDownloadDelegate's Objective-C signature and runs on the main thread.
    unsafe impl WKDownloadDelegate for AssetDownloadDelegate {
        #[unsafe(method(download:willPerformHTTPRedirection:newRequest:decisionHandler:))]
        unsafe fn redirect(
            &self,
            _download: &WKDownload,
            _response: &NSHTTPURLResponse,
            request: &NSURLRequest,
            handler: &block2::DynBlock<dyn Fn(WKDownloadRedirectPolicy)>,
        ) {
            let url = request
                .URL()
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let method = request
                .HTTPMethod()
                .map(|m| m.to_string())
                .unwrap_or_default();
            let get_only = method.is_empty()
                || method.eq_ignore_ascii_case("GET")
                || method.eq_ignore_ascii_case("HEAD");
            let safe_scheme =
                Url::parse(&url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"));
            let policy = if get_only && safe_scheme {
                WKDownloadRedirectPolicy::Allow
            } else {
                self.finish(Err(
                    "download redirect refused (non-GET or disallowed scheme)".into(),
                ));
                WKDownloadRedirectPolicy::Cancel
            };
            handler.call((policy,));
        }

        #[unsafe(method(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:))]
        unsafe fn destination(
            &self,
            _download: &WKDownload,
            response: &NSURLResponse,
            suggested: &NSString,
            handler: &block2::DynBlock<dyn Fn(*mut NSURL)>,
        ) {
            if let Some(http) = response.downcast_ref::<NSHTTPURLResponse>() {
                let status = http.statusCode();
                if status >= 400 {
                    self.finish(Err(format!("download failed with HTTP {status}")));
                    handler.call((std::ptr::null_mut(),));
                    return;
                }
            }
            let expected = response.expectedContentLength();
            if expected > 0 && (expected as u64) > self.ivars().max_bytes {
                self.finish(Err(format!(
                    "download exceeds {} byte size limit",
                    self.ivars().max_bytes
                )));
                handler.call((std::ptr::null_mut(),));
                return;
            }
            let mime = response
                .MIMEType()
                .map(|m| m.to_string())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "application/octet-stream".into());
            *self.ivars().mime_type.borrow_mut() = mime;

            let suggested_path = PathBuf::from(suggested.to_string());
            match contained_download_path(
                &self.ivars().dir,
                &self.ivars().url,
                self.ivars().filename.as_deref(),
                &suggested_path,
            ) {
                Ok(dest) => {
                    *self.ivars().dest.borrow_mut() = Some(dest.clone());
                    let file_url = NSURL::fileURLWithPath_isDirectory(
                        &NSString::from_str(&dest.to_string_lossy()),
                        false,
                    );
                    handler.call((Retained::as_ptr(&file_url) as *mut NSURL,));
                }
                Err(error) => {
                    self.finish(Err(error));
                    handler.call((std::ptr::null_mut(),));
                }
            }
        }

        #[unsafe(method(downloadDidFinish:))]
        unsafe fn finished(&self, download: &WKDownload) {
            let key = download as *const WKDownload as usize;
            let _held = ACTIVE_DOWNLOADS.with(|map| map.borrow_mut().remove(&key));
            let Some(dest) = self.ivars().dest.borrow().clone() else {
                self.finish(Err("download finished without a destination file".into()));
                return;
            };
            let bytes = match std::fs::metadata(&dest) {
                Ok(meta) => meta.len(),
                Err(error) => {
                    self.finish(Err(format!("could not read downloaded file: {error}")));
                    return;
                }
            };
            if bytes > self.ivars().max_bytes {
                let _ = std::fs::remove_file(&dest);
                self.finish(Err(format!(
                    "download exceeds {} byte size limit",
                    self.ivars().max_bytes
                )));
                return;
            }
            quarantine(&dest);
            let mime_type = self.ivars().mime_type.borrow().clone();
            self.finish(Ok(DownloadedAsset {
                path: dest,
                mime_type,
                bytes,
            }));
        }

        #[unsafe(method(download:didFailWithError:resumeData:))]
        unsafe fn failed(&self, download: &WKDownload, error: &NSError, _resume: Option<&NSData>) {
            let key = download as *const WKDownload as usize;
            let _held = ACTIVE_DOWNLOADS.with(|map| map.borrow_mut().remove(&key));
            if let Some(dest) = self.ivars().dest.borrow().as_deref() {
                let _ = std::fs::remove_file(dest);
            }
            self.finish(Err(format!("download failed: {}", load_error(error))));
        }
    }
);

impl AssetDownloadDelegate {
    fn new(
        mtm: MainThreadMarker,
        dir: PathBuf,
        url: Url,
        filename: Option<String>,
        max_bytes: u64,
        done: DownloadDone,
    ) -> Retained<Self> {
        let delegate = mtm
            .alloc::<AssetDownloadDelegate>()
            .set_ivars(AssetDownloadIvars {
                dir,
                url,
                filename,
                max_bytes,
                dest: RefCell::new(None),
                mime_type: RefCell::new("application/octet-stream".into()),
                done: RefCell::new(Some(done)),
            });
        // SAFETY: freshly allocated with ivars set; NSObject `init` returns the same object.
        unsafe { msg_send![super(delegate), init] }
    }

    fn finish(&self, result: Result<DownloadedAsset, String>) {
        if let Some(done) = self.ivars().done.borrow_mut().take() {
            done(result);
        }
    }
}

pub(crate) fn download_asset(
    webview: &WKWebView,
    url: &str,
    dir: PathBuf,
    filename: Option<String>,
    max_bytes: u64,
    done: DownloadDone,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        done(Err("download must start on the main thread".into()));
        return;
    };
    let Ok(parsed) = Url::parse(url) else {
        done(Err("invalid download url".into()));
        return;
    };
    let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        done(Err("invalid download url".into()));
        return;
    };
    let request = NSURLRequest::requestWithURL(&ns_url);
    let delegate = AssetDownloadDelegate::new(mtm, dir, parsed, filename, max_bytes, done);
    let held = delegate.clone();
    let block = RcBlock::new(move |download: NonNull<WKDownload>| {
        // SAFETY: WebKit invokes this block on the main thread with a live WKDownload.
        let download_ref = unsafe { download.as_ref() };
        let key = download.as_ptr() as usize;
        ACTIVE_DOWNLOADS.with(|map| {
            map.borrow_mut().insert(key, held.clone());
        });
        // SAFETY: main thread; `ACTIVE_DOWNLOADS` keeps `held` alive until finish/fail.
        unsafe {
            download_ref.setDelegate(Some(ProtocolObject::from_ref(&*held)));
        }
    });
    // SAFETY: main thread, `webview` and `request` are live, WebKit copies `block`.
    unsafe {
        webview.startDownloadUsingRequest_completionHandler(&request, &block);
    }
}
