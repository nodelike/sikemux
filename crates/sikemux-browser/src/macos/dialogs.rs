//! Page dialogs held in Rust without OS modal popups, answered by the agent or inline tab UI.

use super::*;

pub(super) struct TabUiDelegateIvars {
    tab_id: String,
    dialogs: RefCell<Burst>,
    events: Arc<dyn TabEvents>,
}

/// WebKit aborts the process if a JS dialog completion handler is dropped unanswered, so
/// `Drop` always resolves it with Cancel if the tab closes or another dialog replaces it.
struct PendingDialog {
    answer: Option<Box<dyn FnOnce(bool, String)>>,
    events: Arc<dyn TabEvents>,
    default_text: Option<String>,
}

impl PendingDialog {
    fn resolve(mut self, accept: bool, text: Option<&str>) {
        if let Some(answer) = self.answer.take() {
            let reply = match (accept, text, self.default_text.take()) {
                (true, Some(text), _) => text.to_owned(),
                (true, None, Some(default)) => default,
                _ => String::new(),
            };
            self.events.dialog(None);
            answer(accept, reply);
        }
    }
}

impl Drop for PendingDialog {
    fn drop(&mut self) {
        if let Some(answer) = self.answer.take() {
            self.events.dialog(None);
            answer(false, String::new());
        }
    }
}

thread_local! {
    static OPEN_DIALOGS: RefCell<HashMap<String, PendingDialog>> = RefCell::new(HashMap::new());
}

define_class!(
    // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TabUiDelegateIvars]
    pub(super) struct TabUiDelegate;

    // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for TabUiDelegate {}

    // SAFETY: each method below has the signature WebKit declares for its selector, and
    // WebKit calls them on the main thread with a live webview and arguments.
    unsafe impl WKUIDelegate for TabUiDelegate {
        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        unsafe fn alert(
            &self,
            _webview: &WKWebView,
            message: &NSString,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let done = handler.copy();
            hold_dialog(
                self.ivars(),
                &message.to_string(),
                DialogKind::Alert,
                move |_, _| {
                    done.call(());
                },
            );
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        unsafe fn confirm(
            &self,
            _webview: &WKWebView,
            message: &NSString,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(Bool)>,
        ) {
            let done = handler.copy();
            hold_dialog(
                self.ivars(),
                &message.to_string(),
                DialogKind::Confirm,
                move |accepted, _| {
                    done.call((Bool::new(accepted),));
                },
            );
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        unsafe fn prompt(
            &self,
            _webview: &WKWebView,
            prompt: &NSString,
            default_text: Option<&NSString>,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(*mut NSString)>,
        ) {
            let done = handler.copy();
            let default = default_text
                .map(|text| text.to_string())
                .unwrap_or_default();
            hold_dialog(
                self.ivars(),
                &prompt.to_string(),
                DialogKind::Prompt(default),
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
            _frame: &WKFrameInfo,
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

        /* The new tab's page must be made from `configuration` and handed back, or the
        page that opened it loses its link to it, and a sign-in popup can never post
        its answer back. */
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        unsafe fn create_web_view(
            &self,
            _webview: &WKWebView,
            configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            // SAFETY: main thread, and WebKit keeps `action` alive for the callback.
            let url = unsafe { action.request() }
                .URL()
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string())
                .unwrap_or_default();
            let opener = Opener(configuration.retain());
            self.ivars()
                .events
                .new_window(url, Some(opener))
                .and_then(|tab| tab.webview_here())
        }

        #[unsafe(method(webViewDidClose:))]
        unsafe fn did_close(&self, _webview: &WKWebView) {
            self.ivars().events.closed();
        }
    }
);

impl TabUiDelegate {
    pub(super) fn new(
        mtm: MainThreadMarker,
        tab_id: String,
        events: Arc<dyn TabEvents>,
    ) -> Retained<Self> {
        let delegate = mtm.alloc::<TabUiDelegate>().set_ivars(TabUiDelegateIvars {
            tab_id,
            dialogs: RefCell::new(Burst::new(crate::DIALOG_LIMIT, crate::DIALOG_WINDOW)),
            events,
        });
        // SAFETY: `delegate` is freshly allocated with its ivars set, and NSObject's `init`
        // takes nothing and returns that same object.
        unsafe { msg_send![super(delegate), init] }
    }
}

enum DialogKind {
    Alert,
    Confirm,
    Prompt(String),
}

fn hold_dialog(
    tab: &TabUiDelegateIvars,
    message: &str,
    kind: DialogKind,
    answer: impl FnOnce(bool, String) + 'static,
) {
    if !tab.dialogs.borrow_mut().admit(Instant::now()) {
        answer(false, String::new());
        return;
    }
    let default_text = match &kind {
        DialogKind::Prompt(default) => Some(default.clone()),
        _ => None,
    };
    let described = PageDialog {
        kind: match &kind {
            DialogKind::Alert => "alert",
            DialogKind::Confirm => "confirm",
            DialogKind::Prompt(_) => "prompt",
        },
        message: message.to_owned(),
        default_text: default_text.clone(),
    };
    let pending = PendingDialog {
        answer: Some(Box::new(answer)),
        events: tab.events.clone(),
        default_text,
    };
    let previous = OPEN_DIALOGS.with(|open| open.borrow_mut().insert(tab.tab_id.clone(), pending));
    drop(previous);
    tab.events.dialog(Some(described));
}

/// Resolves the tab's pending dialog with `accept` and optional prompt `text`.
pub fn answer_dialog(tab_id: &str, accept: bool, text: Option<&str>) -> Result<(), String> {
    let pending = OPEN_DIALOGS
        .with(|open| open.borrow_mut().remove(tab_id))
        .ok_or("this tab has no open dialog")?;
    pending.resolve(accept, text);
    Ok(())
}
