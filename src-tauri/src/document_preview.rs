//! Shows PDFs, office documents, iWork files and other formats WebKit cannot
//! draw well with macOS's own views, laid over the editor where the page asks.
//! PDFs get PDFKit's view, which pinches, scrolls and lets the app show around
//! the pages; everything else gets Quick Look's.
//! There is one view for the whole window; whichever viewer showed it last owns
//! it, so a viewer that closes late cannot hide the next one's document.

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

/// Where the view goes, in the window's CSS pixels from its top left.
#[derive(Deserialize, Clone, Copy)]
pub struct Placement {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[tauri::command]
pub async fn document_preview_show(
    app: AppHandle,
    owner: String,
    path: String,
    placement: Placement,
) -> AppResult<()> {
    let path = std::path::Path::new(&path).canonicalize()?;
    if !path.is_file() {
        return Err(AppError::Fs(format!("{} is not a file", path.display())));
    }
    if !(placement.width > 0.0 && placement.height > 0.0) {
        return document_preview_hide(app, owner).await;
    }
    let page = app
        .get_webview("main")
        .ok_or_else(|| AppError::Other("document preview: the main page is gone".into()))?;
    page.with_webview(move |platform| platform::show(platform.inner(), owner, &path, placement))
        .map_err(|error| AppError::Other(format!("document preview: {error}")))
}

/// A reloaded page has no viewer left to hide the view it asked for.
pub fn clear(app: &AppHandle) {
    let _ = app.run_on_main_thread(platform::clear);
}

#[tauri::command]
pub async fn document_preview_hide(app: AppHandle, owner: String) -> AppResult<()> {
    app.run_on_main_thread(move || platform::hide(&owner))
        .map_err(|error| AppError::Other(format!("document preview: {error}")))
}

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::path::Path;

    use super::Placement;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Bool, ProtocolObject};
    use objc2::{msg_send, MainThreadMarker};
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL};
    use objc2_quick_look_ui::{QLPreviewView, QLPreviewViewStyle};
    use objc2_web_kit::WKWebView;

    #[link(name = "PDFKit", kind = "framework")]
    extern "C" {}

    enum Viewer {
        QuickLook(Retained<QLPreviewView>),
        Pdf(Retained<NSView>),
    }

    impl Viewer {
        fn view(&self) -> &NSView {
            match self {
                Viewer::QuickLook(view) => view,
                Viewer::Pdf(view) => view,
            }
        }

        fn shows_pdf(&self) -> bool {
            matches!(self, Viewer::Pdf(_))
        }

        fn new(mtm: MainThreadMarker, pdf: bool) -> Option<Self> {
            if pdf {
                return pdf_view().map(Viewer::Pdf);
            }
            // SAFETY: main thread, and a zero frame is replaced before drawing.
            let view = unsafe {
                QLPreviewView::initWithFrame_style(
                    mtm.alloc(),
                    NSRect::ZERO,
                    QLPreviewViewStyle::Normal,
                )
            }?;
            // SAFETY: main thread; the view lives as long as the window does.
            unsafe { view.setShouldCloseWithWindow(false) };
            Some(Viewer::QuickLook(view))
        }

        fn open(&self, url: Option<&NSURL>) {
            match self {
                // SAFETY: main thread, and NSURL is a Quick Look item.
                Viewer::QuickLook(view) => unsafe {
                    view.setPreviewItem(url.map(ProtocolObject::from_ref))
                },
                Viewer::Pdf(view) => {
                    let document = url.and_then(pdf_document);
                    // SAFETY: PDFView's `setDocument:` takes a PDFDocument or nil.
                    let _: () = unsafe { msg_send![&**view, setDocument: document.as_deref()] };
                }
            }
        }
    }

    fn pdf_view() -> Option<Retained<NSView>> {
        let class = AnyClass::get(c"PDFView")?;
        let clear = AnyClass::get(c"NSColor")?;
        // SAFETY: PDFView is an NSView subclass made with `initWithFrame:`; the
        // setters take a BOOL and an NSColor.
        unsafe {
            let allocated: Allocated<NSView> = msg_send![class, alloc];
            let view: Retained<NSView> = msg_send![allocated, initWithFrame: NSRect::ZERO];
            let clear: Retained<AnyObject> = msg_send![clear, clearColor];
            let _: () = msg_send![&*view, setAutoScales: Bool::YES];
            let _: () = msg_send![&*view, setBackgroundColor: &*clear];
            Some(view)
        }
    }

    fn pdf_document(url: &NSURL) -> Option<Retained<AnyObject>> {
        let class = AnyClass::get(c"PDFDocument")?;
        // SAFETY: PDFDocument's `initWithURL:` answers nil for a file it cannot read.
        unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![allocated, initWithURL: url]
        }
    }

    struct Shown {
        viewer: Viewer,
        owner: String,
        path: String,
    }

    thread_local! {
        static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    }

    /// `page` is the main WKWebView; the view goes in beside it, where the browser tabs sit.
    pub fn show(page: *mut c_void, owner: String, path: &Path, placement: Placement) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // SAFETY: Tauri's `with_webview` hands over the live WKWebView on the main thread.
        let Some(page) = (unsafe { Retained::retain(page.cast::<WKWebView>()) }) else {
            return;
        };
        // SAFETY: main thread, and `page` is retained.
        let Some(parent) = (unsafe { page.superview() }) else {
            return;
        };
        let pdf = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
        SHOWN.with(|shown| {
            let mut shown = shown.borrow_mut();
            if shown
                .as_ref()
                .is_some_and(|current| current.viewer.shows_pdf() != pdf)
            {
                if let Some(previous) = shown.take() {
                    previous.viewer.open(None);
                    previous.viewer.view().removeFromSuperview();
                }
            }
            if shown.is_none() {
                let Some(viewer) = Viewer::new(mtm, pdf) else {
                    return;
                };
                *shown = Some(Shown {
                    viewer,
                    owner: String::new(),
                    path: String::new(),
                });
            }
            let Some(current) = shown.as_mut() else {
                return;
            };
            let view = current.viewer.view();
            // SAFETY: main thread, and both views are retained.
            unsafe {
                if view.superview().as_deref() != Some(&*parent) {
                    parent.addSubview(view);
                }
            }
            let path_text = path.to_string_lossy().into_owned();
            if current.path != path_text {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path_text));
                current.viewer.open(Some(&url));
                current.path = path_text;
            }
            current.owner = owner;
            let size = NSSize::new(placement.width, placement.height);
            let origin = if parent.isFlipped() {
                NSPoint::new(placement.x, placement.y)
            } else {
                NSPoint::new(
                    placement.x,
                    parent.frame().size.height - placement.y - placement.height,
                )
            };
            view.setFrame(NSRect::new(origin, size));
            view.setHidden(false);
        });
    }

    /// Hides the view and lets go of the document, so a closed viewer holds no memory.
    pub fn hide(owner: &str) {
        release(|current| current.owner == owner);
    }

    pub fn clear() {
        release(|_| true);
    }

    fn release(when: impl FnOnce(&Shown) -> bool) {
        SHOWN.with(|shown| {
            let mut shown = shown.borrow_mut();
            let Some(current) = shown.as_mut() else {
                return;
            };
            if !when(current) {
                return;
            }
            current.viewer.view().setHidden(true);
            current.viewer.open(None);
            current.path.clear();
            current.owner.clear();
        });
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    use super::Placement;

    pub fn show<Page>(_page: Page, _owner: String, _path: &Path, _placement: Placement) {}

    pub fn hide(_owner: &str) {}

    pub fn clear() {}
}
