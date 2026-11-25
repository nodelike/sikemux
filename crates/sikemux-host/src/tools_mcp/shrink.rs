//! Pictures a tool hands the model, made small enough that a few of them do
//! not fill the agent's media allowance. Files a tool saves stay full size.

use std::io::Cursor;

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};

/// UI text still reads at this size; the model is charged about one token per 750 pixels.
pub const LONGEST_SIDE: u32 = 1000;
const JPEG_QUALITY: u8 = 82;

pub struct Picture {
    pub data: String,
    pub mime_type: String,
    /// The shrunk picture's size over the original's, when it was shrunk.
    pub scale: Option<f64>,
}

/// `data`, base64 of a `mime_type` picture, fitted inside [`LONGEST_SIDE`] and
/// sent as JPEG. A picture already small enough, one with see-through parts
/// JPEG cannot keep, or one that cannot be read, is passed on as it came.
pub fn for_model(data: &str, mime_type: &str) -> Picture {
    shrunk(data, mime_type).unwrap_or_else(|| Picture {
        data: data.to_owned(),
        mime_type: mime_type.to_owned(),
        scale: None,
    })
}

fn shrunk(data: &str, mime_type: &str) -> Option<Picture> {
    let format = match mime_type {
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        _ => return None,
    };
    let engine = base64::engine::general_purpose::STANDARD;
    let bytes = engine.decode(data).ok()?;
    let picture = image::load_from_memory_with_format(&bytes, format).ok()?;
    let (width, height) = picture.dimensions();
    if width.max(height) <= LONGEST_SIDE || has_transparency(&picture) {
        return None;
    }
    let small = picture
        .resize(LONGEST_SIDE, LONGEST_SIDE, FilterType::CatmullRom)
        .into_rgb8();
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), JPEG_QUALITY)
        .encode_image(&small)
        .ok()?;
    Some(Picture {
        data: engine.encode(jpeg),
        mime_type: "image/jpeg".to_owned(),
        scale: Some(f64::from(small.width()) / f64::from(width)),
    })
}

fn has_transparency(picture: &DynamicImage) -> bool {
    match picture {
        DynamicImage::ImageLumaA8(image) => image
            .as_raw()
            .as_chunks::<2>()
            .0
            .iter()
            .any(|&[_, a]| a < 255),
        DynamicImage::ImageRgba8(image) => image
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .any(|&[_, _, _, a]| a < 255),
        DynamicImage::ImageLumaA16(image) => image
            .as_raw()
            .as_chunks::<2>()
            .0
            .iter()
            .any(|&[_, a]| a < u16::MAX),
        DynamicImage::ImageRgba16(image) => image
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .any(|&[_, _, _, a]| a < u16::MAX),
        DynamicImage::ImageRgba32F(image) => image
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .any(|&[_, _, _, a]| a < 1.0),
        _ => false,
    }
}
