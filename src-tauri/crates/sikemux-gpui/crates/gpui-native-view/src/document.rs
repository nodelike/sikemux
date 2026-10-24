//! A document macOS draws itself: a PDF in PDFKit's view, which pinches and
//! scrolls; office, iWork and other files in Quick Look's.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use gpui::Window;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ProtocolObject};
use objc2::{msg_send, MainThreadMarker};
use objc2_app_kit::NSView;
use objc2_foundation::{NSRect, NSString, NSURL};
use objc2_quick_look_ui::{QLPreviewView, QLPreviewViewStyle};

use crate::{Frame, OwnedView, Surface};

#[link(name = "PDFKit", kind = "framework")]
extern "C" {}

/// A file shown by macOS's own viewer, under the window's overlays.
pub struct Document {
    view: Viewer,
    owned: OwnedView,
    path: RefCell<PathBuf>,
}

enum Viewer {
    Pdf(Retained<NSView>),
    QuickLook(Retained<QLPreviewView>),
}

impl Document {
    /// Shows the file at `path`, out of sight until an element places it.
    pub fn new(path: &Path, window: &Window) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let (view, shown): (Viewer, Retained<NSView>) = if is_pdf(path) {
            let view = pdf_view()?;
            (Viewer::Pdf(view.clone()), view)
        } else {
            // SAFETY: main thread, and a zero frame is replaced before drawing.
            let view = unsafe {
                QLPreviewView::initWithFrame_style(
                    mtm.alloc(),
                    NSRect::ZERO,
                    QLPreviewViewStyle::Normal,
                )
            }?;
            // SAFETY: main thread; `Drop` closes the view itself.
            unsafe { view.setShouldCloseWithWindow(false) };
            (Viewer::QuickLook(view.clone()), Retained::into_super(view))
        };
        let document = Self {
            view,
            owned: OwnedView::new(shown, window)?,
            path: RefCell::new(PathBuf::new()),
        };
        document.open(path);
        Some(document)
    }

    /// Shows `path` in place of the file shown now, or the same file again after it
    /// changed. A PDF's viewer shows only PDFs; make another `Document` for a file of the other kind.
    pub fn open(&self, path: &Path) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        self.show(Some(&url));
        *self.path.borrow_mut() = path.to_owned();
    }

    pub fn path(&self) -> PathBuf {
        self.path.borrow().clone()
    }

    fn show(&self, url: Option<&NSURL>) {
        match &self.view {
            Viewer::Pdf(view) => {
                let document = url.and_then(pdf_document);
                // SAFETY: PDFView's `setDocument:` takes a PDFDocument or nil.
                let _: () = unsafe { msg_send![&**view, setDocument: document.as_deref()] };
            }
            Viewer::QuickLook(view) => {
                // SAFETY: main thread, and an NSURL is a Quick Look item.
                unsafe { view.setPreviewItem(url.map(ProtocolObject::from_ref)) };
                if url.is_some() {
                    // SAFETY: main thread; Quick Look keeps the last drawing of an item it shows again.
                    unsafe { view.refreshPreviewItem() };
                }
            }
        }
    }
}

/// Whether `path` gets PDFKit's viewer rather than Quick Look's.
pub fn is_pdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

impl Surface for Document {
    fn place(&self, frame: Option<Frame>) {
        self.owned.place(frame);
    }

    fn responder(&self) -> Option<Retained<NSView>> {
        self.owned.responder()
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.show(None);
        if let Viewer::QuickLook(view) = &self.view {
            // SAFETY: main thread; a view that does not close with its window is closed by hand.
            unsafe { view.close() };
        }
    }
}

fn pdf_view() -> Option<Retained<NSView>> {
    let class = AnyClass::get(c"PDFView")?;
    let colors = AnyClass::get(c"NSColor")?;
    // SAFETY: PDFView is an NSView subclass made with `initWithFrame:`; the setters
    // take a BOOL and an NSColor.
    unsafe {
        let allocated: Allocated<NSView> = msg_send![class, alloc];
        let view: Retained<NSView> = msg_send![allocated, initWithFrame: NSRect::ZERO];
        let clear: Retained<AnyObject> = msg_send![colors, clearColor];
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
