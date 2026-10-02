//! Shows office documents, iWork files and other formats WebKit cannot draw
//! with macOS's own Quick Look view, laid over the editor where the page asks.
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
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL};
    use objc2_quick_look_ui::{QLPreviewView, QLPreviewViewStyle};
    use objc2_web_kit::WKWebView;

    struct Shown {
        view: Retained<QLPreviewView>,
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
        SHOWN.with(|shown| {
            let mut shown = shown.borrow_mut();
            if shown.is_none() {
                // SAFETY: main thread, and a zero frame is replaced below before drawing.
                let view = unsafe {
                    QLPreviewView::initWithFrame_style(
                        mtm.alloc(),
                        NSRect::ZERO,
                        QLPreviewViewStyle::Normal,
                    )
                };
                let Some(view) = view else {
                    return;
                };
                // SAFETY: main thread; the view lives as long as the window does.
                unsafe { view.setShouldCloseWithWindow(false) };
                *shown = Some(Shown {
                    view,
                    owner: String::new(),
                    path: String::new(),
                });
            }
            let Some(current) = shown.as_mut() else {
                return;
            };
            let view: &NSView = &current.view;
            // SAFETY: main thread, and both views are retained.
            unsafe {
                if view.superview().as_deref() != Some(&*parent) {
                    parent.addSubview(view);
                }
            }
            let path_text = path.to_string_lossy().into_owned();
            if current.path != path_text {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path_text));
                // SAFETY: main thread, and NSURL is a Quick Look item.
                unsafe {
                    current
                        .view
                        .setPreviewItem(Some(ProtocolObject::from_ref(&*url)))
                };
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
            current.view.setHidden(true);
            // SAFETY: main thread; clearing the item closes the document.
            unsafe { current.view.setPreviewItem(None) };
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
