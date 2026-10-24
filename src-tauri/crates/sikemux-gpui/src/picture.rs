//! A picture shown properly: over the whole window, as large as the window lets it
//! be, with a way to keep a copy in Downloads.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use gpui_kit::component::WindowExt;
use gpui_kit::{
    div, img, App, Asset, Context, CursorStyle, Div, ElementId, FontWeight, Hsla, Image,
    ImageCacheError, ImageFormat, ImageSource, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, Pixels, RenderImage, SharedString, StatefulInteractiveElement, Styled,
    StyledImage, WeakEntity, Window,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::core::Ask;
use crate::icons::Icon;
use crate::theme;
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// A picture from a file, one an agent sent as bytes, or one on the web.
#[derive(Clone)]
pub enum Picture {
    File(PathBuf),
    Sent {
        name: SharedString,
        image: Arc<Image>,
    },
    Remote(Remote),
}

/// A picture on the web, which the core fetches, since only it knows which addresses are
/// safe to reach.
#[derive(Clone)]
pub struct Remote {
    url: SharedString,
    asks: mpsc::UnboundedSender<Ask>,
}

impl Remote {
    pub fn new(url: impl Into<SharedString>, asks: mpsc::UnboundedSender<Ask>) -> Self {
        Remote {
            url: url.into(),
            asks,
        }
    }

    fn fetch(&self) -> impl std::future::Future<Output = Result<Value, String>> {
        crate::core::request(&self.asks, "remoteImage", json!({ "url": self.url }))
    }
}

impl Hash for Remote {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.url.hash(state);
    }
}

/// GPUI's loader for a picture on the web: it asks the core, then draws what came back as
/// it draws any other picture.
enum Fetched {}

impl Asset for Fetched {
    type Source = Remote;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        remote: Remote,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let (renderer, fetched) = (cx.svg_renderer(), remote.fetch());
        async move {
            let answer = fetched
                .await
                .map_err(|error| ImageCacheError::Asset(error.into()))?;
            let image = decode(
                answer["mime"].as_str().unwrap_or_default(),
                answer["data"].as_str().unwrap_or_default(),
            )
            .ok_or_else(|| ImageCacheError::Asset("Couldn't load the picture".into()))?;
            image
                .to_image_data(renderer)
                .map_err(|error| ImageCacheError::Other(Arc::new(error)))
        }
    }
}

/// A picture decoded to no more than `longest` pixels along its longer side, for drawing it
/// small: a screenshot holds tens of megabytes decoded, and as much again as its texture.
#[derive(Clone, Hash)]
struct Thumb {
    picture: Shrunk,
    longest: u32,
}

#[derive(Clone, Hash)]
enum Shrunk {
    File(PathBuf),
    Sent(Arc<Image>),
}

/// GPUI's loader for a thumbnail, which decodes the picture once and keeps only its
/// shrunk copy.
enum Thumbnail {}

/// How many thumbnails decode at once: each holds its whole picture until it is shrunk, so
/// twenty pasted together would otherwise hold twenty screenshots at their full size.
static DECODING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

impl Asset for Thumbnail {
    type Source = Thumb;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        thumb: Thumb,
        _: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        shrink_when_free(thumb)
    }
}

/// Shrinks `thumb` once fewer than the most allowed are decoding.
async fn shrink_when_free(thumb: Thumb) -> Result<Arc<RenderImage>, ImageCacheError> {
    let _decoding = DECODING.acquire().await;
    shrink(&thumb).map_err(|error| ImageCacheError::Asset(error.into()))
}

fn shrink(thumb: &Thumb) -> Result<Arc<RenderImage>, String> {
    let read;
    let bytes = match &thumb.picture {
        Shrunk::File(path) => {
            read = std::fs::read(path).map_err(|error| error.to_string())?;
            read.as_slice()
        }
        Shrunk::Sent(image) => image.bytes.as_slice(),
    };
    let decoded = match jpeg_near(bytes, thumb.longest) {
        Some(decoded) => decoded,
        None => image::load_from_memory(bytes).map_err(|error| error.to_string())?,
    };
    let fitted = match decoded.width().max(decoded.height()) > thumb.longest {
        true => decoded.thumbnail(thumb.longest, thumb.longest),
        false => decoded,
    };
    let mut pixels = fitted.into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new([image::Frame::new(pixels)])))
}

