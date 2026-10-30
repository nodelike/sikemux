//! A tab's recording on Linux: WebKitGTK's snapshots, encoded by GStreamer as H.264 MP4
//! when the system has an H.264 encoder, otherwise as VP8 WebM.

use std::path::Path;
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;
use gtk::prelude::WidgetExt;
use gtk::{cairo, gdk, gdk_pixbuf, glib};
use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebView, WebViewExt};

use super::{Frame, Summary};

/// GStreamer's H.264 encoders, the first one installed is used; without any, VP8.
const H264_ENCODERS: [&str; 2] = ["x264enc", "openh264enc"];
const FRAMES_PER_SECOND: i32 = 10;

/// The tab's picture as WebKit drew it.
pub(super) struct Picture {
    pixels: glib::Bytes,
    width: i32,
    height: i32,
    stride: i32,
    alpha: bool,
}

enum Codec {
    H264(&'static str),
    Vp8,
}

fn codec() -> &'static Codec {
    static CODEC: OnceLock<Codec> = OnceLock::new();
    CODEC.get_or_init(|| {
        let installed = gst::init().is_ok();
        H264_ENCODERS
            .into_iter()
            .find(|name| installed && gst::ElementFactory::find(name).is_some())
            .map_or(Codec::Vp8, Codec::H264)
    })
}

pub(super) fn extension() -> &'static str {
    match codec() {
        Codec::H264(_) => "mp4",
        Codec::Vp8 => "webm",
    }
}

pub(super) fn size(webview: &WebView) -> (f64, f64) {
    let shown = webview.allocation();
    (f64::from(shown.width()), f64::from(shown.height()))
}

/// Photographs the tab and hands the picture over; it is fitted to the video's frame when
/// it is encoded, off this thread.
pub(super) fn capture(
    webview: &WebView,
    _width: f64,
    done: Box<dyn FnOnce(Option<Picture>) + Send>,
) {
    webview.snapshot(
        SnapshotRegion::Visible,
        SnapshotOptions::NONE,
        None::<&gtk::gio::Cancellable>,
        move |surface| {
            let picture = surface
                .ok()
                .and_then(|surface| cairo::ImageSurface::try_from(surface).ok())
                .and_then(|surface| {
                    gdk::pixbuf_get_from_surface(&surface, 0, 0, surface.width(), surface.height())
                })
                .map(|pixbuf| Picture {
                    pixels: pixbuf.read_pixel_bytes(),
                    width: pixbuf.width(),
                    height: pixbuf.height(),
                    stride: pixbuf.rowstride(),
                    alpha: pixbuf.has_alpha(),
                });
            done(picture);
        },
    );
}

pub(super) struct Writer {
    pipeline: gst::Pipeline,
    source: AppSrc,
    width: i32,
    height: i32,
}

impl Writer {
    pub(super) fn open(path: &Path, width: usize, height: usize) -> Result<Self, String> {
        gst::init().map_err(|error| format!("GStreamer could not start: {error}"))?;
        let (width, height) = (
            i32::try_from(width).map_err(|_| "the tab is too wide to record")?,
            i32::try_from(height).map_err(|_| "the tab is too tall to record")?,
        );
        let encode = match codec() {
            Codec::H264("x264enc") => "x264enc ! mp4mux".to_owned(),
            Codec::H264(encoder) => format!("{encoder} ! h264parse ! mp4mux"),
            Codec::Vp8 => "vp8enc deadline=1 cpu-used=8 ! webmmux".to_owned(),
        };
        let line = format!(
            "appsrc name=source is-live=true format=time \
             caps=video/x-raw,format=RGBA,width={width},height={height},framerate={FRAMES_PER_SECOND}/1 \
             ! videoconvert ! {encode} ! filesink name=file"
        );
        let missing = || {
            "this system lacks GStreamer's video plugins; install gstreamer1.0-plugins-good"
                .to_string()
        };
        let pipeline = gst::parse_launch(&line)
            .map_err(|_| missing())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| missing())?;
        let source = pipeline
            .by_name("source")
            .and_then(|source| source.downcast::<AppSrc>().ok())
            .ok_or_else(missing)?;
        pipeline
            .by_name("file")
            .ok_or_else(missing)?
            .set_property("location", path.to_string_lossy().as_ref());
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| "the video writer did not start".to_string())?;
        Ok(Self {
            pipeline,
            source,
            width,
            height,
        })
    }

    pub(super) fn write(self, incoming: mpsc::Receiver<Frame>) -> Result<Summary, String> {
        let mut frames = 0;
        let mut last: Option<Duration> = None;
        for frame in incoming {
            let at_ms = frame.at.as_millis();
            if last.is_some_and(|last| at_ms <= last.as_millis()) {
                continue;
            }
            let Some(pixels) = self.fit(&frame.image) else {
                continue;
            };
            let mut buffer = gst::Buffer::from_mut_slice(pixels);
            if let Some(buffer) = buffer.get_mut() {
                buffer.set_pts(gst::ClockTime::from_mseconds(at_ms as u64));
            }
            if self.source.push_buffer(buffer).is_ok() {
                frames += 1;
                last = Some(frame.at);
            }
        }
        self.finish(frames, last.unwrap_or_default())
    }

    /// The picture scaled to fit the frame, its top centred on white, as RGBA rows.
    fn fit(&self, picture: &Picture) -> Option<Vec<u8>> {
        let image = gdk_pixbuf::Pixbuf::from_bytes(
            &picture.pixels,
            gdk_pixbuf::Colorspace::Rgb,
            picture.alpha,
            8,
            picture.width,
            picture.height,
            picture.stride,
        );
        let frame = gdk_pixbuf::Pixbuf::new(
            gdk_pixbuf::Colorspace::Rgb,
            true,
            8,
            self.width,
            self.height,
        )?;
        frame.fill(0xffff_ffff);
        let scale = (f64::from(self.width) / f64::from(picture.width.max(1)))
            .min(f64::from(self.height) / f64::from(picture.height.max(1)));
        let (drawn_w, drawn_h) = (
            ((f64::from(picture.width) * scale).round() as i32).clamp(1, self.width),
            ((f64::from(picture.height) * scale).round() as i32).clamp(1, self.height),
        );
        let left = (self.width - drawn_w) / 2;
        image.composite(
            &frame,
            left,
            0,
            drawn_w,
            drawn_h,
            f64::from(left),
            0.0,
            scale,
            scale,
            gdk_pixbuf::InterpType::Bilinear,
            255,
        );
        let row = self.width as usize * 4;
        let stride = frame.rowstride() as usize;
        let pixels = frame.read_pixel_bytes();
        Some(
            pixels
                .chunks(stride)
                .take(self.height as usize)
                .flat_map(|line| &line[..row])
                .copied()
                .collect(),
        )
    }

    fn finish(self, frames: u64, length: Duration) -> Result<Summary, String> {
        let _ = self.source.end_of_stream();
        let bus = self.pipeline.bus().ok_or("the video writer has no bus")?;
        let ended = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        let _ = self.pipeline.set_state(gst::State::Null);
        match ended.as_ref().map(|message| message.view()) {
            Some(gst::MessageView::Eos(_)) => Ok(Summary {
                frames,
                seconds: length.as_secs_f64(),
            }),
            Some(gst::MessageView::Error(error)) => Err(error.error().to_string()),
            _ => Err("the video could not be finished".into()),
        }
    }
}
