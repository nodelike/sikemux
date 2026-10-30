//! Page dialogs as window sheets, and the answers agents give them.

use super::*;

pub(super) struct TabUiDelegateIvars {
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
    pub(super) struct TabUiDelegate;

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

pub(super) enum Sheet {
    Alert,
    Confirm,
    Prompt(String),
}

/// A page dialog as a sheet on the app window, the way Safari shows them. The
/// page waits on `answer`, so every path must call it exactly once. The agent
/// answers through `answer_dialog`, which ends the same sheet.
pub(super) fn present_sheet(
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
pub(super) struct AnswerOnce<F: Fn(bool, String)> {
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