/// A JPEG decoded at an eighth, a quarter or half its size, the smallest still at least
/// `longest` along one side, so its whole picture is never held; none for other pictures.
fn jpeg_near(bytes: &[u8], longest: u32) -> Option<image::DynamicImage> {
    if image::guess_format(bytes).ok()? != image::ImageFormat::Jpeg {
        return None;
    }
    let mut decoder = jpeg_decoder::Decoder::new(bytes);
    let side = longest.min(u32::from(u16::MAX)) as u16;
    let (width, height) = decoder.scale(side, side).ok()?;
    let format = decoder.info()?.pixel_format;
    let pixels = decoder.decode().ok()?;
    let (width, height) = (u32::from(width), u32::from(height));
    match format {
        jpeg_decoder::PixelFormat::RGB24 => {
            image::RgbImage::from_raw(width, height, pixels).map(Into::into)
        }
        jpeg_decoder::PixelFormat::L8 => {
            image::GrayImage::from_raw(width, height, pixels).map(Into::into)
        }
        _ => None,
    }
}

/// Whether a picture of this kind is shown from a shrunk copy when drawn small. Animations and
/// drawings are not: a thumbnail would hold one still frame, and a drawing costs little.
fn shrinks(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Webp
            | ImageFormat::Bmp
            | ImageFormat::Tiff
    )
}

impl From<PathBuf> for Picture {
    fn from(path: PathBuf) -> Self {
        Picture::File(path)
    }
}

/// A picture as its type and its bytes in base64.
pub fn decode(mime: &str, base64: &str) -> Option<Arc<Image>> {
    let format = ImageFormat::from_mime_type(mime)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

impl Picture {
    /// A picture an agent sent, named by its type since nothing else names it.
    pub fn sent(mime: &str, base64: &str) -> Option<Self> {
        let kind: String = mime
            .rsplit('/')
            .next()
            .and_then(|kind| kind.split('+').next())
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let kind = if kind.is_empty() {
            "png".to_owned()
        } else {
            kind
        };
        Some(Picture::Sent {
            name: format!("attachment.{kind}").into(),
            image: decode(mime, base64)?,
        })
    }

    pub fn name(&self) -> SharedString {
        match self {
            Picture::File(path) => path
                .file_name()
                .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy())
                .into_owned()
                .into(),
            Picture::Sent { name, .. } => name.clone(),
            Picture::Remote(remote) => remote
                .url
                .split(['?', '#'])
                .next()
                .and_then(|path| path.rsplit('/').next())
                .filter(|name| !name.is_empty())
                .unwrap_or("picture")
                .to_owned()
                .into(),
        }
    }

    /// The picture as `img` draws it no larger than `longest` points: from a copy decoded at
    /// twice that, for a sharp screen, where it would otherwise be decoded whole.
    pub fn source_at(&self, longest: Pixels) -> ImageSource {
        let picture = match self {
            Picture::File(path) if format_of(path).is_some_and(shrinks) => {
                Shrunk::File(path.clone())
            }
            Picture::Sent { image, .. } if shrinks(image.format) => Shrunk::Sent(image.clone()),
            _ => return self.source(),
        };
        let thumb = Thumb {
            picture,
            longest: (f32::from(longest) * 2.).ceil() as u32,
        };
        ImageSource::Custom(Arc::new(move |window, cx| {
            let image = window.use_asset::<Thumbnail>(&thumb, cx);
            if let Some(Ok(image)) = &image {
                cx.decoded_image_drawn::<Thumbnail>(&thumb, image);
            }
            image
        }))
    }

    pub fn source(&self) -> ImageSource {
        match self {
            Picture::File(path) => path.clone().into(),
            Picture::Sent { image, .. } => image.clone().into(),
            Picture::Remote(remote) => {
                let remote = remote.clone();
                ImageSource::Custom(Arc::new(move |window, cx| {
                    window.use_asset::<Fetched>(&remote, cx)
                }))
            }
        }
    }
}

