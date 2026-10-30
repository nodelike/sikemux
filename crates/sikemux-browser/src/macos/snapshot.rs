//! Pictures of a page: what it shows now, or the whole of it, as JPEG.

use super::*;

/// The visible page as a JPEG at 1x: plenty for a model to read at a fraction
/// of the bytes of a Retina PNG.
/// `area` is a part of the viewport in CSS pixels: left, top, width, height.
pub fn snapshot_jpeg(
    webview: &WKWebView,
    area: Option<(f64, f64, f64, f64)>,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let webview = webview.retain();
    let Some(mtm) = MainThreadMarker::new() else {
        done(Err("the tab is gone".into()));
        return;
    };
    // SAFETY: main thread, as `mtm` proves.
    let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
    let scale = webview
        .window()
        .map(|window| window.backingScaleFactor())
        .unwrap_or(1.0)
        .max(1.0);
    // SAFETY: main thread, and `webview` is retained.
    let zoom = unsafe { webview.pageZoom() }.max(0.01);
    let width = match area {
        Some((_, _, width, _)) => (width / scale).max(1.0),
        None => (webview.bounds().size.width / zoom / scale).max(1.0),
    };
    // SAFETY: `configuration` is ours and not yet handed to WebKit.
    unsafe {
        configuration.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width)));
        configuration.setAfterScreenUpdates(true);
        if let Some((left, top, width, height)) = area {
            configuration.setRect(NSRect::new(
                NSPoint::new(left * zoom, top * zoom),
                NSSize::new(width * zoom, height * zoom),
            ));
        }
    }
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let Some(done) = done.locked().take() else {
            return;
        };
        // SAFETY: WebKit passes a live image or nil; `retain` takes our own reference.
        let image = unsafe { Retained::retain(image) };
        let Some(image) = image else {
            // SAFETY: WebKit passes a live error or nil; `retain` takes our own reference.
            done(Err(unsafe { Retained::retain(error) }
                .map(|error| error.localizedDescription().to_string())
                .unwrap_or_else(|| "the page could not be captured".into())));
            return;
        };
        let Some(pixels) = image.TIFFRepresentation().map(|tiff| tiff.to_vec()) else {
            done(Err("could not encode the page image".into()));
            return;
        };
        // Compressing the picture takes milliseconds, and this block runs on
        // the thread that draws every window.
        std::thread::spawn(move || {
            done(jpeg_bytes(&pixels).ok_or_else(|| "could not encode the page image".to_string()));
        });
    });
    // SAFETY: main thread; WebKit copies the block and holds `configuration` for the call.
    unsafe {
        webview.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block)
    };
}

/// Where a script runs. The page world is the page's own, where scripts see
/// and can change each other's globals. The helper world shares the page's DOM
/// but none of its JavaScript, so the page cannot tamper with what runs there.
#[derive(Clone, Copy)]
pub enum World {
    Page,
    Helper,
}

/// The whole page rather than the part on screen, as a JPEG at 1x. WebKit
/// lays it out as one tall PDF page, which is then drawn as a picture. A page
/// taller than `most` is cut at that height.
pub fn full_page_jpeg(
    webview: &WKWebView,
    height: f64,
    most: f64,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let webview = webview.retain();
    let Some(mtm) = MainThreadMarker::new() else {
        done(Err("the tab is gone".into()));
        return;
    };
    // SAFETY: main thread, as `mtm` proves.
    let configuration = unsafe { WKPDFConfiguration::new(mtm) };
    if height > most {
        let width = webview.bounds().size.width;
        // SAFETY: `configuration` is ours and not yet handed to WebKit.
        unsafe {
            configuration.setRect(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, most),
            ))
        };
    }
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
        let Some(done) = done.locked().take() else {
            return;
        };
        // SAFETY: WebKit passes live data or nil; `retain` takes our own reference.
        let Some(data) = (unsafe { Retained::retain(data) }) else {
            // SAFETY: WebKit passes a live error or nil; `retain` takes our own reference.
            done(Err(unsafe { Retained::retain(error) }
                .map(|error| error.localizedDescription().to_string())
                .unwrap_or_else(|| "the page could not be captured".into())));
            return;
        };
        let pixels = NSImage::initWithData(mtm.alloc::<NSImage>(), &data)
            .and_then(|image| image.TIFFRepresentation())
            .map(|tiff| tiff.to_vec());
        let Some(pixels) = pixels else {
            done(Err("could not draw the page".into()));
            return;
        };
        std::thread::spawn(move || {
            done(jpeg_bytes(&pixels).ok_or_else(|| "could not encode the page image".to_string()));
        });
    });
    // SAFETY: main thread; WebKit copies the block and holds `configuration` for the call.
    unsafe { webview.createPDFWithConfiguration_completionHandler(Some(&configuration), &block) };
}

pub(super) fn jpeg_bytes(image: &[u8]) -> Option<Vec<u8>> {
    let bitmap = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(image))?;
    let quality = NSNumber::numberWithDouble(0.82);
    let properties: Retained<NSDictionary<NSString, AnyObject>> =
        // SAFETY: an immutable constant string AppKit sets up when it loads.
        NSDictionary::from_slices(&[unsafe { NSImageCompressionFactor }], &[&*quality]);
    // SAFETY: NSBitmapImageRep works off the main thread, and `properties` maps the
    // compression key to an NSNumber as AppKit expects.
    let jpeg = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::JPEG, &properties)
    }?;
    Some(jpeg.to_vec())
}
