//! The stand-in that holds the window's native views offscreen, and the pictures of them laid
//! over the window's own.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapImageFileType,
    NSBitmapImageRep, NSImage, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSPoint, NSRect, NSSize};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

/// Lets WebKit and AppKit do what they have waiting, without blocking.
pub fn run_loop() {
    core_foundation::runloop::CFRunLoop::run_in_mode(
        // SAFETY: a constant CoreFoundation sets up when it loads.
        unsafe { core_foundation::runloop::kCFRunLoopDefaultMode },
        Duration::ZERO,
        false,
    );
}

pub struct StandIn {
    _window: Retained<NSWindow>,
    view: Retained<NSView>,
    mtm: MainThreadMarker,
}

impl StandIn {
    /// A borderless window as big as the filmed one, far off screen, whose view the
    /// offscreen window's native views are added to.
    pub fn open(size: (f32, f32)) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        let size = NSSize::new(f64::from(size.0), f64::from(size.1));
        // SAFETY: main thread, and the window is kept for as long as the film runs.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                NSRect::new(NSPoint::new(-20_000., -20_000.), size),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: as above.
        unsafe { window.setReleasedWhenClosed(false) };
        let view = NSView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::ZERO, size));
        window.setContentView(Some(&view));
        window.orderBack(None);
        gpui_native_view::stand_in_for_offscreen(view.clone());
        Some(Self {
            _window: window,
            view,
            mtm,
        })
    }

    /// Lays every WebKit view showing in the stand-in over `shot`, where it sits.
    pub fn lay_over(&self, shot: &mut image::RgbaImage) -> anyhow::Result<()> {
        let height = self.view.frame().size.height;
        let density = f64::from(shot.width()) / self.view.frame().size.width;
        let mut found = Vec::new();
        web_views(&self.view, &mut found);
        for webview in found {
            let visible = webview.visibleRect();
            if visible.size.width < 1. || visible.size.height < 1. {
                continue;
            }
            let placed = webview.convertRect_toView(visible, Some(&self.view));
            let top = if self.view.isFlipped() {
                placed.origin.y
            } else {
                height - placed.origin.y - placed.size.height
            };
            let picture = self.picture(&webview, visible, density)?;
            image::imageops::overlay(
                shot,
                &picture,
                (placed.origin.x * density).round() as i64,
                (top * density).round() as i64,
            );
        }
        Ok(())
    }

    /// What `webview` draws in `rect`, at `density` pixels a point.
    fn picture(
        &self,
        webview: &WKWebView,
        rect: NSRect,
        density: f64,
    ) -> anyhow::Result<image::RgbaImage> {
        let screen = webview
            .window()
            .map_or(1., |window| window.backingScaleFactor());
        // SAFETY: main thread, as `mtm` proves.
        let configuration = unsafe { WKSnapshotConfiguration::new(self.mtm) };
        // SAFETY: `configuration` is ours and not yet handed to WebKit.
        unsafe {
            configuration.setRect(rect);
            configuration.setSnapshotWidth(Some(&NSNumber::numberWithDouble(
                rect.size.width * density / screen,
            )));
        }
        let taken: Rc<RefCell<Option<Option<Vec<u8>>>>> = Rc::default();
        let kept = taken.clone();
        let block = RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
            // SAFETY: WebKit passes a live image or nil; `retain` takes our own reference.
            let image = unsafe { Retained::retain(image) };
            *kept.borrow_mut() = Some(image.and_then(|image| png(&image)));
        });
        // SAFETY: main thread; WebKit copies the block and holds `configuration` for the call.
        unsafe {
            webview.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block)
        };
        let started = Instant::now();
        while taken.borrow().is_none() {
            if started.elapsed() > Duration::from_secs(10) {
                return Err(anyhow!("WebKit never drew a page"));
            }
            run_loop();
            std::thread::sleep(Duration::from_millis(5));
        }
        let png = taken
            .borrow_mut()
            .take()
            .flatten()
            .ok_or_else(|| anyhow!("WebKit drew no picture of a page"))?;
        Ok(image::load_from_memory(&png)
            .context("reading a page's picture")?
            .to_rgba8())
    }
}

/// The WebKit views under `view` that are not hidden, front to back as AppKit keeps them.
fn web_views(view: &NSView, found: &mut Vec<Retained<WKWebView>>) {
    for child in view.subviews().iter() {
        if child.isHiddenOrHasHiddenAncestor() {
            continue;
        }
        match child.downcast::<WKWebView>() {
            Ok(webview) => found.push(webview),
            Err(child) => web_views(&child, found),
        }
    }
}

fn png(image: &NSImage) -> Option<Vec<u8>> {
    let tiff = image.TIFFRepresentation()?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)?;
    // SAFETY: an empty dictionary is a valid set of properties.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(png.to_vec())
}
