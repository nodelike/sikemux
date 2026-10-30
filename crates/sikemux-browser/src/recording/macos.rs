//! A tab's recording on macOS: WebKit's snapshots, encoded as H.264 MP4 by AVFoundation.

use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, Message};
use objc2_app_kit::NSImage;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoHeightKey,
    AVVideoWidthKey,
};
use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_core_media::{kCMTimeZero, CMTime};
use objc2_core_video::{
    kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_32BGRA, kCVReturnSuccess, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString, NSURL};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};
use sikemux_process::sync::Locked;

use super::{Frame, Summary};

pub(super) type Picture = CFRetained<CGImage>;

pub(super) fn extension() -> &'static str {
    "mp4"
}

pub(super) fn size(webview: &WKWebView) -> (f64, f64) {
    let size = webview.frame().size;
    (size.width, size.height)
}

pub(super) struct Writer {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    width: usize,
    height: usize,
}

fn cf_key(key: &CFString) -> &NSString {
    // SAFETY: CFString and NSString are toll-free bridged, so the same pointer is a
    // valid NSString for as long as `key` is borrowed.
    unsafe { &*(key as *const CFString).cast::<NSString>() }
}

impl Writer {
    pub(super) fn open(path: &Path, width: usize, height: usize) -> Result<Self, String> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        // SAFETY: immutable constant strings AVFoundation sets up when it loads.
        let (file_type, media_type, codec_key, h264, width_key, height_key) = unsafe {
            (
                AVFileTypeMPEG4,
                AVMediaTypeVideo,
                AVVideoCodecKey,
                AVVideoCodecTypeH264,
                AVVideoWidthKey,
                AVVideoHeightKey,
            )
        };
        let (
            Some(file_type),
            Some(media_type),
            Some(codec_key),
            Some(h264),
            Some(width_key),
            Some(height_key),
        ) = (
            file_type, media_type, codec_key, h264, width_key, height_key,
        )
        else {
            return Err("this system has no H.264 video writer".into());
        };
        // SAFETY: `url` is a file URL and `file_type` is AVFoundation's own constant. The
        // writer is created and used only on this recording thread.
        let writer = unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type) }
            .map_err(|error| describe(&error))?;
        let (wide, tall) = (
            NSNumber::numberWithUnsignedInteger(width),
            NSNumber::numberWithUnsignedInteger(height),
        );
        let settings: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
            &[codec_key, width_key, height_key],
            &[h264.as_ref(), wide.as_ref(), tall.as_ref()],
        );
        // SAFETY: a plain query on the fresh writer; it throws on nothing.
        if !unsafe { writer.canApplyOutputSettings_forMediaType(Some(&settings), media_type) } {
            return Err(format!("a {width}x{height} video cannot be recorded"));
        }
        // SAFETY: `settings` uses AVFoundation's own keys with the NSString and NSNumber
        // values it documents for them, and the writer accepts them.
        let input = unsafe {
            AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media_type,
                Some(&settings),
            )
        };
        // SAFETY: `input` is fresh and not yet attached to a writer, so it can still be set up.
        unsafe { input.setExpectsMediaDataInRealTime(true) };
        let format = NSNumber::numberWithUnsignedInt(kCVPixelFormatType_32BGRA);
        // SAFETY: the kCVPixelBuffer keys are immutable constants CoreVideo sets up when it loads.
        let attributes: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
            NSDictionary::from_slices(
                &[
                    cf_key(kCVPixelBufferPixelFormatTypeKey),
                    cf_key(kCVPixelBufferWidthKey),
                    cf_key(kCVPixelBufferHeightKey),
                ],
                &[format.as_ref(), wide.as_ref(), tall.as_ref()],
            )
        };
        // SAFETY: `input` is not yet added to the writer, as the adaptor requires.
        let adaptor = unsafe {
            AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                &input,
                Some(&attributes),
            )
        };
        // SAFETY: this thread alone owns the writer and input. The input is added before
        // writing starts, and the session starts only once writing has begun.
        unsafe {
            if !writer.canAddInput(&input) {
                return Err("the video writer refused its input".into());
            }
            writer.addInput(&input);
            if !writer.startWriting() {
                return Err(writer
                    .error()
                    .map(|error| describe(&error))
                    .unwrap_or_else(|| "the video writer did not start".into()));
            }
            writer.startSessionAtSourceTime(kCMTimeZero);
        }
        Ok(Self {
            writer,
            input,
            adaptor,
            width,
            height,
        })
    }

    pub(super) fn write(self, incoming: mpsc::Receiver<Frame>) -> Result<Summary, String> {
        let mut frames = 0;
        let mut last: Option<Duration> = None;
        for frame in incoming {
            let at_ms = frame.at.as_millis();
            if last.is_some_and(|last| at_ms <= last.as_millis()) || !self.ready() {
                continue;
            }
            let Some(buffer) = self.draw(&frame.image) else {
                continue;
            };
            if !self.ready() {
                continue;
            }
            // SAFETY: builds a plain value from two integers.
            let at = unsafe { CMTime::new(at_ms as i64, 1000) };
            // SAFETY: `buffer` came from the adaptor's own pool, and `at` is later than every
            // frame already appended.
            if unsafe {
                self.adaptor
                    .appendPixelBuffer_withPresentationTime(&buffer, at)
            } {
                frames += 1;
                last = Some(frame.at);
            }
        }
        self.finish(frames, last.unwrap_or_default())
    }

    /// AVFoundation throws, rather than refusing, when a frame arrives while the
    /// input is busy or the writer has failed.
    fn ready(&self) -> bool {
        // SAFETY: `writer` and `input` belong to this thread; reading their state is always allowed.
        unsafe {
            self.writer.status() == AVAssetWriterStatus::Writing
                && self.input.isReadyForMoreMediaData()
        }
    }

    /// Draws a picture into a fresh video frame, fitted inside it: a tab that
    /// is resized while recording keeps its proportions.
    fn draw(&self, image: &CGImage) -> Option<CFRetained<CVPixelBuffer>> {
        // SAFETY: the adaptor's writer has started, so the pool is either ready or nil.
        let pool: Retained<CVPixelBufferPool> = unsafe { self.adaptor.pixelBufferPool() }?;
        let mut raw: *mut CVPixelBuffer = std::ptr::null_mut();
        let created =
            // SAFETY: `raw` is a valid slot for CoreVideo to write the new buffer's pointer into.
            unsafe { CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut raw)) };
        if created != kCVReturnSuccess {
            return None;
        }
        // SAFETY: a successful create gives us one reference in `raw`, which this takes over.
        let buffer = unsafe { CFRetained::from_raw(NonNull::new(raw)?) };
        let flags = CVPixelBufferLockFlags(0);
        // SAFETY: `buffer` is a live pixel buffer only this thread holds.
        if unsafe { CVPixelBufferLockBaseAddress(&buffer, flags) } != kCVReturnSuccess {
            return None;
        }
        let space = CGColorSpace::new_device_rgb();
        // SAFETY: the buffer stays locked until after the drawing below, so its memory is
        // valid. The pool made it `width` by `height`, 32-bit BGRA, at this row stride.
        let context: Option<CFRetained<CGContext>> = unsafe {
            CGBitmapContextCreate(
                CVPixelBufferGetBaseAddress(&buffer),
                self.width,
                self.height,
                8,
                CVPixelBufferGetBytesPerRow(&buffer),
                space.as_deref(),
                CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0,
            )
        };
        if let Some(context) = &context {
            let (frame_w, frame_h) = (self.width as f64, self.height as f64);
            CGContext::set_rgb_fill_color(Some(context), 1.0, 1.0, 1.0, 1.0);
            CGContext::fill_rect(
                Some(context),
                CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(frame_w, frame_h)),
            );
            let (image_w, image_h) = (
                CGImage::width(Some(image)) as f64,
                CGImage::height(Some(image)) as f64,
            );
            let scale = (frame_w / image_w.max(1.0)).min(frame_h / image_h.max(1.0));
            let (drawn_w, drawn_h) = (image_w * scale, image_h * scale);
            CGContext::draw_image(
                Some(context),
                CGRect::new(
                    CGPoint::new((frame_w - drawn_w) / 2.0, frame_h - drawn_h),
                    CGSize::new(drawn_w, drawn_h),
                ),
                Some(image),
            );
        }
        // SAFETY: unlocks with the flags it was locked with; nothing draws into it after this.
        unsafe { CVPixelBufferUnlockBaseAddress(&buffer, flags) };
        context.map(|_| buffer)
    }

    fn finish(self, frames: u64, length: Duration) -> Result<Summary, String> {
        // SAFETY: the writer belongs to this thread; reading its status is always allowed.
        if unsafe { self.writer.status() } != AVAssetWriterStatus::Writing {
            return Err(self.failure());
        }
        let (sender, finished) = mpsc::channel();
        let block = RcBlock::new(move || {
            let _ = sender.send(());
        });
        // SAFETY: this thread alone owns the input and writer, and appends no frame after this.
        unsafe {
            self.input.markAsFinished();
            self.writer.finishWritingWithCompletionHandler(&block);
        }
        let _ = finished.recv_timeout(Duration::from_secs(30));
        // SAFETY: the writer belongs to this thread; reading its status is always allowed.
        if unsafe { self.writer.status() } != AVAssetWriterStatus::Completed {
            return Err(self.failure());
        }
        Ok(Summary {
            frames,
            seconds: length.as_secs_f64(),
        })
    }

    fn failure(&self) -> String {
        // SAFETY: the writer belongs to this thread; reading its error is always allowed.
        unsafe { self.writer.error() }
            .map(|error| describe(&error))
            .unwrap_or_else(|| "the video could not be finished".into())
    }
}

fn describe(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

/// Photographs the tab at `width` CSS pixels across and hands the picture over.
pub(super) fn capture(
    webview: &WKWebView,
    width: f64,
    done: Box<dyn FnOnce(Option<CFRetained<CGImage>>) + Send>,
) {
    let webview = webview.retain();
    let Some(mtm) = MainThreadMarker::new() else {
        done(None);
        return;
    };
    // SAFETY: main thread, as `mtm` proves.
    let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
    // SAFETY: `configuration` is ours and not yet handed to WebKit.
    unsafe {
        configuration.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width)));
    }
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
        let Some(done) = done.locked().take() else {
            return;
        };
        // SAFETY: WebKit passes a live image or nil; `retain` takes our own reference.
        let picture = unsafe { Retained::retain(image) }
            // SAFETY: a null rect and no context or hints ask for the image at its own size.
            .and_then(|image| unsafe {
                image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
            })
            // SAFETY: `picture` is a live CGImage; this takes our own reference to it.
            .map(|picture| unsafe { CFRetained::retain(NonNull::from(&*picture)) });
        done(picture);
    });
    // SAFETY: main thread; WebKit copies the block and holds `configuration` for the call.
    unsafe {
        webview.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block)
    };
}
