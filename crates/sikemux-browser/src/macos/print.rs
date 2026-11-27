//! Native macOS printing for visible desk browser tabs via `NSPrintOperation`.

use objc2_app_kit::{NSPrintInfo, NSPrintOperation, NSWindow};
use objc2_foundation::NSCopying;

use super::*;

thread_local! {
    static PRINTING: Cell<bool> = const { Cell::new(false) };
    static PRINT_DELEGATE: RefCell<Option<Retained<PrintDelegate>>> = const { RefCell::new(None) };
}

define_class!(
    // SAFETY: a plain NSObject subclass with no ivars and no dealloc, used only on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct PrintDelegate;

    // SAFETY: every NSObject subclass conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for PrintDelegate {}

    impl PrintDelegate {
        // SAFETY: matches AppKit's `didRunSelector` signature for `NSPrintOperation`.
        #[unsafe(method(printOperationDidRun:success:contextInfo:))]
        fn did_run(
            &self,
            _operation: Option<&NSPrintOperation>,
            _success: Bool,
            _context_info: *mut c_void,
        ) {
            PRINTING.with(|active| active.set(false));
        }
    }
);

impl PrintDelegate {
    fn shared(mtm: MainThreadMarker) -> Retained<Self> {
        PRINT_DELEGATE.with(|slot| {
            slot.borrow_mut()
                .get_or_insert_with(|| {
                    let delegate = mtm.alloc::<PrintDelegate>().set_ivars(());
                    // SAFETY: NSObject's `init` takes nothing and returns the object it was sent to.
                    unsafe { msg_send![super(delegate), init] }
                })
                .clone()
        })
    }
}

fn printable_window(webview: &WKWebView) -> Option<Retained<NSWindow>> {
    let window = webview.window()?;
    if !window.isVisible() || window.isMiniaturized() || window.attachedSheet().is_some() {
        return None;
    }
    if webview.isHiddenOrHasHiddenAncestor() || webview.alphaValue() <= 0.0 {
        return None;
    }
    // SAFETY: main thread, and `webview` is alive for this call.
    let superview = unsafe { webview.superview() }?;
    intersection(webview.frame(), superview.bounds())?;
    Some(window)
}

pub(super) fn can_print(webview: &WKWebView, from_page: bool) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    if from_page && crate::input::agent_input_is_latest() {
        return false;
    }
    if PRINTING.with(Cell::get) || NSPrintOperation::currentOperation(mtm).is_some() {
        return false;
    }
    printable_window(webview).is_some()
}

pub(super) fn print_webview(webview: &WKWebView) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if PRINTING.with(Cell::get) || NSPrintOperation::currentOperation(mtm).is_some() {
        return;
    }
    let Some(window) = printable_window(webview) else {
        return;
    };
    let print_info = NSPrintInfo::sharedPrintInfo().copy();
    // SAFETY: main thread, and both `webview` and `print_info` are live.
    let operation = unsafe { webview.printOperationWithPrintInfo(&print_info) };
    operation.setShowsPrintPanel(true);
    operation.setShowsProgressPanel(true);
    // SAFETY: main thread, and `webview` is live.
    if let Some(title) = unsafe { webview.title() } {
        if !title.is_empty() {
            operation.setJobTitle(Some(&title));
        }
    }
    // WebKit can leave the internal printing view at zero size, causing AppKit to fail pagination.
    if let Some(print_view) = operation.view() {
        let frame = print_view.frame();
        if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
            let bounds = webview.bounds();
            if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
                print_view.setFrame(bounds);
            }
        }
    }
    let delegate = PrintDelegate::shared(mtm);
    PRINTING.with(|active| active.set(true));
    // SAFETY: main thread; `window` and the singleton `delegate` stay alive while the sheet runs,
    // and `printOperationDidRun:success:contextInfo:` matches AppKit's expected signature.
    unsafe {
        operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            &window,
            Some(delegate.as_ref()),
            Some(sel!(printOperationDidRun:success:contextInfo:)),
            std::ptr::null_mut(),
        );
    }
}