/// What kind of picture the file at `path` holds, by its name.
fn format_of(path: &Path) -> Option<ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "jpg" => Some(ImageFormat::Jpeg),
        "tif" => Some(ImageFormat::Tiff),
        other => ImageFormat::from_mime_type(&format!("image/{other}")),
    }
}

/// A picture in a transcript at its own size, no larger than `width` by `height`, which
/// opens over the window when clicked.
pub fn thumb(
    id: impl Into<ElementId>,
    picture: Picture,
    width: Pixels,
    height: Pixels,
    bench: WeakEntity<Workbench>,
    quiet: Hsla,
) -> Div {
    let source = picture.source_at(width.max(height));
    let clickable = div()
        .id(id)
        .max_w(width)
        .rounded(theme::RADIUS_2)
        .overflow_hidden()
        .cursor(CursorStyle::PointingHand)
        .on_click(move |_, window, cx| {
            bench
                .update(cx, |bench, cx| {
                    bench.show_picture(picture.clone(), window, cx)
                })
                .ok();
        })
        .child(
            img(source)
                .max_w(width)
                .max_h(height)
                .object_fit(ObjectFit::Contain)
                .with_fallback(move || {
                    ui::text("Couldn't load the picture", 12., FontWeight::NORMAL)
                        .text_color(quiet)
                        .into_any_element()
                }),
        );
    div().flex().child(clickable)
}

impl Workbench {
    /// Shows `picture` over the window until Esc, a click outside it, or ✕.
    pub fn show_picture(
        &self,
        picture: impl Into<Picture>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picture = picture.into();
        let name = picture.name();
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, _| {
            let room = window.viewport_size();
            let save = {
                let (this, picture) = (this.clone(), picture.clone());
                Button::new("picture-save", "Save to Downloads", Variant::Outline)
                    .icon(Icon::Download)
                    .on_click(move |_, window, cx| {
                        this.update(cx, |bench, cx| bench.keep_picture(&picture, window, cx))
                            .ok();
                    })
            };
            dialog
                .w(room.width * 0.9)
                .max_w(room.width * 0.9)
                .margin_top(room.height * 0.04)
                .title(name.clone())
                .child(
                    div().flex().justify_center().child(
                        img(picture.source())
                            .max_w_full()
                            .max_h(room.height * 0.78)
                            .object_fit(ObjectFit::Contain),
                    ),
                )
                .footer(div().flex().justify_end().child(save))
        });
    }

    /// Copies the picture into the Downloads folder of the host the core runs on.
    fn keep_picture(&self, picture: &Picture, window: &mut Window, cx: &mut Context<Self>) {
        let Some(downloads) =
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads"))
        else {
            return;
        };
        let (asks, name, picture) = (self.asks.clone(), picture.name(), picture.clone());
        cx.spawn_in(window, async move |_, cx| {
            let kept = match picture {
                Picture::File(path) => {
                    let asked = json!({ "from": path, "dir": downloads });
                    crate::core::request(&asks, "files.copyInto", asked).await
                }
                Picture::Sent { image, .. } => {
                    let base64 = base64::engine::general_purpose::STANDARD.encode(&image.bytes);
                    write(&asks, &downloads, &name, base64).await
                }
                Picture::Remote(remote) => match remote.fetch().await {
                    Ok(answer) => {
                        let base64 = answer["data"].as_str().unwrap_or_default().to_owned();
                        write(&asks, &downloads, &name, base64).await
                    }
                    Err(error) => Err(error),
                },
            };
            let said = match kept {
                Ok(saved) => {
                    let name = saved
                        .as_str()
                        .and_then(|saved| saved.rsplit('/').next())
                        .unwrap_or("the picture")
                        .to_owned();
                    format!("Saved {name} to Downloads")
                }
                Err(error) => format!("Could not save the picture: {error}"),
            };
            cx.update(|window, cx| window.push_notification(said, cx))
                .ok();
        })
        .detach();
    }
}

fn write(
    asks: &mpsc::UnboundedSender<Ask>,
    dir: &Path,
    name: &str,
    base64: String,
) -> impl std::future::Future<Output = Result<Value, String>> {
    let asked = json!({ "dir": dir, "name": name, "base64": base64 });
    crate::core::request(asks, "files.writeBytes", asked)
}
