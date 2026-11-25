//! The stand-in that holds the window's native views offscreen, and the pictures of them laid
//! over the window's own.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapImageFileType,
    NSBitmapImageRep, NSImage, NSView, NSWindow, NSWindowNumberListOptions, NSWindowStyleMask,
};
use objc2_foundation::{
    NSDictionary, NSError, NSIntersectionRect, NSNumber, NSPoint, NSRect, NSSize,
};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

/// Lets WebKit and AppKit do what they have waiting, without blocking, and fails if that put a
/// window of the film's where the person, or a list of windows they pick from, could show it.
pub fn run_loop() -> anyhow::Result<()> {
    core_foundation::runloop::CFRunLoop::run_in_mode(
        // SAFETY: a constant CoreFoundation sets up when it loads.
        unsafe { core_foundation::runloop::kCFRunLoopDefaultMode },
        Duration::ZERO,
        false,
    );
    let mtm = MainThreadMarker::new().context("a film runs on the main thread")?;
    let shown = NSWindow::windowNumbersWithOptions(NSWindowNumberListOptions::AllSpaces, mtm);
    anyhow::ensure!(
        shown.is_none_or(|shown| shown.is_empty()),
        "a window of the film's is on screen"
    );
    Ok(())
}

pub struct StandIn {
    window: Retained<NSWindow>,
    view: Retained<NSView>,
    mtm: MainThreadMarker,
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.window.setContentView(None);
        self.window.close();
    }
}

impl StandIn {
    /// A borderless window as big as the filmed one, never put on screen, whose view the
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
                NSRect::new(NSPoint::ZERO, size),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: as above.
        unsafe { window.setReleasedWhenClosed(false) };
        let view = NSView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::ZERO, size));
        window.setContentView(Some(&view));
        gpui_native_view::stand_in_for_offscreen(view.clone());
        Some(Self { window, view, mtm })
    }

    /// Lays every WebKit view showing in the stand-in over `shot`, where it sits, under what
    /// the window drew `above` them, in its own pixels. They keep what it left as
    /// its ground, as around a rounded corner.
    pub fn lay_over(
        &self,
        shot: &mut image::RgbaImage,
        above: &[gpui::Bounds<gpui::ScaledPixels>],
    ) -> anyhow::Result<()> {
        let drawn = shot.clone();
        let height = self.view.frame().size.height;
        let density = f64::from(shot.width()) / self.view.frame().size.width;
        let mut found = Vec::new();
        web_views(&self.view, &mut found);
        for webview in found {
            let mut visible = NSIntersectionRect(webview.visibleRect(), webview.bounds());
            // SAFETY: main thread, as `mtm` proves, and `webview` is live.
            if let Some(clip) = unsafe { webview.superview() } {
                let shown = webview.convertRect_fromView(clip.bounds(), Some(&clip));
                visible = NSIntersectionRect(visible, shown);
            }
            if visible.size.width < 1. || visible.size.height < 1. {
                continue;
            }
            let placed = webview.convertRect_toView(visible, Some(&self.view));
            let top = if self.view.isFlipped() {
                placed.origin.y
            } else {
                height - placed.origin.y - placed.size.height
            };
            let mut picture = self.picture(&webview, visible, placed.size.width * density)?;
            round_bottom(&mut picture, (bottom_radius(&webview) * density) as f32);
            image::imageops::overlay(
                shot,
                &picture,
                (placed.origin.x * density).round() as i64,
                (top * density).round() as i64,
            );
        }
        let (max_w, max_h) = (drawn.width(), drawn.height());
        if max_w > 0 && max_h > 0 {
            for bounds in above {
                let left = (bounds.origin.x.0.max(0.) as u32).min(max_w);
                let top = (bounds.origin.y.0.max(0.) as u32).min(max_h);
                let right = left
                    .saturating_add(bounds.size.width.0.max(0.) as u32)
                    .min(max_w);
                let bottom = top
                    .saturating_add(bounds.size.height.0.max(0.) as u32)
                    .min(max_h);
                if left >= right || top >= bottom {
                    continue;
                }
                let ground_x = left.saturating_sub(1).min(max_w - 1);
                for y in top..bottom {
                    let ground = *drawn.get_pixel(ground_x, y);
                    let inside = |x: &u32| !alike(drawn.get_pixel(*x, y), &ground);
                    let Some(first) = (left..right).find(inside) else {
                        continue;
                    };
                    let last = (first..right).rev().find(inside).unwrap_or(first);
                    for x in first..=last {
                        shot.put_pixel(x, y, *drawn.get_pixel(x, y));
                    }
                }
            }
        }
        Ok(())
    }

    /// What `webview` draws in `rect`, `width` pixels wide, so a page drawn scaled down comes
    /// out at the size it shows.
    fn picture(
        &self,
        webview: &WKWebView,
        rect: NSRect,
        width: f64,
    ) -> anyhow::Result<image::RgbaImage> {
        let screen = webview
            .window()
            .map_or(1., |window| window.backingScaleFactor());
        // SAFETY: main thread, as `mtm` proves.
        let configuration = unsafe { WKSnapshotConfiguration::new(self.mtm) };
        // SAFETY: `configuration` is ours and not yet handed to WebKit.
        unsafe {
            configuration.setRect(rect);
            configuration.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width / screen)));
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
            run_loop()?;
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

fn alike(one: &image::Rgba<u8>, other: &image::Rgba<u8>) -> bool {
    (one.0.iter().zip(other.0)).all(|(one, other)| one.abs_diff(other) <= 1)
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

/// How far AppKit rounds `webview`'s lower corners, by its own layer or the view that cuts it.
fn bottom_radius(webview: &WKWebView) -> f64 {
    // SAFETY: main thread, as every caller has a `StandIn`, and `webview` is live.
    let clip = unsafe { webview.superview() };
    let radius = |view: &NSView| -> f64 {
        // SAFETY: every NSView answers `layer` with a CALayer or nil.
        let layer: Option<Retained<AnyObject>> = unsafe { msg_send![view, layer] };
        // SAFETY: a CALayer's `cornerRadius` is a CGFloat.
        layer.map_or(0., |layer| unsafe { msg_send![&*layer, cornerRadius] })
    };
    clip.as_deref().map_or(0., radius).max(radius(webview))
}

fn round_bottom(picture: &mut image::RgbaImage, radius: f32) {
    let (width, height) = picture.dimensions();
    let (right, bottom) = (width as f32 - radius, height as f32 - radius);
    for y in (bottom.max(0.) as u32)..height {
        for x in 0..width {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let across = (radius - px).max(px - right).max(0.);
            let down = (py - bottom).max(0.);
            let inside = (radius - across.hypot(down) + 0.5).clamp(0., 1.);
            let alpha = &mut picture.get_pixel_mut(x, y).0[3];
            *alpha = (f32::from(*alpha) * inside) as u8;
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
