//! A picture shown properly: over the whole window, as large as the window lets it
//! be, with a way to keep a copy in Downloads.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use gpui_kit::component::WindowExt;
use gpui_kit::{
    div, img, px, size, App, Asset, Context, CursorStyle, Div, ElementId, FontWeight, Hsla, Image,
    ImageCacheError, ImageFormat, ImageSource, ImgResourceLoader, InteractiveElement, IntoElement,
    ObjectFit, ParentElement, Pixels, RenderImage, SharedString, Size, StatefulInteractiveElement,
    Styled, StyledImage, WeakEntity, Window,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::core::Ask;
use crate::icons::Icon;
use crate::theme;
use crate::theme::{SPACE_2, SPACE_3, SPACE_4, SPACE_6, TEXT_SM, TEXT_XS};
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

/// A picture the core fetches: one on the web, since only it knows which addresses are safe
/// to reach, or one in the account's Stash.
#[derive(Clone)]
pub struct Remote {
    url: SharedString,
    stashed: bool,
    asks: mpsc::UnboundedSender<Ask>,
}

impl Remote {
    pub fn new(url: impl Into<SharedString>, asks: mpsc::UnboundedSender<Ask>) -> Self {
        Remote {
            url: url.into(),
            stashed: false,
            asks,
        }
    }

    /// The picture stash item `id` holds.
    pub fn stashed(id: impl Into<SharedString>, asks: mpsc::UnboundedSender<Ask>) -> Self {
        Remote {
            url: id.into(),
            stashed: true,
            asks,
        }
    }

    fn fetch(&self) -> impl std::future::Future<Output = Result<Value, String>> {
        match self.stashed {
            true => crate::core::request(&self.asks, "stashPicture", json!({ "id": self.url })),
            false => crate::core::request(&self.asks, "remoteImage", json!({ "url": self.url })),
        }
    }

    fn image(&self) -> impl std::future::Future<Output = Result<Arc<Image>, ImageCacheError>> {
        let fetched = self.fetch();
        async move {
            let answer = fetched
                .await
                .map_err(|error| ImageCacheError::Asset(error.into()))?;
            decode(
                answer["mime"].as_str().unwrap_or_default(),
                answer["data"].as_str().unwrap_or_default(),
            )
            .ok_or_else(|| ImageCacheError::Asset("Couldn't load the picture".into()))
        }
    }
}

impl Hash for Remote {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.url.hash(state);
        self.stashed.hash(state);
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
        let (renderer, image) = (cx.svg_renderer(), remote.image());
        async move {
            image
                .await?
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
    Remote(Remote),
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
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        async move {
            let unreadable = |error: String| ImageCacheError::Asset(error.into());
            let image = match thumb.picture {
                Shrunk::File(path) => {
                    let _decoding = DECODING.acquire().await;
                    let read =
                        std::fs::read(path).map_err(|error| unreadable(error.to_string()))?;
                    return shrink(&read, thumb.longest).map_err(unreadable);
                }
                Shrunk::Sent(image) => image,
                Shrunk::Remote(remote) => {
                    let image = remote.image().await?;
                    if !shrinks(image.format) {
                        return image
                            .to_image_data(renderer)
                            .map_err(|error| ImageCacheError::Other(Arc::new(error)));
                    }
                    image
                }
            };
            let _decoding = DECODING.acquire().await;
            shrink(&image.bytes, thumb.longest).map_err(unreadable)
        }
    }
}

/// The picture in `bytes` no larger than `longest` along its longer side.
fn shrink(bytes: &[u8], longest: u32) -> Result<Arc<RenderImage>, String> {
    let decoded = match jpeg_near(bytes, longest) {
        Some(decoded) => decoded,
        None => image::load_from_memory(bytes).map_err(|error| error.to_string())?,
    };
    let fitted = match decoded.width().max(decoded.height()) > longest {
        true => decoded.thumbnail(longest, longest),
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
    let format = match mime {
        "image/x-icon" | "image/vnd.microsoft.icon" => ImageFormat::Ico,
        mime => ImageFormat::from_mime_type(mime)?,
    };
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
            Picture::Remote(remote) => Shrunk::Remote(remote.clone()),
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

    /// The picture's own size, once it has loaded.
    fn size(&self, window: &mut Window, cx: &mut App) -> Option<Size<Pixels>> {
        let loaded = match self {
            Picture::File(path) => window
                .use_asset::<ImgResourceLoader>(&path.clone().into(), cx)?
                .ok(),
            Picture::Sent { image, .. } => image.clone().use_render_image(window, cx),
            Picture::Remote(remote) => window.use_asset::<Fetched>(remote, cx)?.ok(),
        }?;
        let pixels = loaded.size(0);
        Some(size(px(pixels.width.0 as f32), px(pixels.height.0 as f32)))
    }

    pub fn source(&self) -> ImageSource {
        match self {
            Picture::File(path) => path.clone().into(),
            Picture::Sent { image, .. } => image.clone().into(),
            Picture::Remote(remote) => {
                let remote = remote.clone();
                ImageSource::Custom(Arc::new(move |window, cx| {
                    let image = window.use_asset::<Fetched>(&remote, cx);
                    if let Some(Ok(image)) = &image {
                        cx.decoded_image_drawn::<Fetched>(&remote, image);
                    }
                    image
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
                    ui::text("Couldn't load the picture", TEXT_XS, FontWeight::NORMAL)
                        .text_color(quiet)
                        .into_any_element()
                }),
        );
    div().flex().child(clickable)
}

/// Room the picture viewer keeps beside the picture, and above and below it for its
/// name, its buttons and the gaps between.
const VIEWER_SIDES: Pixels = px(16.);
const VIEWER_CHROME: Pixels = px(8. + 24. + 12. + 12. + 32. + 16. + 2.);

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
        window.open_dialog(cx, move |dialog, window, cx| {
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
            let most = size(
                room.width * 0.9 - VIEWER_SIDES * 2.,
                room.height * 0.9 - VIEWER_CHROME,
            );
            let shown = picture.size(window, cx).map_or(most, |natural| {
                let scale = (most.width / natural.width)
                    .min(most.height / natural.height)
                    .min(1.);
                size(natural.width * scale, natural.height * scale)
            });
            let height = shown.height + VIEWER_CHROME;
            dialog
                .w((shown.width + VIEWER_SIDES * 2.).max(px(360.)))
                .pt(SPACE_2)
                .pb(SPACE_4)
                .margin_top(((room.height - height) / 2.).max(px(0.)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACE_3)
                        .child(
                            div()
                                .h(px(24.))
                                .pr(SPACE_6)
                                .flex()
                                .items_center()
                                .child(ui::text(name.clone(), TEXT_SM, FontWeight::SEMIBOLD)),
                        )
                        .child(
                            div().flex().justify_center().child(
                                img(picture.source())
                                    .w(shown.width)
                                    .h(shown.height)
                                    .object_fit(ObjectFit::Contain),
                            ),
                        )
                        .child(
                            div()
                                .h(px(32.))
                                .flex()
                                .items_center()
                                .justify_end()
                                .child(save),
                        ),
                )
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
