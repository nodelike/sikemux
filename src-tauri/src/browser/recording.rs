//! Records a tab to an MP4. WebKit has no screencast to tap, so the tab is
//! photographed several times a second and the pictures are encoded as H.264.
//! Only the photographs are taken on the main thread; encoding has its own.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
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
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::BrowserManager;

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const MAX_LENGTH: Duration = Duration::from_secs(10 * 60);
const FINISH_TIMEOUT: Duration = Duration::from_secs(40);

/// A recording in progress for one agent. It follows whichever tab the agent
/// is on, so switching tabs mid-recording keeps filming.
pub struct Session {
    path: PathBuf,
    width: usize,
    height: usize,
    stop: Arc<AtomicBool>,
    finished: mpsc::Receiver<Result<Summary, String>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

struct Frame {
    image: CFRetained<CGImage>,
    at: Duration,
}

struct Summary {
    frames: u64,
    seconds: f64,
}

/// Takes frames until every sender is dropped, then finishes the file.
struct Encoder {
    frames: mpsc::Sender<Frame>,
    finished: mpsc::Receiver<Result<Summary, String>>,
}

pub async fn start(app: &AppHandle, agent_id: &str, path: Option<String>) -> Result<Value, String> {
    let manager = app.state::<BrowserManager>();
    if manager.recordings_lock().contains_key(agent_id) {
        return Err("this agent is already recording; stop it first".into());
    }
    let (tab_id, view) = manager
        .active_view(agent_id)
        .map_err(|_| "no browser tab is open; call browser_navigate first".to_string())?;
    let path = match path {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() || path.extension().is_none_or(|extension| extension != "mp4") {
                return Err("path must be an absolute path ending in .mp4".into());
            }
            if path.exists() {
                return Err(format!("{} already exists", path.display()));
            }
            path
        }
        None => {
            let folder = app
                .path()
                .video_dir()
                .map_err(|error| error.to_string())?
                .join("Sikemux");
            let title = manager.page(agent_id, &tab_id).unwrap_or_default().title;
            default_path(&folder, &title)
        }
    };
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    view.with_webview(move |platform| {
        let size = unsafe { Retained::retain(platform.inner().cast::<WKWebView>()) }
            .map(|webview| webview.frame().size);
        let _ = sender.send(size);
    })
    .map_err(|error| error.to_string())?;
    let size = receiver
        .await
        .ok()
        .flatten()
        .ok_or("the tab has no size to record")?;
    let (width, height) = (even(size.width), even(size.height));
    let encoder = open(&path, width, height)?;
    let stop = Arc::new(AtomicBool::new(false));
    tauri::async_runtime::spawn(film(
        app.clone(),
        agent_id.to_owned(),
        encoder.frames,
        stop.clone(),
        width as f64,
    ));
    manager.recordings_lock().insert(
        agent_id.to_owned(),
        Session {
            path: path.clone(),
            width,
            height,
            stop,
            finished: encoder.finished,
        },
    );
    Ok(json!({ "recording": true, "path": path, "width": width, "height": height }))
}

pub async fn stop(app: &AppHandle, agent_id: &str) -> Result<Value, String> {
    let session = app
        .state::<BrowserManager>()
        .recordings_lock()
        .remove(agent_id)
        .ok_or("nothing is recording")?;
    session.stop.store(true, Ordering::Release);
    let (path, width, height) = (session.path.clone(), session.width, session.height);
    let summary = tauri::async_runtime::spawn_blocking(move || {
        session
            .finished
            .recv_timeout(FINISH_TIMEOUT)
            .map_err(|_| "the video did not finish in time".to_string())
    })
    .await
    .map_err(|error| error.to_string())???;
    Ok(json!({
        "path": path,
        "seconds": (summary.seconds * 10.0).round() / 10.0,
        "frames": summary.frames,
        "width": width,
        "height": height,
    }))
}

/// Photographs the agent's current tab until told to stop, the agent has no
/// tab left, or the recording reaches its length limit.
async fn film(
    app: AppHandle,
    agent_id: String,
    frames: mpsc::Sender<Frame>,
    stop: Arc<AtomicBool>,
    width: f64,
) {
    let started = Instant::now();
    let mut tick = tokio::time::interval(FRAME_INTERVAL);
    while !stop.load(Ordering::Acquire) && started.elapsed() < MAX_LENGTH {
        tick.tick().await;
        let Ok((_, view)) = app.state::<BrowserManager>().active_view(&agent_id) else {
            break;
        };
        let frames = frames.clone();
        let _ = view.with_webview(move |platform| {
            capture(
                platform.inner(),
                width,
                Box::new(move |image| {
                    if let Some(image) = image {
                        let _ = frames.send(Frame {
                            image,
                            at: started.elapsed(),
                        });
                    }
                }),
            );
        });
    }
}

fn open(path: &Path, width: usize, height: usize) -> Result<Encoder, String> {
    let (frames, incoming) = mpsc::channel::<Frame>();
    let (ready, started) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let path = path.to_owned();
    std::thread::Builder::new()
        .name("tab-recording".into())
        .spawn(move || match Writer::open(&path, width, height) {
            Ok(writer) => {
                let _ = ready.send(Ok(()));
                let _ = done.send(writer.write(incoming));
            }
            Err(error) => {
                let _ = ready.send(Err(error));
            }
        })
        .map_err(|error| error.to_string())?;
    started
        .recv()
        .map_err(|_| "the recorder stopped before it started".to_string())??;
    Ok(Encoder { frames, finished })
}

