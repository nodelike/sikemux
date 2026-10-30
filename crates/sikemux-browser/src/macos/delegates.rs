//! What WebKit tells a tab as it navigates and downloads.

use super::*;

pub(super) struct TabNavigationDelegateIvars {
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
    pub(super) struct TabNavigationDelegate;

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
    pub(super) fn new(
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

pub(super) struct TabDownloadDelegateIvars {
    events: Arc<dyn TabEvents>,
}

define_class!(
    /// Asks where each file a page saves goes, and says when it is done.
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TabDownloadDelegateIvars]
    pub(super) struct TabDownloadDelegate;

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
    pub(super) fn new(mtm: MainThreadMarker, events: Arc<dyn TabEvents>) -> Retained<Self> {
        let delegate = mtm
            .alloc::<TabDownloadDelegate>()
            .set_ivars(TabDownloadDelegateIvars { events });
        // SAFETY: `delegate` is freshly allocated with its ivars set, and NSObject's `init`
        // takes nothing and returns that same object.
        unsafe { msg_send![super(delegate), init] }
    }
}

pub(super) fn download_address(download: &WKDownload) -> String {
    // SAFETY: main thread, and WebKit keeps `download` alive for the callback.
    unsafe { download.originalRequest() }
        .and_then(|request| request.URL())
        .and_then(|url| url.absoluteString())
        .map(|url| url.to_string())
        .unwrap_or_default()
}

pub(super) fn address_of(webview: &WKWebView) -> String {
    // SAFETY: main thread, and WebKit keeps `webview` alive for the callback.
    unsafe { webview.URL() }
        .and_then(|url| url.absoluteString())
        .map(|url| url.to_string())
        .unwrap_or_default()
}

pub(super) fn page_load(webview: &WKWebView, started: bool) -> PageLoad {
    let (can_go_back, can_go_forward) = history_state(webview);
    PageLoad {
        started,
        url: address_of(webview),
        can_go_back,
        can_go_forward,
    }
}

pub(super) fn navigation_id(navigation: Option<&WKNavigation>) -> usize {
    navigation.map_or(0, |navigation| navigation as *const WKNavigation as usize)
}

/// WebKit's description of why a load failed, with its code, since the same
/// words cover several causes. A load stopped for another is only "cancelled".
pub(super) fn load_error(error: &NSError) -> String {
    let (domain, code) = (error.domain().to_string(), error.code());
    if domain == "NSURLErrorDomain" && code == -999 {
        return "cancelled".into();
    }
    format!("{} ({domain} {code})", error.localizedDescription())
}
