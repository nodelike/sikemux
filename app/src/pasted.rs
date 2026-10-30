//! Pictures pasted from the clipboard, in a format every agent reads and small enough to
//! send to a chat. macOS often hands a copied picture over as TIFF only, which agents
//! cannot open, and a copied photo can be larger than a chat takes.

use gpui_kit::{Image, ImageFormat};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::DynamicImage;
use sikemux_core::protocol::MAX_ATTACHMENT_BYTES;

const JPEG_QUALITY: u8 = 85;

/// `image` as it is when agents read its format and a chat takes its size; else as a PNG,
/// or as a JPEG made smaller until a chat takes it; as it is when it does not decode.
pub fn readable(image: Image) -> Image {
    readable_within(image, MAX_ATTACHMENT_BYTES)
}

fn readable_within(image: Image, limit: usize) -> Image {
    if agents_read(image.format) && image.bytes.len() <= limit {
        return image;
    }
    let Ok(decoded) = image::load_from_memory(&image.bytes) else {
        return image;
    };
    if let Some(png) = png(&decoded).filter(|png| png.len() <= limit) {
        return Image::from_bytes(ImageFormat::Png, png);
    }
    match jpeg_within(decoded, limit) {
        Some(jpeg) => Image::from_bytes(ImageFormat::Jpeg, jpeg),
        None => image,
    }
}

fn agents_read(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Webp | ImageFormat::Gif
    )
}

fn png(picture: &DynamicImage) -> Option<Vec<u8>> {
    let mut png = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

/// `picture` as a JPEG of at most `limit` bytes, shrunk a quarter at a time until it fits.
fn jpeg_within(picture: DynamicImage, limit: usize) -> Option<Vec<u8>> {
    let mut picture = DynamicImage::ImageRgb8(picture.into_rgb8());
    loop {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
            .encode_image(&picture)
            .ok()?;
        if jpeg.len() <= limit {
            return Some(jpeg);
        }
        let (width, height) = (picture.width() * 3 / 4, picture.height() * 3 / 4);
        if width == 0 || height == 0 {
            return None;
        }
        picture = picture.resize(width, height, FilterType::Triangle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(picture: &DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        picture
            .write_to(&mut std::io::Cursor::new(&mut bytes), format)
            .expect("encode");
        bytes
    }

    #[test]
    fn a_tiff_from_the_clipboard_is_kept_as_a_png() {
        let picture = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            3,
            2,
            image::Rgba([10, 20, 30, 255]),
        ));
        let tiff = encoded(&picture, image::ImageFormat::Tiff);
        let kept = readable(Image::from_bytes(ImageFormat::Tiff, tiff));
        assert_eq!(kept.format, ImageFormat::Png);
        let decoded = image::load_from_memory(&kept.bytes).expect("decode");
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
    }

    #[test]
    fn a_picture_larger_than_a_chat_takes_is_made_a_jpeg_that_fits() {
        let noise = image::RgbImage::from_fn(256, 256, |x, y| {
            let n = (x * 7919 + y * 104_729) ^ (x * y);
            image::Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
        });
        let png = encoded(&DynamicImage::ImageRgb8(noise), image::ImageFormat::Png);
        let limit = png.len() / 4;
        let kept = readable_within(Image::from_bytes(ImageFormat::Png, png), limit);
        assert_eq!(kept.format, ImageFormat::Jpeg);
        assert!(kept.bytes.len() <= limit, "{} > {limit}", kept.bytes.len());
        image::load_from_memory(&kept.bytes).expect("decode");
    }

    #[test]
    fn a_picture_a_chat_takes_is_left_as_it_is() {
        let picture = DynamicImage::ImageRgb8(image::RgbImage::new(4, 4));
        let png = encoded(&picture, image::ImageFormat::Png);
        let kept = readable(Image::from_bytes(ImageFormat::Png, png.clone()));
        assert_eq!((kept.format, kept.bytes), (ImageFormat::Png, png));
    }
}