struct Writer {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    width: usize,
    height: usize,
}

fn cf_key(key: &CFString) -> &NSString {
    // CoreFoundation strings and Foundation strings are the same objects.
    unsafe { &*(key as *const CFString).cast::<NSString>() }
}

impl Writer {
    fn open(path: &Path, width: usize, height: usize) -> Result<Self, String> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
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
        let input = unsafe {
            AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media_type,
                Some(&settings),
            )
        };
        unsafe { input.setExpectsMediaDataInRealTime(true) };
        let format = NSNumber::numberWithUnsignedInt(kCVPixelFormatType_32BGRA);
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
        let adaptor = unsafe {
            AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                &input,
                Some(&attributes),
            )
        };
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

    fn write(self, incoming: mpsc::Receiver<Frame>) -> Result<Summary, String> {
        let mut frames = 0;
        let mut last: Option<Duration> = None;
        for frame in incoming {
            if last.is_some_and(|last| frame.at <= last)
                || !unsafe { self.input.isReadyForMoreMediaData() }
            {
                continue;
            }
            let Some(buffer) = self.draw(&frame.image) else {
                continue;
            };
            let at = unsafe { CMTime::new(frame.at.as_millis() as i64, 1000) };
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

    /// Draws a picture into a fresh video frame, fitted inside it: a tab that
    /// is resized while recording keeps its proportions.
    fn draw(&self, image: &CGImage) -> Option<CFRetained<CVPixelBuffer>> {
        let pool: Retained<CVPixelBufferPool> = unsafe { self.adaptor.pixelBufferPool() }?;
        let mut raw: *mut CVPixelBuffer = std::ptr::null_mut();
        let created =
            unsafe { CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut raw)) };
        if created != kCVReturnSuccess {
            return None;
        }
        let buffer = unsafe { CFRetained::from_raw(NonNull::new(raw)?) };
        let flags = CVPixelBufferLockFlags(0);
        if unsafe { CVPixelBufferLockBaseAddress(&buffer, flags) } != kCVReturnSuccess {
            return None;
        }
        let space = CGColorSpace::new_device_rgb();
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
        unsafe { CVPixelBufferUnlockBaseAddress(&buffer, flags) };
        context.map(|_| buffer)
    }

    fn finish(self, frames: u64, length: Duration) -> Result<Summary, String> {
        let (sender, finished) = mpsc::channel();
        let block = RcBlock::new(move || {
            let _ = sender.send(());
        });
        unsafe {
            self.input.markAsFinished();
            self.writer.finishWritingWithCompletionHandler(&block);
        }
        let _ = finished.recv_timeout(Duration::from_secs(30));
        if unsafe { self.writer.status() } != AVAssetWriterStatus::Completed {
            return Err(unsafe { self.writer.error() }
                .map(|error| describe(&error))
                .unwrap_or_else(|| "the video could not be finished".into()));
        }
        Ok(Summary {
            frames,
            seconds: length.as_secs_f64(),
        })
    }
}

fn describe(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

/// Photographs the tab at `width` CSS pixels across and hands the picture over.
fn capture(
    pointer: *mut c_void,
    width: f64,
    done: Box<dyn FnOnce(Option<CFRetained<CGImage>>) + Send>,
) {
    let (Some(webview), Some(mtm)) = (
        unsafe { Retained::retain(pointer.cast::<WKWebView>()) },
        MainThreadMarker::new(),
    ) else {
        done(None);
        return;
    };
    let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
    unsafe {
        configuration.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width)));
    }
    let done = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
        let Some(done) = done.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        let picture = unsafe { Retained::retain(image) }
            .and_then(|image| unsafe {
                image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
            })
            .map(|picture| unsafe { CFRetained::retain(NonNull::from(&*picture)) });
        done(picture);
    });
    unsafe {
        webview.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block)
    };
}

/// Video encoders want even sides.
fn even(side: f64) -> usize {
    ((side.round() as usize).max(2) / 2) * 2
}

fn default_path(folder: &Path, title: &str) -> PathBuf {
    let cleaned: String = title
        .chars()
        .map(|char| {
            if matches!(char, '/' | '\\' | ':') {
                '_'
            } else {
                char
            }
        })
        .collect();
    let name = cleaned.trim();
    let name = if name.is_empty() { "Recording" } else { name };
    super::unique_download_path(folder, &format!("{name}.mp4"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_sides_are_even_and_never_empty() {
        assert_eq!(even(1201.4), 1200);
        assert_eq!(even(801.0), 800);
        assert_eq!(even(0.0), 2);
    }

    #[test]
    fn a_recording_is_named_after_its_page() {
        let folder = std::env::temp_dir().join("sikemux-recording-names");
        assert_eq!(
            default_path(&folder, "Sign in: Acme/Admin"),
            folder.join("Sign in_ Acme_Admin.mp4")
        );
        assert_eq!(default_path(&folder, "  "), folder.join("Recording.mp4"));
    }
}
